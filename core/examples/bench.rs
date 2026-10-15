//! Caller 引擎效能量測（內建 loopback mock UAS）
//!
//! 用法：cargo run --release -p sipress-core --example bench -- <cps|idle|rtp>
//! CPU 時間請搭配 `/usr/bin/time -l`（macOS）或 `/usr/bin/time -v`（Linux）量測。
use sipress_core::config::Config;
use sipress_core::engine::Engine;
use sipress_core::sip::SipMessage;
use std::net::SocketAddr;
use std::time::Instant;
use tokio::net::UdpSocket;

fn header_line<'a>(raw: &'a str, names: &[&str]) -> &'a str {
    raw.lines()
        .find(|l| {
            let lower = l.to_lowercase();
            names.iter().any(|n| lower.starts_with(&format!("{}:", n)))
        })
        .unwrap_or("")
}

/// 立即接聽所有 INVITE、回應所有 BYE 的 mock UAS；RTP 導向一個持續清空的 sink
async fn spawn_uas() -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_port = sink.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while sink.recv(&mut buf).await.is_ok() {}
    });
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
            let raw = String::from_utf8_lossy(&buf[..n]);
            let resp = if raw.starts_with("INVITE") {
                let sdp = format!(
                    "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
                     m=audio {} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n",
                    rtp_port
                );
                format!(
                    "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{};tag=uas\r\n{}\r\n{}\r\n\
                     Contact: <sip:2000@{}>\r\nContent-Type: application/sdp\r\n\
                     Content-Length: {}\r\n\r\n{}",
                    header_line(&raw, &["via", "v"]),
                    header_line(&raw, &["from", "f"]),
                    header_line(&raw, &["to", "t"]),
                    header_line(&raw, &["call-id", "i"]),
                    header_line(&raw, &["cseq"]),
                    addr,
                    sdp.len(),
                    sdp,
                )
            } else if raw.starts_with("BYE") || raw.starts_with("CANCEL") {
                SipMessage::ok_for_server_bye(&raw)
            } else {
                continue;
            };
            let _ = sock.send_to(resp.as_bytes(), peer).await;
        }
    });
    addr
}

/// 產生 30 秒 44.1kHz 16-bit mono PCM WAV（每通電話載入時需重新取樣）
fn write_test_wav(path: &std::path::Path) {
    let rate = 44_100u32;
    let samples: Vec<i16> = (0..rate * 30)
        .map(|i| {
            ((i as f64 * 440.0 * 2.0 * std::f64::consts::PI / rate as f64).sin() * 8000.0) as i16
        })
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, wav).unwrap();
}

#[tokio::main]
async fn main() {
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "cps".into());
    let uas = spawn_uas().await;
    let tmp = std::env::temp_dir().join(format!("sipress-bench-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();

    let base = Config {
        server_addr: uas.to_string(),
        logs_dir: tmp.to_string_lossy().into_owned(),
        callee_fixed: Some("2000".into()),
        max_concurrent_calls: 100_000,
        rtp_base_port: 20000,
        ..Default::default()
    };
    let cfg = match scenario.as_str() {
        // 目標 3000 CPS 持續 3 秒：看實際發話速率
        "cps" => Config {
            cps: 3000.0,
            duration_secs: 3,
            call_duration_secs: 1,
            ..base
        },
        // 300 通已接通、無媒體的閒置通話：看主迴圈本身的 CPU 開銷
        "idle" => Config {
            cps: 300.0,
            max_total_calls: Some(300),
            duration_secs: 10,
            call_duration_secs: 8,
            ..base
        },
        // 200 通帶 RTP + 44.1kHz WAV 的通話
        "rtp" => {
            let wav = tmp.join("tone.wav");
            write_test_wav(&wav);
            Config {
                cps: 100.0,
                max_total_calls: Some(200),
                duration_secs: 10,
                call_duration_secs: 6,
                enable_rtp: true,
                audio_file: Some(wav),
                ..base
            }
        }
        other => panic!("未知情境: {}（cps / idle / rtp）", other),
    };

    let target_secs = cfg.duration_secs as f64;
    let started = Instant::now();
    let report = Engine::new(cfg).run(None).await.unwrap();
    let wall = started.elapsed().as_secs_f64();

    println!("scenario={scenario}");
    println!("wall_secs={wall:.2}");
    println!(
        "initiated={} answered={} completed={} failed={} timeout={}",
        report.calls_initiated,
        report.calls_answered,
        report.calls_completed,
        report.calls_failed,
        report.calls_timeout
    );
    if scenario == "cps" {
        println!(
            "initiate_rate_cps={:.0}",
            report.calls_initiated as f64 / target_secs
        );
    }
    if let Some(sent) = report.rtp_sent {
        println!(
            "rtp_sent={} rtp_recv={}",
            sent,
            report.rtp_recv.unwrap_or(0)
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
