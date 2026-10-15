/// 座席行為模擬：響鈴延遲、拒接、不接（CANCEL → 487）、主動掛斷、wrap-up 休息時間
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, AgentBehavior, Config, Mode};
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::FinalReport;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// mock 對座席送出的請求
fn request(method: &str, call_id: &str, cseq: u32, branch: &str, mock: SocketAddr, agent: SocketAddr, to_tag: &str) -> String {
    let body = if method == "INVITE" {
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio 9 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n"
    } else {
        ""
    };
    let content_type = if body.is_empty() { "" } else { "Content-Type: application/sdp\r\n" };
    let to_tag = if to_tag.is_empty() { String::new() } else { format!(";tag={}", to_tag) };
    format!(
        "{method} sip:1001@{agent} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {mock};branch=z9hG4bK-{branch}\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:caller@{mock}>;tag=mocktag\r\n\
         To: <sip:1001@{agent}>{to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} {method}\r\n\
         Contact: <sip:caller@{mock}>\r\n\
         {content_type}Content-Length: {len}\r\n\r\n{body}",
        len = body.len(),
    )
}

/// 收到的訊息（含收到時間）
type Log = Arc<Mutex<Vec<(Instant, String)>>>;

/// 劇本：mock 在座席首次註冊後送出的動作
#[derive(Clone, Copy)]
enum Script {
    /// 一通 INVITE，等待最終回應
    Invite,
    /// INVITE 後 500ms 送 CANCEL
    InviteThenCancel,
    /// INVITE → 收到 200 OK 回 ACK → 收到座席 BYE 回 200 → 300ms 後第二通 INVITE
    AnswerHangupThenSecondCall,
}

async fn spawn_server(script: Script) -> (SocketAddr, Log) {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let log: Log = Arc::default();
    let log_rx = Arc::clone(&log);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut started = false;
        while let Ok((n, agent)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            log_rx.lock().unwrap().push((Instant::now(), raw.clone()));
            if raw.starts_with("REGISTER") {
                let _ = sock.send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), agent).await;
                if std::mem::replace(&mut started, true) { continue; }
                let sock = Arc::clone(&sock);
                tokio::spawn(async move {
                    let _ = sock.send_to(request("INVITE", "call-1", 1, "inv1", addr, agent, "").as_bytes(), agent).await;
                    if let Script::InviteThenCancel = script {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        let _ = sock.send_to(request("CANCEL", "call-1", 1, "inv1", addr, agent, "").as_bytes(), agent).await;
                    }
                });
                continue;
            }
            if !matches!(script, Script::AnswerHangupThenSecondCall) { continue; }
            if raw.starts_with("SIP/2.0 200") && SipParser::cseq_method(&raw).as_deref() == Some("INVITE") {
                let to_tag = SipParser::to_tag(&raw).unwrap_or_default();
                let _ = sock.send_to(request("ACK", "call-1", 1, "ack1", addr, agent, &to_tag).as_bytes(), agent).await;
            } else if raw.starts_with("BYE") {
                let _ = sock.send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), agent).await;
                let sock = Arc::clone(&sock);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = sock.send_to(request("INVITE", "call-2", 1, "inv2", addr, agent, "").as_bytes(), agent).await;
                });
            }
        }
    });
    (addr, log)
}

async fn run(script: Script, behavior: AgentBehavior, duration_secs: u64) -> (FinalReport, Vec<(Instant, String)>) {
    let (server, log) = spawn_server(script).await;
    let logs = std::env::temp_dir().join(format!("sipress-agentbhv-test-{}", std::process::id()));
    let cfg = Config {
        server_addr:    server.to_string(),
        logs_dir:       logs.to_string_lossy().into_owned(),
        mode:           Mode::Agent,
        duration_secs,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(), username: "1001".into(), password: "pw".into(), domain: String::new(),
        }],
        agent_behavior: behavior,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(20), AgentEngine::new(cfg).run(None))
        .await
        .expect("agent engine 應正常結束")
        .unwrap();
    let msgs = log.lock().unwrap().clone();
    (report, msgs)
}

