/// REGISTER 認證流程測試：401/407 challenge、錯誤密碼不可無限重送
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::registrar::{register_once, RegisterStatus};
use sipress_core::sip::SipMessage;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;

/// mock registrar 的認證行為
#[derive(Clone, Copy)]
enum Registrar {
    /// 一律回 401（模擬密碼錯誤），每次都換新 nonce
    AlwaysReject,
    /// 回 407，只有帶 Proxy-Authorization 的請求才回 200
    ProxyAuth,
}

fn challenge(raw: &str, code: &str, header: &str, nonce: u32) -> String {
    // 重用 200 OK 建構器鏡射 Via/From/To/Call-ID/CSeq，再換掉狀態列並加上 challenge
    SipMessage::ok_for_server_bye(raw)
        .replacen("SIP/2.0 200 OK", &format!("SIP/2.0 {}", code), 1)
        .replacen(
            "Content-Length: 0",
            &format!("{}: Digest realm=\"test\", nonce=\"n{}\"\r\nContent-Length: 0", header, nonce),
            1,
        )
}

async fn spawn_registrar(mode: Registrar) -> (SocketAddr, Arc<AtomicUsize>) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let registers = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&registers);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut nonce = 0u32;
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if !raw.starts_with("REGISTER") {
                continue;
            }
            counter.fetch_add(1, Ordering::Relaxed);
            nonce += 1;
            let resp = match mode {
                Registrar::AlwaysReject => challenge(&raw, "401 Unauthorized", "WWW-Authenticate", nonce),
                Registrar::ProxyAuth if raw.contains("\r\nProxy-Authorization: Digest ") => {
                    SipMessage::ok_for_server_bye(&raw)
                }
                Registrar::ProxyAuth => {
                    challenge(&raw, "407 Proxy Authentication Required", "Proxy-Authenticate", nonce)
                }
            };
            let _ = sock.send_to(resp.as_bytes(), peer).await;
        }
    });
    (addr, registers)
}

#[tokio::test]
async fn register_once_answers_407_with_proxy_authorization() {
    let (server, _) = spawn_registrar(Registrar::ProxyAuth).await;
    let result = register_once(&server.to_string(), "test", "1001", "pw", 60, "UDP")
        .await
        .unwrap();
    assert!(
        matches!(result.status, RegisterStatus::Registered),
        "407 應以 Proxy-Authorization 回應：{:?}",
        result
    );
}

#[tokio::test]
async fn agent_stops_retrying_register_when_credentials_are_rejected() {
    let (server, registers) = spawn_registrar(Registrar::AlwaysReject).await;
    let logs = std::env::temp_dir().join(format!("sipress-reg-test-{}", std::process::id()));
    let cfg = Config {
        server_addr:    server.to_string(),
        logs_dir:       logs.to_string_lossy().into_owned(),
        mode:           Mode::Agent,
        duration_secs:  1,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username:  "1001".into(),
            password:  "wrong".into(),
            domain:    String::new(),
        }],
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(10), AgentEngine::new(cfg).run(None))
        .await
        .expect("agent engine 應正常結束")
        .unwrap();

    let sent = registers.load(Ordering::Relaxed);
    // 初次 REGISTER + 有限次認證重送 + 結束時的 Expires=0
    assert!(sent <= 5, "密碼錯誤時不應無限重送 REGISTER（實際送出 {} 次）", sent);
}
