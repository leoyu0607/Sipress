/// 端到端對照：民眾端引擎 → mock SIPX（B2BUA，每段換新 Call-ID）→ 座席端引擎，
/// 用兩端的追蹤紀錄配對
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::correlate::{correlate, CorrelateOptions};
use sipress_core::engine::Engine;
use sipress_core::sip::{SipMessage, SipParser};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;

/// 在 header 區塊最後（Content-Length 前）加一行
fn with_header(msg: &str, line: &str) -> String {
    msg.replacen(
        "Content-Length:",
        &format!("{}\r\nContent-Length:", line),
        1,
    )
}

/// mock SIPX 對座席送出的請求
fn to_agent(
    method: &str,
    call_id: &str,
    cseq: u32,
    caller: &str,
    mock: SocketAddr,
    agent: SocketAddr,
    cin: Option<&str>,
) -> String {
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 9 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n";
    let body = if method == "INVITE" { sdp } else { "" };
    let ctype = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/sdp\r\n"
    };
    let cin = cin
        .map(|c| format!("P-CIN-CallId: {}\r\n", c))
        .unwrap_or_default();
    format!(
        "{method} sip:1001@{agent} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {mock};branch=z9hG4bK-{call_id}-{cseq}\r\n\
         Max-Forwards: 70\r\n\
         From: \"{caller}\" <sip:{caller}@{mock};user=phone>;tag=sipx\r\n\
         To: <sip:1001@{agent}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} {method}\r\n\
         Contact: <sip:sipx@{mock}>\r\n\
         {cin}{ctype}Content-Length: {len}\r\n\r\n{body}",
        len = body.len(),
    )
}

struct Leg {
    caller_raw: String,
    caller_addr: SocketAddr,
    cin: String,
    agent_call_id: String,
}

/// mock SIPX：民眾端 INVITE → 指派 P-CIN-CallId → 以新 Call-ID 轉給已註冊的座席；
/// 座席 180 / 200 回給民眾端（`send_cin` 時兩個方向都帶 P-CIN-CallId），民眾端 BYE 轉給座席
async fn spawn_sipx(send_cin: bool) -> SocketAddr {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut agent_addr: Option<SocketAddr> = None;
        let mut legs: Vec<Leg> = Vec::new();
        let mut by_caller: HashMap<String, usize> = HashMap::new();
        let mut by_agent: HashMap<String, usize> = HashMap::new();
        while let Ok((n, from)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            let call_id = SipParser::call_id(&raw).unwrap_or_default();
            let send = |msg: String, to: SocketAddr| {
                let sock = Arc::clone(&sock);
                async move {
                    let _ = sock.send_to(msg.as_bytes(), to).await;
                }
            };
            if raw.starts_with("REGISTER") {
                agent_addr = Some(from);
                send(SipMessage::ok_for_server_bye(&raw), from).await;
            } else if raw.starts_with("INVITE") && Some(from) != agent_addr {
                if by_caller.contains_key(&call_id) {
                    continue;
                } // 重傳
                let Some(agent) = agent_addr else { continue };
                let i = legs.len();
                let leg = Leg {
                    caller_raw: raw.clone(),
                    caller_addr: from,
                    cin: format!("C2026TEST{:06}", i),
                    agent_call_id: format!("sipx-leg-{}@mock", i),
                };
                send(SipMessage::response_for_request(&raw, "100 Trying"), from).await;
                let caller = SipParser::from_user(&raw).unwrap_or_default();
                let cin = send_cin.then_some(leg.cin.as_str());
                send(
                    to_agent("INVITE", &leg.agent_call_id, 1, &caller, addr, agent, cin),
                    agent,
                )
                .await;
                by_caller.insert(call_id, i);
                by_agent.insert(leg.agent_call_id.clone(), i);
                legs.push(leg);
            } else if raw.starts_with("SIP/2.0") && Some(from) == agent_addr {
                let Some(&i) = by_agent.get(&call_id) else {
                    continue;
                };
                if SipParser::cseq_method(&raw).as_deref() != Some("INVITE") {
                    continue;
                }
                let leg = &legs[i];
                let code = SipParser::status_code(&raw).unwrap_or(0);
                let status = match code {
                    180 => "180 Ringing",
                    200 => "200 OK",
                    _ => continue,
                };
                if code == 200 {
                    let caller = SipParser::from_user(&leg.caller_raw).unwrap_or_default();
                    let ack = to_agent(
                        "ACK",
                        &leg.agent_call_id,
                        1,
                        &caller,
                        addr,
                        agent_addr.unwrap(),
                        None,
                    );
                    send(ack, from).await;
                }
                let mut resp =
                    SipMessage::response_with_to_tag(&leg.caller_raw, status, "sipx-b2b");
                if send_cin {
                    resp = with_header(&resp, &format!("P-CIN-CallId: {}", leg.cin));
                }
                send(resp, leg.caller_addr).await;
            } else if raw.starts_with("BYE") && Some(from) != agent_addr {
                send(SipMessage::ok_for_server_bye(&raw), from).await;
                let Some(&i) = by_caller.get(&call_id) else {
                    continue;
                };
                let leg = &legs[i];
                let caller = SipParser::from_user(&leg.caller_raw).unwrap_or_default();
                send(
                    to_agent(
                        "BYE",
                        &leg.agent_call_id,
                        2,
                        &caller,
                        addr,
                        agent_addr.unwrap(),
                        None,
                    ),
                    agent_addr.unwrap(),
                )
                .await;
            }
        }
    });
    addr
}

