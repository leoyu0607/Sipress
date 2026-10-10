/// Caller 引擎端到端流程測試：用 loopback mock UAS 跑完整 INVITE → 200 → ACK → BYE
use sipress_core::config::Config;
use sipress_core::engine::{Engine, ProgressCallback};
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::StatsSnapshot;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// mock UAS 對 INVITE 的行為
#[derive(Clone, Copy)]
enum UasBehavior {
    /// 180 → 200 → 200（重傳）
    AnswerWithRetransmit,
    /// 200 → 立刻由伺服器端送 BYE
    AnswerThenBye,
    /// 只回 200，等待客戶端 BYE
    Answer,
    /// 回 200 後，再對一個不存在的 Call-ID 送 BYE
    AnswerThenByeUnknownDialog,
}

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
        via  = header_line(raw, &["via", "v"]),
        from = header_line(raw, &["from", "f"]),
        to   = header_line(raw, &["to", "t"]),
        cid  = header_line(raw, &["call-id", "i"]),
        cseq = header_line(raw, &["cseq"]),
        uas  = uas_addr,
        len  = sdp.len(),
        sdp  = sdp,
    )
}

fn ringing_for_invite(raw: &str) -> String {
    format!(
        "SIP/2.0 180 Ringing\r\n{}\r\n{}\r\n{};tag=uas\r\n{}\r\n{}\r\nContent-Length: 0\r\n\r\n",
        header_line(raw, &["via", "v"]),
        header_line(raw, &["from", "f"]),
        header_line(raw, &["to", "t"]),
        header_line(raw, &["call-id", "i"]),
        header_line(raw, &["cseq"]),
    )
}

fn server_bye(invite: &str, call_id: &str) -> String {
    let from_tag = SipParser::from_tag(invite).unwrap_or_default();
    format!(
        "BYE sip:1000@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK-srvbye\r\n\
         From: <sip:2000@127.0.0.1>;tag=uas\r\nTo: <sip:1000@127.0.0.1>;tag={}\r\n\
         Call-ID: {}\r\nCSeq: 1 BYE\r\nContent-Length: 0\r\n\r\n",
        from_tag, call_id
    )
}

/// 啟動 mock UAS，回傳其 SIP 位址與它收到的所有 SIP 回應
async fn spawn_uas(behavior: UasBehavior) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let responses: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let responses_rx = Arc::clone(&responses);
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    // 對端 RTP 黑洞（只需一個可送達的 port）
    let rtp_sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_port = rtp_sink.local_addr().unwrap().port();

    tokio::spawn(async move {
        let _keep = rtp_sink;
        let mut buf = vec![0u8; 65536];
        loop {
            let (n, peer) = match sock.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => break,
            };
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if raw.starts_with("SIP/2.0") {
                responses_rx.lock().unwrap().push(raw); // 引擎對伺服器請求的回應
                continue;
            }
            let method = raw.split_whitespace().next().unwrap_or("").to_string();
            match method.as_str() {
                "INVITE" => {
                    let ok = ok_for_invite(&raw, addr, rtp_port);
                    match behavior {
                        UasBehavior::AnswerWithRetransmit => {
                            let _ = sock.send_to(ringing_for_invite(&raw).as_bytes(), peer).await;
                            let _ = sock.send_to(ok.as_bytes(), peer).await;
                            let _ = sock.send_to(ok.as_bytes(), peer).await;
                        }
                        UasBehavior::AnswerThenBye => {
                            let cid = SipParser::call_id(&raw).unwrap_or_default();
                            let _ = sock.send_to(ok.as_bytes(), peer).await;
                            let _ = sock.send_to(server_bye(&raw, &cid).as_bytes(), peer).await;
                        }
                        UasBehavior::Answer => {
                            let _ = sock.send_to(ok.as_bytes(), peer).await;
                        }
                        UasBehavior::AnswerThenByeUnknownDialog => {
                            let _ = sock.send_to(ok.as_bytes(), peer).await;
                            let bye = server_bye(&raw, "no-such-dialog@127.0.0.1");
                            let _ = sock.send_to(bye.as_bytes(), peer).await;
                        }
                    }
                }
                "BYE" | "CANCEL" => {
                    let ok = SipMessage::ok_for_server_bye(&raw);
                    let _ = sock.send_to(ok.as_bytes(), peer).await;
                }
                _ => {} // ACK
            }
        }
    });
    (addr, responses)
}

fn test_config(server: SocketAddr) -> Config {
    // 各測試使用獨立的 log 目錄與 RTP port 區段，避免平行執行時互搶
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let logs = std::env::temp_dir().join(format!("sipress-test-{}-{}", std::process::id(), n));
    Config {
        server_addr:         server.to_string(),
        logs_dir:            logs.to_string_lossy().into_owned(),
        callee_fixed:        Some("2000".into()),
        invite_timeout_secs: 5,
        rtp_base_port:       30000 + n * 2000,
        ..Default::default()
    }
}

