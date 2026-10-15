/// 座席 REGISTER 遺失時（UDP）重傳：大量帳號同時註冊時伺服器可能漏收，
/// 沒有重傳的話該帳號要等下次刷新（約 2 分鐘）才會上線
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::sip::{SipMessage, SipParser};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

#[tokio::test]
async fn lost_register_is_retransmitted() {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server = sock.local_addr().unwrap();
    // (收到時間, CSeq, Expires 是否為 0)
    let seen: Arc<Mutex<Vec<(Instant, u32, bool)>>> = Arc::default();
    let rx = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut dropped = 0;
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if !raw.starts_with("REGISTER") {
                continue;
            }
            let unregister = SipParser::expires(&raw) == Some(0);
            rx.lock().unwrap().push((
                Instant::now(),
                SipParser::cseq_number(&raw).unwrap_or(0),
                unregister,
            ));
            // 前兩次 REGISTER 當作遺失（不回應）
            if !unregister && dropped < 2 {
                dropped += 1;
                continue;
            }
            let _ = sock
                .send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), peer)
                .await;
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-reg-retx-{}", std::process::id()));
    let started = Instant::now();
    let registered_at: Arc<Mutex<Option<Duration>>> = Arc::default();
    let reg_tx = Arc::clone(&registered_at);
    let cb: sipress_core::engine::ProgressCallback = Arc::new(move |s, _| {
        let mut r = reg_tx.lock().unwrap();
        if s.agents_registered > 0 && r.is_none() {
            *r = Some(started.elapsed());
        }
    });
    let cfg = Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        mode: Mode::Agent,
        duration_secs: 4,
        agent_accounts: vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "pw".into(),
            domain: String::new(),
        }],
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(15), AgentEngine::new(cfg).run(Some(cb)))
        .await
        .expect("座席引擎應正常結束")
        .unwrap();

    let seen = seen.lock().unwrap().clone();
    let regs: Vec<_> = seen.iter().filter(|(_, _, unreg)| !unreg).collect();
    assert_eq!(regs.len(), 3, "第一次 + 兩次重傳：{:?}", regs.len());
    assert!(
        regs.iter().all(|(_, cseq, _)| *cseq == 1),
        "重傳是同一個 transaction（CSeq 不變）"
    );
    // 間隔約 0.5 秒、1 秒（Timer E 加倍）
    let gap = |i: usize| regs[i + 1].0.duration_since(regs[i].0).as_secs_f64();
    assert!(
        (0.4..0.8).contains(&gap(0)),
        "第一次重傳約 0.5 秒後：{}",
        gap(0)
    );
    assert!(
        (0.8..1.4).contains(&gap(1)),
        "第二次重傳約 1 秒後：{}",
        gap(1)
    );
    let t = registered_at.lock().unwrap().expect("重傳後應註冊成功");
    assert!(
        t < Duration::from_secs(3),
        "應在數秒內上線，不必等刷新：{:?}",
        t
    );
}
