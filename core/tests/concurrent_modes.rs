/// 同一 process 內同時跑 Engine（民眾端）與 AgentEngine（座席端）：
/// GUI 的雙端模式兩者共用 rtp_base_port（16000），驗證不會互相綁定衝突、統計互不污染
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::engine::{Engine, ProgressCallback};
use sipress_core::sip::SipMessage;
use sipress_core::stats::StatsSnapshot;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

fn header_line(raw: &str, names: &[&str]) -> String {
    raw.lines()
        .find(|l| {
            let lower = l.to_lowercase();
            names.iter().any(|n| lower.starts_with(&format!("{}:", n)))
        })
        .unwrap_or("")
        .to_string()
}

fn ok_for_invite(raw: &str, uas_addr: SocketAddr, rtp_port: u16) -> String {
    let sdp = format!(
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n",
        rtp_port
    );
    format!(
        "SIP/2.0 200 OK\r\n{via}\r\n{from}\r\n{to};tag=uas\r\n{cid}\r\n{cseq}\r\n\
         Contact: <sip:2000@{uas}>\r\nContent-Type: application/sdp\r\n\
         Content-Length: {len}\r\n\r\n{sdp}",
        via = header_line(raw, &["via", "v"]),
        from = header_line(raw, &["from", "f"]),
        to = header_line(raw, &["to", "t"]),
        cid = header_line(raw, &["call-id", "i"]),
        cseq = header_line(raw, &["cseq"]),
        uas = uas_addr,
        len = sdp.len(),
        sdp = sdp,
    )
}

const CALL_ID: &str = "agent-call-1@mock";

/// mock 撥入的對話內請求（INVITE / ACK / CANCEL / BYE）
fn request(method: &str, cseq: u32, mock: SocketAddr, agent: SocketAddr, body: &str) -> String {
    let content_type = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/sdp\r\n"
    };
    format!(
        "{method} sip:1001@{agent} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {mock};branch=z9hG4bK-{branch}\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:caller@{mock}>;tag=mocktag\r\n\
         To: <sip:1001@{agent}>\r\n\
         Call-ID: {CALL_ID}\r\n\
         CSeq: {cseq} {method}\r\n\
         Contact: <sip:caller@{mock}>\r\n\
         {content_type}Content-Length: {len}\r\n\r\n{body}",
        // CANCEL 必須與 INVITE 同 branch；BYE 是新 transaction
        branch = if method == "BYE" { "bye" } else { "inv" },
        len = body.len(),
    )
}

/// 啟動 mock server：回應 REGISTER，首次註冊成功後依序送 INVITE → ACK → CANCEL → BYE
async fn spawn_server() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let rtp_sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_port = rtp_sink.local_addr().unwrap().port();
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_rx = Arc::clone(&received);

    tokio::spawn(async move {
        let _keep = rtp_sink;
        let mut buf = vec![0u8; 65536];
        let mut call_started = false;
        while let Ok((n, agent)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            received_rx.lock().unwrap().push(raw.clone());
            if !raw.starts_with("REGISTER") {
                continue;
            }
            let ok = SipMessage::ok_for_server_bye(&raw);
            let _ = sock.send_to(ok.as_bytes(), agent).await;
            if call_started {
                continue;
            }
            call_started = true;

            let sock = Arc::clone(&sock);
            tokio::spawn(async move {
                let sdp = format!(
                    "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
                     m=audio {} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n",
                    rtp_port
                );
                let steps = [
                    (0, request("INVITE", 1, addr, agent, &sdp)),
                    (200, request("ACK", 1, addr, agent, "")),
                    // 座席已回 200 OK 之後才到的 CANCEL：不可影響通話（RFC 3261 §9.2）
                    (100, request("CANCEL", 1, addr, agent, "")),
                    (1500, request("BYE", 2, addr, agent, "")),
                ];
                for (delay_ms, msg) in steps {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    let _ = sock.send_to(msg.as_bytes(), agent).await;
                }
            });
        }
    });
    (addr, received)
}

/// 簡化版 mock UAS：INVITE 回 200（含 SDP），BYE 回 200
async fn spawn_uas() -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let rtp_sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_port = rtp_sink.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _keep = rtp_sink;
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if raw.starts_with("SIP/2.0") {
                continue;
            }
            match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    let _ = sock
                        .send_to(ok_for_invite(&raw, addr, rtp_port).as_bytes(), peer)
                        .await;
                }
                "BYE" | "CANCEL" => {
                    let _ = sock
                        .send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), peer)
                        .await;
                }
                _ => {}
            }
        }
    });
    addr
}

fn caller_cfg(server: SocketAddr, tag: &str, duration_secs: u64) -> Config {
    let logs = std::env::temp_dir().join(format!("sipress-conc-{}-{}", std::process::id(), tag));
    Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("2000".into()),
        invite_timeout_secs: 5,
        cps: 5.0,
        call_duration_secs: 1,
        duration_secs,
        enable_rtp: true,
        rtp_base_port: 41000, // 與座席端相同
        ..Default::default()
    }
}

fn agent_cfg(server: SocketAddr, tag: &str, duration_secs: u64) -> Config {
    let logs = std::env::temp_dir().join(format!("sipress-conc-{}-{}", std::process::id(), tag));
    Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs,
        enable_rtp: true,
        rtp_base_port: 41000, // 與民眾端相同
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    }
}