#[tokio::test]
async fn retransmitted_200_ok_is_not_double_counted() {
    let (uas, _) = spawn_uas(UasBehavior::AnswerWithRetransmit).await;
    let cfg = Config {
        cps:                20.0,
        max_total_calls:    Some(3),
        call_duration_secs: 1,
        duration_secs:      10,
        enable_rtp:         true,
        ..test_config(uas)
    };
    let report = Engine::new(cfg).run(None).await.unwrap();
    assert_eq!(report.calls_initiated, 3);
    assert_eq!(report.calls_answered, 3, "重傳的 200 OK 不應重複計算接通");
    assert_eq!(report.calls_completed, 3);
    assert!(report.asr <= 100.0, "ASR={}", report.asr);
}

#[tokio::test]
async fn calls_active_at_deadline_are_completed_without_waiting_full_timeout() {
    let (uas, _) = spawn_uas(UasBehavior::Answer).await;
    let cfg = Config {
        cps:                5.0,
        call_duration_secs: 0, // 不主動 BYE → 測試結束時必定仍在通話
        duration_secs:      2,
        ..test_config(uas)
    };
    let started = Instant::now();
    let report = Engine::new(cfg).run(None).await.unwrap();
    let elapsed = started.elapsed();

    assert!(report.calls_answered > 0);
    assert_eq!(
        report.calls_completed, report.calls_answered,
        "結束時送出的 BYE 收到 200 OK 後應計為 completed"
    );
    assert!(report.acd_secs > 0.0, "結束時掛斷的通話應記錄通話時長");
    assert!(
        elapsed < Duration::from_secs(2 + 3),
        "BYE 已全部回應，不應等滿 invite_timeout（實際 {:?}）",
        elapsed
    );
}

#[tokio::test]
async fn unlimited_duration_does_not_panic_and_stops_on_signal() {
    let (uas, _) = spawn_uas(UasBehavior::Answer).await;
    let cfg = Config {
        cps:                5.0,
        call_duration_secs: 0,
        duration_secs:      0, // 不限時
        ..test_config(uas)
    };
    let engine = Engine::new(cfg);
    let stop = engine.stop_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = stop.send(true);
    });
    let report = tokio::time::timeout(Duration::from_secs(10), engine.run(None))
        .await
        .expect("引擎應在收到停止訊號後結束")
        .unwrap();
    assert!(report.calls_initiated > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rtp_session_is_stopped_when_server_bye_arrives_right_after_200_ok() {
    let (uas, _) = spawn_uas(UasBehavior::AnswerThenBye).await;
    let cfg = Config {
        cps:                20.0,
        call_duration_secs: 0,
        duration_secs:      3,
        enable_rtp:         true,
        ..test_config(uas)
    };
    let snaps: Arc<Mutex<Vec<StatsSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let snaps_cb = Arc::clone(&snaps);
    let cb: ProgressCallback = Arc::new(move |s: StatsSnapshot, _p: f64| {
        snaps_cb.lock().unwrap().push(s);
    });
    let report = Engine::new(cfg).run(Some(cb)).await.unwrap();

    assert!(report.calls_completed > 0);
    let snaps = snaps.lock().unwrap();
    let after_calls: Vec<u64> = snaps.iter()
        .filter(|s| s.calls_completed > 0)
        .map(|s| s.rtp_sessions)
        .collect();
    assert!(!after_calls.is_empty());
    assert!(
        after_calls.iter().all(|&n| n == 0),
        "每通電話都已被伺服器掛斷，不應殘留 RTP session：{:?}",
        after_calls
    );
}

#[tokio::test]
async fn bye_for_unknown_dialog_is_still_answered() {
    let (uas, responses) = spawn_uas(UasBehavior::AnswerThenByeUnknownDialog).await;
    let cfg = Config {
        cps:                5.0,
        max_total_calls:    Some(1),
        call_duration_secs: 1,
        duration_secs:      5,
        ..test_config(uas)
    };
    Engine::new(cfg).run(None).await.unwrap();

    let responses = responses.lock().unwrap();
    let answer = responses.iter()
        .find(|r| SipParser::call_id(r).as_deref() == Some("no-such-dialog@127.0.0.1"));
    let answer = answer.expect("對不存在 dialog 的 BYE 也必須回應，否則伺服器會一直重傳");
    assert!(
        answer.starts_with("SIP/2.0 200") || answer.starts_with("SIP/2.0 481"),
        "非預期回應：{}",
        answer.lines().next().unwrap_or("")
    );
    assert_eq!(SipParser::cseq_method(answer).as_deref(), Some("BYE"));
}