/// 同時跑座席端與民眾端，回傳配對結果
async fn run_both(
    send_cin: bool,
    caller_number_seq: bool,
) -> sipress_core::correlate::CorrelationReport {
    let server = spawn_sipx(send_cin).await.to_string();
    let logs = std::env::temp_dir()
        .join(format!(
            "sipress-correlation-test-{}-{}",
            std::process::id(),
            send_cin
        ))
        .to_string_lossy()
        .into_owned();
    let agent_cfg = Config {
        server_addr: server.clone(),
        logs_dir: logs.clone(),
        mode: Mode::Agent,
        duration_secs: 6,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    };
    let caller_cfg = Config {
        server_addr: server,
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs,
        caller_number: "0912000000".into(),
        caller_number_seq,
        cps: 10.0,
        max_total_calls: Some(3),
        duration_secs: 4,
        call_duration_secs: 1,
        invite_timeout_secs: 3,
        ..Default::default()
    };

    let agent = AgentEngine::new(agent_cfg);
    let agent_traces = agent.trace_log();
    let agent_run = tokio::spawn(async move { agent.run(None).await });
    tokio::time::sleep(Duration::from_millis(300)).await; // 等座席註冊

    let caller = Engine::new(caller_cfg);
    let caller_traces = caller.trace_log();
    let report = tokio::time::timeout(Duration::from_secs(15), caller.run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();
    assert_eq!(report.calls_answered, 3, "mock 應讓 3 通全部接通");
    tokio::time::timeout(Duration::from_secs(15), agent_run)
        .await
        .expect("座席端應正常結束")
        .unwrap()
        .unwrap();

    correlate(
        &caller_traces.snapshot(),
        &agent_traces.snapshot(),
        CorrelateOptions::default(),
    )
}

#[tokio::test]
async fn pairs_calls_by_cin_call_id_across_b2bua() {
    let r = run_both(true, false).await;
    assert_eq!(r.caller_calls, 3);
    assert_eq!(r.caller_answered, 3);
    assert_eq!(r.matched_by_cin, 3, "{:#?}", r.pairs);
    assert_eq!((r.agent_answered, r.missed, r.rerouted), (3, 0, 0));
    assert!(r.orphans.is_empty());
    assert_eq!(r.caller_cin_pct, 100.0);
    assert_eq!(r.agent_cin_pct, 100.0);
    for p in &r.pairs {
        let offer = &p.offers[0];
        assert_eq!(p.caller.cin_call_id, offer.cin_call_id);
        assert_ne!(
            p.caller.sip_call_id, offer.sip_call_id,
            "B2BUA 兩段的 Call-ID 不同"
        );
        assert_eq!(offer.agent.as_deref(), Some("1001"));
        assert!(
            offer.ended_at_ms.is_some(),
            "民眾端掛斷後座席端應記錄結束時間"
        );
        let d = p.distribution_ms().unwrap();
        assert!((0..1000).contains(&d), "分配延遲 {}ms", d);
        assert!(p.agent_answer_ms().unwrap() >= d);
    }
    assert_eq!(r.distribution.count, 3);
}

#[tokio::test]
async fn falls_back_to_sequential_caller_numbers_without_cin() {
    let r = run_both(false, true).await;
    assert_eq!(r.caller_cin_pct, 0.0);
    assert_eq!(r.matched_by_cin, 0);
    assert_eq!(r.matched_by_number, 3, "{:#?}", r.pairs);
    assert_eq!(r.ambiguous, 0, "每通不同主叫號碼 → 一對一");
    assert_eq!(r.missed, 0);
    let numbers: Vec<&str> = r
        .pairs
        .iter()
        .map(|p| p.caller.caller_number.as_str())
        .collect();
    assert_eq!(numbers, ["0912000000", "0912000001", "0912000002"]);
    for p in &r.pairs {
        assert_eq!(p.offers[0].caller_number, p.caller.caller_number);
    }
}
