/// 座席註冊狀態回報：即時快照的 agents_registered / agents_total
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::engine::ProgressCallback;
use sipress_core::sip::SipMessage;
use sipress_core::stats::StatsSnapshot;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// mock registrar：帳號名稱以 "ok" 開頭的回 200，其餘一律 403（不會送 challenge，不會重試）
async fn spawn_registrar() -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if !raw.starts_with("REGISTER") {
                continue;
            }
            let ok = SipMessage::ok_for_server_bye(&raw);
            let resp = if raw.contains("From: <sip:ok") {
                ok
            } else {
                ok.replacen("SIP/2.0 200 OK", "SIP/2.0 403 Forbidden", 1)
            };
            let _ = sock.send_to(resp.as_bytes(), peer).await;
        }
    });
    addr
}

fn account(name: &str) -> AgentAccount {
    AgentAccount {
        extension: name.into(),
        username: name.into(),
        password: "pw".into(),
        domain: String::new(),
    }
}

/// 跑座席引擎並收集每秒的快照
async fn run_agents(accounts: Vec<AgentAccount>) -> Vec<StatsSnapshot> {
    let server = spawn_registrar().await;
    let logs = std::env::temp_dir().join(format!("sipress-agentreg-test-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs: 2,
        agent_accounts: accounts,
        ..Default::default()
    };
    let snaps: Arc<Mutex<Vec<StatsSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let snaps_cb = Arc::clone(&snaps);
    let cb: ProgressCallback = Arc::new(move |s: StatsSnapshot, _p: f64| {
        snaps_cb.lock().unwrap().push(s);
    });
    tokio::time::timeout(Duration::from_secs(15), AgentEngine::new(cfg).run(Some(cb)))
        .await
        .expect("agent engine 應正常結束")
        .unwrap();
    let out = snaps.lock().unwrap().clone();
    assert!(!out.is_empty(), "應至少收到一筆快照");
    out
}

#[tokio::test]
async fn all_accounts_registered_are_reported() {
    let snaps = run_agents(vec![account("ok1001"), account("ok1002")]).await;
    assert!(
        snaps.iter().all(|s| s.agents_total == 2),
        "agents_total 應為帳號數：{:?}",
        snaps
    );
    assert_eq!(
        snaps.iter().map(|s| s.agents_registered).max(),
        Some(2),
        "{:?}",
        snaps
    );
}

#[tokio::test]
async fn rejected_accounts_are_not_counted() {
    let snaps = run_agents(vec![
        account("ok1001"),
        account("bad1002"),
        account("bad1003"),
    ])
    .await;
    assert!(snaps.iter().all(|s| s.agents_total == 3));
    assert!(
        snaps.iter().all(|s| s.agents_registered <= 1),
        "被拒絕的帳號不可計入已註冊：{:?}",
        snaps
    );
    // 用最大值而非最後一筆：最後一筆快照可能落在結束時的解除註冊之後（會是 0）
    assert_eq!(
        snaps.iter().map(|s| s.agents_registered).max(),
        Some(1),
        "{:?}",
        snaps
    );
}
