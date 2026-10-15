/// IPv6：SIP 訊息、SDP 與 RTP 在 IPv6 位址下不被截斷（環境沒有 IPv6 時略過）
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::engine::Engine;
use sipress_core::sip::SipMessage;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// 本機無法綁定 `[::1]` 時（容器常見）回傳 None，測試直接略過
async fn bind_v6() -> Option<UdpSocket> {
    match UdpSocket::bind("[::1]:0").await {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("略過 IPv6 測試：無法綁定 [::1]（{}）", e);
            None
        }
    }
}

fn sdp_v6(port: u16) -> String {
    format!(
        "v=0\r\no=- 1 1 IN IP6 ::1\r\ns=-\r\nc=IN IP6 ::1\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n"
    )
}

#[tokio::test]
async fn caller_calls_ipv6_server_with_ip6_sdp_and_rtp() {
    let Some(sock) = bind_v6().await else { return };
    let Some(rtp) = bind_v6().await else { return };
    let server = sock.local_addr().unwrap();
    let rtp_port = rtp.local_addr().unwrap().port();
    let rtp_count = Arc::new(Mutex::new(0usize));
    let rtp_rx = Arc::clone(&rtp_count);
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = rtp.recv_from(&mut buf).await {
            if n > 12 {
                *rtp_rx.lock().unwrap() += 1;
            }
        }
    });
    let invites: Arc<Mutex<Vec<String>>> = Arc::default();
    let inv_rx = Arc::clone(&invites);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            let replies = match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    inv_rx.lock().unwrap().push(raw.clone());
                    let body = sdp_v6(rtp_port);
                    let ok = SipMessage::response_with_to_tag(&raw, "200 OK", "uas").replace(
                        "Content-Length: 0\r\n\r\n",
                        &format!(
                            "Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        ),
                    );
                    vec![ok]
                }
                "BYE" => vec![SipMessage::ok_for_server_bye(&raw)],
                _ => vec![],
            };
            for r in replies {
                let _ = sock.send_to(r.as_bytes(), peer).await;
            }
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-ipv6-caller-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        local_addr: Some("::1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("2001".into()),
        max_total_calls: Some(1),
        duration_secs: 10,
        call_duration_secs: 1,
        enable_rtp: true,
        rtp_base_port: 43000,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(15), Engine::new(cfg).run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();
    assert_eq!(report.calls_answered, 1, "IPv6 伺服器應可接通");

    let invite = invites.lock().unwrap()[0].clone();
    assert!(
        invite.starts_with(&format!("INVITE sip:2001@{} SIP/2.0", server)),
        "Request-URI 的 IPv6 位址要加中括號且完整：\n{}",
        invite
    );
    assert!(invite.contains("c=IN IP6 ::1\r\n"), "{}", invite);
    assert!(*rtp_count.lock().unwrap() > 0, "RTP 應送到 IPv6 位址");
}

#[tokio::test]
async fn agent_registers_to_ipv6_server() {
    let Some(sock) = bind_v6().await else { return };
    let server = sock.local_addr().unwrap();
    let registers: Arc<Mutex<Vec<String>>> = Arc::default();
    let reg_rx = Arc::clone(&registers);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if raw.starts_with("REGISTER") {
                reg_rx.lock().unwrap().push(raw.clone());
                let _ = sock
                    .send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), peer)
                    .await;
            }
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-ipv6-agent-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs: 2,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(10), AgentEngine::new(cfg).run(None))
        .await
        .expect("座席引擎應正常結束")
        .unwrap();

    let regs = registers.lock().unwrap();
    assert!(!regs.is_empty(), "座席應向 IPv6 伺服器註冊");
    assert!(
        regs[0].contains("[::1]"),
        "REGISTER 中的 IPv6 位址要加中括號：\n{}",
        regs[0]
    );
}