fn collect() -> (Arc<Mutex<Vec<StatsSnapshot>>>, ProgressCallback) {
    let snaps: Arc<Mutex<Vec<StatsSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&snaps);
    (
        snaps,
        Arc::new(move |x: StatsSnapshot, _p: f64| s.lock().unwrap().push(x)),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_and_agent_engines_run_concurrently_with_same_rtp_base_port() {
    let uas = spawn_uas().await;
    let (agent_srv, _received) = spawn_server().await;
    let (agent_snaps, agent_cb) = collect();

    let caller =
        tokio::spawn(async move { Engine::new(caller_cfg(uas, "c1-caller", 3)).run(None).await });
    let agent = tokio::spawn(async move {
        AgentEngine::new(agent_cfg(agent_srv, "c1-agent", 4))
            .run(Some(agent_cb))
            .await
    });

    let (c, a) = tokio::time::timeout(Duration::from_secs(20), async {
        (caller.await, agent.await)
    })
    .await
    .expect("兩個引擎都應正常結束");
    let c = c.unwrap().expect("民眾端引擎不應失敗（含 RTP port 綁定）");
    let a = a.unwrap().expect("座席端引擎不應失敗（含 RTP port 綁定）");

    // 民眾端：只看到自己撥出的電話
    assert!(c.calls_initiated > 0);
    assert_eq!(c.calls_failed, 0, "民眾端不應有失敗通話");
    assert_eq!(c.calls_answered, c.calls_initiated);
    assert!(c.rtp_sent.unwrap_or(0) > 0, "民眾端應有 RTP 封包");

    // 座席端：只有 mock 撥入的 1 通，不含民眾端的 INVITE
    assert_eq!(a.calls_answered, 1, "座席端統計不應混入民眾端的通話");
    assert_eq!(a.calls_completed, 1);
    assert_eq!(a.calls_failed, 0);
    assert!(a.rtp_sent.unwrap_or(0) > 0, "座席端應有 RTP 封包");
    assert!(!agent_snaps.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_caller_engine_does_not_stop_agent_engine() {
    let uas = spawn_uas().await;
    let (agent_srv, _received) = spawn_server().await;

    let caller_engine = Engine::new(caller_cfg(uas, "c2a-caller", 0)); // 不限時
    let caller_stop = caller_engine.stop_handle();
    let agent_engine = AgentEngine::new(agent_cfg(agent_srv, "c2a-agent", 5));

    let started = Instant::now();
    let caller = tokio::spawn(async move { caller_engine.run(None).await });
    let agent = tokio::spawn(async move {
        let r = agent_engine.run(None).await;
        (r, started.elapsed())
    });

    tokio::time::sleep(Duration::from_millis(1000)).await;
    let _ = caller_stop.send(true);

    let c = tokio::time::timeout(Duration::from_secs(10), caller)
        .await
        .expect("民眾端應在停止後結束")
        .unwrap()
        .expect("民眾端引擎不應失敗");
    let caller_done_at = started.elapsed();
    assert!(c.calls_initiated > 0);

    let (a, agent_elapsed) = tokio::time::timeout(Duration::from_secs(15), agent)
        .await
        .expect("座席端應照常結束")
        .unwrap();
    let a = a.expect("座席端引擎不應失敗");
    assert!(
        agent_elapsed > caller_done_at + Duration::from_millis(1500),
        "座席端應持續執行到自己的 duration（民眾端於 {:?} 停止，座席端 {:?} 結束）",
        caller_done_at,
        agent_elapsed
    );
    assert_eq!(a.calls_completed, 1, "停止民眾端不應影響座席端的通話");
    assert_eq!(a.calls_failed, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_agent_engine_does_not_stop_caller_engine() {
    let uas = spawn_uas().await;
    let (agent_srv, _received) = spawn_server().await;

    let agent_engine = AgentEngine::new(agent_cfg(agent_srv, "c2b-agent", 0)); // 不限時
    let agent_stop = agent_engine.stop_handle();
    let caller_engine = Engine::new(caller_cfg(uas, "c2b-caller", 5));

    let started = Instant::now();
    let agent = tokio::spawn(async move { agent_engine.run(None).await });
    let caller = tokio::spawn(async move {
        let r = caller_engine.run(None).await;
        (r, started.elapsed())
    });

    tokio::time::sleep(Duration::from_millis(2500)).await; // 座席通話約 1.8s 結束
    let _ = agent_stop.send(true);

    let a = tokio::time::timeout(Duration::from_secs(10), agent)
        .await
        .expect("座席端應在停止後結束")
        .unwrap()
        .expect("座席端引擎不應失敗");
    assert_eq!(a.calls_completed, 1);

    let (c, caller_elapsed) = tokio::time::timeout(Duration::from_secs(15), caller)
        .await
        .expect("民眾端應照常結束")
        .unwrap();
    let c = c.expect("民眾端引擎不應失敗");
    assert!(
        caller_elapsed >= Duration::from_secs(4),
        "民眾端應跑滿自己的 duration（實際 {:?}）",
        caller_elapsed
    );
    assert!(
        c.calls_initiated >= 10,
        "停止座席端後民眾端應持續撥號（initiated={}）",
        c.calls_initiated
    );
    assert_eq!(c.calls_failed, 0);
}
