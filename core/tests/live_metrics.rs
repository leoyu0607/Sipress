/// 即時指標：CPS 由引擎每秒計算、時間到後收尾期間仍持續回報，直到引擎結束
use sipress_core::config::Config;
use sipress_core::engine::{Engine, ProgressCallback};
use sipress_core::sip::SipMessage;
use sipress_core::stats::StatsSnapshot;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

#[tokio::test]
async fn progress_reports_engine_measured_cps_until_finished() {
    // mock UAS：INVITE 等 300 ms 才回 200（讓即時快照中有還在響鈴的通話），BYE 回 200
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let server = sock.local_addr().unwrap();
    let rx = Arc::clone(&sock);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = rx.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    let s = Arc::clone(&rx);
                    tokio::spawn(async move {
                        let ringing = SipMessage::response_with_to_tag(&raw, "180 Ringing", "uas");
                        let _ = s.send_to(ringing.as_bytes(), peer).await;
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let ok = SipMessage::response_with_to_tag(&raw, "200 OK", "uas");
                        let _ = s.send_to(ok.as_bytes(), peer).await;
                    });
                }
                "BYE" => {
                    let _ = rx
                        .send_to(SipMessage::ok_for_server_bye(&raw).as_bytes(), peer)
                        .await;
                }
                _ => {}
            }
        }
    });

    let snaps: Arc<Mutex<Vec<StatsSnapshot>>> = Arc::default();
    let tx = Arc::clone(&snaps);
    let cb: ProgressCallback = Arc::new(move |s, _| tx.lock().unwrap().push(s));
    let logs = std::env::temp_dir().join(format!("sipress-live-metrics-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("1".into()),
        cps: 20.0,
        duration_secs: 4,
        call_duration_secs: 1,
        invite_timeout_secs: 3,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(20), Engine::new(cfg).run(Some(cb)))
        .await
        .expect("民眾端應正常結束")
        .unwrap();
    // 最後一筆快照在引擎結束後才送出
    tokio::time::sleep(Duration::from_millis(1200)).await;

    let snaps = snaps.lock().unwrap().clone();
    // 第 1–3 秒：每秒約 20 通（不會出現 0 與兩倍交替）
    let mid: Vec<f64> = snaps.iter().skip(1).take(3).map(|s| s.cps).collect();
    assert_eq!(
        mid.len(),
        3,
        "{:?}",
        snaps.iter().map(|s| s.cps).collect::<Vec<_>>()
    );
    for cps in &mid {
        assert!((15.0..=25.0).contains(cps), "每秒 CPS 應接近 20：{:?}", mid);
    }
    // 即時 ASR 不把還在響鈴的通話算進分母：每通都會接通，所以有結果的通話中接通率應接近 100%
    for s in snaps.iter().skip(1).take(3) {
        assert!(s.calls_initiated > s.calls_answered, "應有還在等回應的通話");
        assert!(s.asr > 95.0, "即時 ASR 不應被進行中的通話拉低：{}", s.asr);
    }
    let last = snaps.last().unwrap();
    assert!(last.finished, "時間到後收尾期間仍要回報，直到引擎結束");
    assert_eq!(last.calls_initiated, report.calls_initiated);
    assert!(snaps.iter().any(|s| s.pdd_p50_ms.is_some()));
}

#[tokio::test]
async fn timeline_buckets_add_up_to_the_final_report() {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let server = sock.local_addr().unwrap();
    let rx = Arc::clone(&sock);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut n_invite = 0u32;
        while let Ok((n, peer)) = rx.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            let reply = match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => {
                    n_invite += 1;
                    // 每 4 通拒接一通（503）
                    if n_invite.is_multiple_of(4) {
                        SipMessage::response_with_to_tag(&raw, "503 Service Unavailable", "uas")
                    } else {
                        SipMessage::response_with_to_tag(&raw, "200 OK", "uas")
                    }
                }
                "BYE" => SipMessage::ok_for_server_bye(&raw),
                _ => continue,
            };
            let _ = rx.send_to(reply.as_bytes(), peer).await;
        }
    });

    let logs = std::env::temp_dir().join(format!("sipress-timeline-{}", std::process::id()));
    let cfg = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("1".into()),
        cps: 8.0,
        duration_secs: 4,
        call_duration_secs: 1,
        stats_interval_secs: 1,
        ..Default::default()
    };
    let report = tokio::time::timeout(Duration::from_secs(20), Engine::new(cfg).run(None))
        .await
        .expect("民眾端應正常結束")
        .unwrap();

    let tl = &report.timeline;
    assert!(
        (4..=5).contains(&tl.len()),
        "4 秒、每段 1 秒：{:?}",
        tl.len()
    );
    let sum = |f: fn(&sipress_core::timeline::TimelineBucket) -> u64| tl.iter().map(f).sum::<u64>();
    assert_eq!(sum(|b| b.initiated), report.calls_initiated);
    assert_eq!(sum(|b| b.answered), report.calls_answered);
    assert_eq!(
        sum(|b| b.completed),
        report.calls_completed,
        "收尾期間結束的通話也要算進去"
    );
    assert_eq!(sum(|b| b.failed), report.calls_failed);
    let codes: u64 = tl
        .iter()
        .map(|b| b.fail_codes.get(&503).copied().unwrap_or(0))
        .sum();
    assert_eq!(codes, report.calls_failed);
    for w in tl.windows(2) {
        assert_eq!(w[0].end_secs, w[1].start_secs, "時段要連續");
    }
    // 不分段時為空
    let off = Config {
        server_addr: server.to_string(),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("1".into()),
        max_total_calls: Some(1),
        duration_secs: 5,
        call_duration_secs: 0,
        stats_interval_secs: 0,
        ..Default::default()
    };
    let r = Engine::new(off).run(None).await.unwrap();
    assert!(r.timeline.is_empty());
}
