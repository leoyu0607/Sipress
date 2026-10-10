/// Agent（座席）引擎端到端流程測試：mock server 先處理 REGISTER，再對座席撥入一通電話
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::engine::ProgressCallback;
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::StatsSnapshot;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

const CALL_ID: &str = "agent-call-1@mock";

/// mock 撥入的對話內請求（INVITE / ACK / CANCEL / BYE）
fn request(method: &str, cseq: u32, mock: SocketAddr, agent: SocketAddr, body: &str) -> String {
    let content_type = if body.is_empty() { "" } else { "Content-Type: application/sdp\r\n" };
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
                    (0,    request("INVITE", 1, addr, agent, &sdp)),
                    (200,  request("ACK",    1, addr, agent, "")),
                    // 座席已回 200 OK 之後才到的 CANCEL：不可影響通話（RFC 3261 §9.2）
                    (100,  request("CANCEL", 1, addr, agent, "")),
                    (1500, request("BYE",    2, addr, agent, "")),
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

#[tokio::test]
async fn agent_call_flow_counts_rtp_and_ignores_late_cancel() {
    let (server, received) = spawn_server().await;
    let logs = std::env::temp_dir().join(format!("sipress-agent-test-{}", std::process::id()));
    let cfg = Config {
        server_addr:    server.to_string(),
        logs_dir:       logs.to_string_lossy().into_owned(),
        mode:           Mode::Agent,
        duration_secs:  4,
        enable_rtp:     true,
        rtp_base_port:  40000,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username:  "1001".into(),
            password:  "pw".into(),
            domain:    String::new(),
        }],
        ..Default::default()
    };

    let snaps: Arc<Mutex<Vec<StatsSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let snaps_cb = Arc::clone(&snaps);
    let cb: ProgressCallback = Arc::new(move |s: StatsSnapshot, _p: f64| {
        snaps_cb.lock().unwrap().push(s);
    });
    let report = tokio::time::timeout(Duration::from_secs(15), AgentEngine::new(cfg).run(Some(cb)))
        .await
        .expect("agent engine 應正常結束")
        .unwrap();

    // ── 第 13 點：接聽後的 CANCEL 不可掛斷通話 ──
    {
        let received = received.lock().unwrap();
        let cancel_resp = received.iter()
            .find(|m| m.starts_with("SIP/2.0") && SipParser::cseq_method(m).as_deref() == Some("CANCEL"))
            .expect("CANCEL 必須有回應");
        assert!(cancel_resp.starts_with("SIP/2.0 200"), "{}", cancel_resp.lines().next().unwrap());
        assert!(
            !received.iter().any(|m| m.starts_with("SIP/2.0 487")),
            "INVITE 已回 200 OK，不可再送 487"
        );
        let bye_resp = received.iter()
            .find(|m| m.starts_with("SIP/2.0") && SipParser::cseq_method(m).as_deref() == Some("BYE"))
            .expect("BYE 必須有回應");
        assert!(bye_resp.starts_with("SIP/2.0 200"));
    }
    assert_eq!(report.calls_failed, 0, "晚到的 CANCEL 不應把通話算成失敗");
    assert_eq!(report.calls_completed, 1, "通話應由 BYE 正常結束");

    // ── 第 14 點：PDD / 建立時間只是本機處理時間，不應記錄假數據 ──
    assert_eq!(report.pdd_max_ms, 0.0, "座席端無法量測 PDD");
    assert_eq!(report.setup_max_ms, 0.0, "座席端無法量測通話建立時間");

    // ── 第 14 / 15 點：RTP session 計數與即時品質只反映進行中的通話 ──
    let snaps = snaps.lock().unwrap();
    let during: Vec<_> = snaps.iter()
        .filter(|s| s.calls_answered == 1 && s.calls_completed == 0)
        .collect();
    let after: Vec<_> = snaps.iter().filter(|s| s.calls_completed == 1).collect();
    assert!(!during.is_empty() && !after.is_empty(), "測試時序不足：{:?}", *snaps);
    assert!(during.iter().all(|s| s.rtp_sessions == 1), "通話中應有 1 個 RTP session");
    assert!(after.iter().all(|s| s.rtp_sessions == 0), "通話結束後 RTP session 應歸零");
    assert!(
        after.iter().all(|s| s.rtp_sent_packets.is_none()),
        "已結束的通話不應再計入即時 RTP 品質"
    );
    assert!(report.rtp_sent.unwrap_or(0) > 0, "最終報告仍須包含已結束通話的 RTP 統計");
}
