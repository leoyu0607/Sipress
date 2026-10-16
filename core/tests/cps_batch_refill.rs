/// 民眾端發話節奏：批次模式（每 M 秒一次發 N 通，併發已滿就略過）與補話延遲（線路釋放後 N 秒才補）
use sipress_core::config::{Config, CpsPattern};
use sipress_core::engine::Engine;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

/// mock UAS 收到的 INVITE 時間（重傳的 INVITE 只算一次）
type Invites = Arc<Mutex<Vec<Instant>>>;

fn header_line(raw: &str, name: &str) -> String {
    raw.lines()
        .find(|l| l.to_lowercase().starts_with(&format!("{}:", name)))
        .unwrap_or("")
        .to_string()
}

fn reply(raw: &str, status: &str, uas: SocketAddr) -> String {
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 9 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n";
    let body = if status.starts_with("200") && raw.starts_with("INVITE") {
        sdp
    } else {
        ""
    };
    let ctype = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/sdp\r\n"
    };
    format!(
        "SIP/2.0 {status}\r\n{via}\r\n{from}\r\n{to};tag=uas\r\n{cid}\r\n{cseq}\r\n\
         Contact: <sip:2000@{uas}>\r\n{ctype}Content-Length: {len}\r\n\r\n{body}",
        via = header_line(raw, "via"),
        from = header_line(raw, "from"),
        to = header_line(raw, "to"),
        cid = header_line(raw, "call-id"),
        cseq = header_line(raw, "cseq"),
        len = body.len(),
    )
}

/// 對 INVITE 立刻 200 OK、對 BYE 回 200 OK 的 UAS
async fn spawn_uas() -> (SocketAddr, Invites) {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let invites: Invites = Arc::default();
    let rec = Arc::clone(&invites);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        let mut seen = std::collections::HashSet::new();
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if raw.starts_with("INVITE") {
                if seen.insert(header_line(&raw, "call-id")) {
                    rec.lock().unwrap().push(Instant::now());
                }
                let _ = sock
                    .send_to(reply(&raw, "200 OK", addr).as_bytes(), peer)
                    .await;
            } else if raw.starts_with("BYE") {
                let _ = sock
                    .send_to(reply(&raw, "200 OK", addr).as_bytes(), peer)
                    .await;
            }
        }
    });
    (addr, invites)
}

fn config(server: SocketAddr, tag: &str) -> Config {
    let logs = std::env::temp_dir().join(format!("sipress-pace-{}-{}", std::process::id(), tag));
    Config {
        server_addr: server.to_string(),
        logs_dir: logs.to_string_lossy().into_owned(),
        callee_fixed: Some("2000".into()),
        invite_timeout_secs: 5,
        ..Default::default()
    }
}

async fn run(cfg: Config) {
    tokio::time::timeout(Duration::from_secs(30), Engine::new(cfg).run(None))
        .await
        .expect("引擎應正常結束")
        .unwrap();
}

/// 相對第一通的時間（秒，四捨五入到 0.1）
fn offsets(invites: &Invites) -> Vec<f64> {
    let v = invites.lock().unwrap().clone();
    let t0 = v[0];
    v.iter()
        .map(|t| (t.duration_since(t0).as_secs_f64() * 10.0).round() / 10.0)
        .collect()
}

#[tokio::test]
async fn batch_sends_count_calls_every_period() {
    let (uas, invites) = spawn_uas().await;
    let cfg = Config {
        cps_pattern: CpsPattern::Batch {
            count: 3,
            every_secs: 1.0,
        },
        max_concurrent_calls: 20,
        duration_secs: 2,
        call_duration_secs: 1,
        ..config(uas, "batch")
    };
    run(cfg).await;
    let t = offsets(&invites);
    assert_eq!(t.len(), 6, "0 秒與 1 秒各一批 3 通：{:?}", t);
    assert!(t[..3].iter().all(|x| *x < 0.2), "{:?}", t);
    assert!(t[3..].iter().all(|x| (0.8..1.3).contains(x)), "{:?}", t);
}

#[tokio::test]
async fn batch_skips_calls_that_do_not_fit() {
    let (uas, invites) = spawn_uas().await;
    let cfg = Config {
        cps_pattern: CpsPattern::Batch {
            count: 3,
            every_secs: 1.0,
        },
        max_concurrent_calls: 2,
        duration_secs: 3,
        call_duration_secs: 10,
        ..config(uas, "batchfull")
    };
    run(cfg).await;
    assert_eq!(
        invites.lock().unwrap().len(),
        2,
        "併發上限 2：第一批只發 2 通，之後各批都滿、略過不累積"
    );
}

#[tokio::test]
async fn refill_delay_waits_after_a_line_is_released() {
    let (uas, invites) = spawn_uas().await;
    let cfg = Config {
        cps: 10.0,
        max_concurrent_calls: 1,
        duration_secs: 5,
        call_duration_secs: 1,
        refill_delay_secs: 1.5,
        ..config(uas, "refill")
    };
    run(cfg).await;
    let t = offsets(&invites);
    assert!(t.len() >= 2, "{:?}", t);
    // 第一通 1 秒後掛斷，再等 1.5 秒才補下一通
    let gap = t[1] - t[0];
    assert!((2.3..3.0).contains(&gap), "補話間隔 {} 秒：{:?}", gap, t);
}

#[tokio::test]
async fn without_refill_delay_lines_are_refilled_immediately() {
    let (uas, invites) = spawn_uas().await;
    let cfg = Config {
        cps: 10.0,
        max_concurrent_calls: 1,
        duration_secs: 3,
        call_duration_secs: 1,
        ..config(uas, "norefill")
    };
    run(cfg).await;
    let t = offsets(&invites);
    let gap = t[1] - t[0];
    assert!((0.9..1.4).contains(&gap), "掛斷後應立即補：{:?}", t);
}