/// 座席對指定 Call-ID 的 INVITE 回應（依收到順序）
fn invite_responses<'a>(msgs: &'a [(Instant, String)], call_id: &str) -> Vec<&'a (Instant, String)> {
    msgs.iter()
        .filter(|(_, m)| m.starts_with("SIP/2.0")
            && SipParser::cseq_method(m).as_deref() == Some("INVITE")
            && SipParser::call_id(m).as_deref() == Some(call_id))
        .collect()
}

fn status(m: &str) -> u16 { SipParser::status_code(m).unwrap_or(0) }

#[tokio::test]
async fn reject_sends_486_after_ring_delay() {
    let behavior = AgentBehavior { reject_pct: 100.0, ring_min_secs: 0.5, ring_max_secs: 0.5, ..Default::default() };
    let (report, msgs) = run(Script::Invite, behavior, 3).await;

    let resp = invite_responses(&msgs, "call-1");
    let codes: Vec<u16> = resp.iter().map(|(_, m)| status(m)).collect();
    assert_eq!(codes, vec![100, 180, 486]);
    let rang = resp[2].0 - resp[1].0;
    assert!(rang >= Duration::from_millis(450), "486 應在響鈴 0.5s 後送出（實際 {:?}）", rang);
    // 486 與 180 帶同一個 To-tag
    assert_eq!(SipParser::to_tag(&resp[1].1), SipParser::to_tag(&resp[2].1));

    assert_eq!(report.calls_initiated, 1);
    assert_eq!(report.calls_answered, 0);
    assert_eq!(report.calls_failed, 1);
    assert_eq!(report.fail_4xx, 1);
}

#[tokio::test]
async fn no_answer_rings_until_cancel_then_487() {
    let behavior = AgentBehavior { no_answer_pct: 100.0, ..Default::default() };
    let (report, msgs) = run(Script::InviteThenCancel, behavior, 3).await;

    let codes: Vec<u16> = invite_responses(&msgs, "call-1").iter().map(|(_, m)| status(m)).collect();
    assert_eq!(codes, vec![100, 180, 487]);
    let cancel_ok = msgs.iter()
        .find(|(_, m)| m.starts_with("SIP/2.0") && SipParser::cseq_method(m).as_deref() == Some("CANCEL"))
        .expect("CANCEL 應有回應");
    assert_eq!(status(&cancel_ok.1), 200);

    assert_eq!(report.calls_answered, 0);
    assert_eq!(report.calls_failed, 1);
}

#[tokio::test]
async fn delayed_answer_agent_hangup_and_wrap_up() {
    let behavior = AgentBehavior {
        ring_min_secs: 0.5, ring_max_secs: 0.5,
        talk_min_secs: 1.0, talk_max_secs: 1.0,
        wrap_up_secs:  5.0,
        ..Default::default()
    };
    let (report, msgs) = run(Script::AnswerHangupThenSecondCall, behavior, 4).await;

    // 第一通：響鈴 0.5s 後接聽
    let resp = invite_responses(&msgs, "call-1");
    let codes: Vec<u16> = resp.iter().map(|(_, m)| status(m)).collect();
    assert_eq!(codes, vec![100, 180, 200]);
    assert!(resp[2].0 - resp[1].0 >= Duration::from_millis(450), "應響鈴 0.5s 後才接聽");

    // 接聽約 1s 後座席主動 BYE
    let bye = msgs.iter().find(|(_, m)| m.starts_with("BYE")).expect("座席應主動掛斷");
    let talk = bye.0 - resp[2].0;
    assert!((Duration::from_millis(900)..Duration::from_millis(2500)).contains(&talk), "通話時長 {:?}", talk);

    // wrap-up 中的第二通直接 486，不響鈴
    let codes2: Vec<u16> = invite_responses(&msgs, "call-2").iter().map(|(_, m)| status(m)).collect();
    assert_eq!(codes2, vec![486]);

    assert_eq!(report.calls_initiated, 2);
    assert_eq!(report.calls_answered, 1);
    assert_eq!(report.calls_completed, 1);
    assert_eq!(report.calls_failed, 1);
}
