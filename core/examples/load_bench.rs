//! 大量並發的效能基準（Linux；mock SIPX 跑在子行程，量到的 CPU 只有引擎本身）
//!
//! 用法：
//!   cargo run --release -p sipress-core --example load_bench -- <caller|agent> <並發數> [rtp]
//! 例：
//!   load_bench caller 5000 rtp    民眾端維持 5000 通並發、每通送 RTP
//!   load_bench agent 1000         1000 個座席帳號各接一通、不送 RTP
//!   load_bench raw 5000           對照組：只有 5000 個 socket 各自每 20 ms 送一個 RTP 大小的封包
//!                                 （沒有引擎邏輯；與 caller/agent 的 RTP 情境比較即可看出引擎本身的額外開銷）
//!
//! 先爬升到目標並發，再於穩定期量測：
//! - 引擎行程 CPU（/proc/self/stat，100% = 一個核心）、RSS
//! - tokio 排程延遲：一個 10 ms 週期的 task 實際醒來晚了多少（主迴圈 / RTP 計時器被拖慢的指標）
//! - 實際並發、每秒發起數
//! - mock 每秒實際收到的 RTP 封包數（理論值 = 並發數 × 50）
use sipress_core::agent_engine::AgentEngine;
use sipress_core::config::{AgentAccount, Config, Mode};
use sipress_core::engine::{Engine, ProgressCallback};
use sipress_core::sip::{SipMessage, SipParser};
use sipress_core::stats::StatsSnapshot;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

// ─── mock SIPX（子行程） ─────────────────────────────────────────

fn header_line<'a>(raw: &'a str, names: &[&str]) -> &'a str {
    raw.lines()
        .find(|l| {
            let lower = l.to_ascii_lowercase();
            names.iter().any(|n| lower.starts_with(&format!("{}:", n)))
        })
        .unwrap_or("")
}

fn sdp(rtp_port: u16) -> String {
    format!(
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {rtp_port} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n"
    )
}

/// mock 的進入點：stdout 第一行印出 `SIP port RTP sink port`，之後每秒印出 `rtp <封包數>`
async fn run_mock(mode: &str, invite_rate: u32) {
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = sock.local_addr().unwrap();
    let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let rtp_port = sink.local_addr().unwrap().port();
    let rtp_count = Arc::new(AtomicU64::new(0));
    {
        let c = Arc::clone(&rtp_count);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            while sink.recv(&mut buf).await.is_ok() {
                c.fetch_add(1, Ordering::Relaxed);
            }
        });
        let c = Arc::clone(&rtp_count);
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(1));
            t.tick().await;
            loop {
                t.tick().await;
                println!("rtp {}", c.swap(0, Ordering::Relaxed));
            }
        });
    }
    println!("{} {}", addr.port(), rtp_port);

    // 座席模式：註冊成功的座席依序排入佇列，以固定速率打 INVITE 給它們
    let (inv_tx, mut inv_rx) = tokio::sync::mpsc::unbounded_channel::<(SocketAddr, String)>();
    if mode == "agent" {
        let s = Arc::clone(&sock);
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs_f64(1.0 / invite_rate as f64));
            let mut n = 0u64;
            while let Some((agent, ext)) = inv_rx.recv().await {
                t.tick().await;
                n += 1;
                let body = sdp(rtp_port);
                let inv = format!(
                    "INVITE sip:{ext}@{agent} SIP/2.0\r\nVia: SIP/2.0/UDP {addr};branch=z9hG4bK-b{n}\r\n\
                     Max-Forwards: 70\r\nFrom: <sip:0912{n:06}@{addr}>;tag=mock{n}\r\nTo: <sip:{ext}@{agent}>\r\n\
                     Call-ID: bench-{n}@mock\r\nCSeq: 1 INVITE\r\nContact: <sip:mock@{addr}>\r\n\
                     Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.send_to(inv.as_bytes(), agent).await;
            }
        });
    }

    let mut buf = vec![0u8; 65536];
    while let Ok((n, peer)) = sock.recv_from(&mut buf).await {
        let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
        let method = raw.split_whitespace().next().unwrap_or("");
        let reply = match method {
            "INVITE" => {
                let body = sdp(rtp_port);
                Some(format!(
                    "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{};tag=uas\r\n{}\r\n{}\r\n\
                     Contact: <sip:2000@{addr}>\r\nContent-Type: application/sdp\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    header_line(&raw, &["via", "v"]),
                    header_line(&raw, &["from", "f"]),
                    header_line(&raw, &["to", "t"]),
                    header_line(&raw, &["call-id", "i"]),
                    header_line(&raw, &["cseq"]),
                    body.len(),
                ))
            }
            "REGISTER" => {
                let unregister = header_line(&raw, &["expires"]).ends_with(" 0");
                if !unregister && SipParser::cseq_number(&raw) == Some(1) {
                    let ext = SipParser::from_user(&raw).unwrap_or_default();
                    let _ = inv_tx.send((peer, ext));
                }
                Some(SipMessage::ok_for_server_bye(&raw))
            }
            "BYE" | "CANCEL" => Some(SipMessage::ok_for_server_bye(&raw)),
            // 座席接聽的 200 OK → ACK
            "SIP/2.0" if SipParser::cseq_method(&raw).as_deref() == Some("INVITE") => {
                let tag = SipParser::to_tag(&raw).unwrap_or_default();
                Some(format!(
                    "ACK {} SIP/2.0\r\nVia: SIP/2.0/UDP {addr};branch=z9hG4bK-ack{}\r\nMax-Forwards: 70\r\n\
                     {}\r\n{};tag={tag}\r\n{}\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
                    SipParser::contact_uri(&raw).unwrap_or_else(|| format!("sip:agent@{peer}")),
                    rand_branch(),
                    header_line(&raw, &["from", "f"]),
                    header_line(&raw, &["to", "t"]).split(";tag=").next().unwrap_or(""),
                    header_line(&raw, &["call-id", "i"]),
                ))
            }
            _ => None,
        };
        if let Some(r) = reply {
            let _ = sock.send_to(r.as_bytes(), peer).await;
        }
    }
}

fn rand_branch() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

// ─── 對照組：只有計時器 + send_to ───────────────────────────────

fn run_raw(
    n: u64,
    rtp_sink: u16,
    started: Instant,
    window: &Arc<Mutex<Window>>,
    win_from: u64,
    win_to: u64,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let target: SocketAddr = format!("127.0.0.1:{}", rtp_sink).parse().unwrap();
        for i in 0..n {
            tokio::spawn(async move {
                let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                // 錯開起始時間，避免全部擠在同一個 tick
                tokio::time::sleep(Duration::from_micros(i * 20_000 / n.max(1))).await;
                let mut t = tokio::time::interval(Duration::from_millis(20));
                t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let pkt = [0x80u8; 172];
                loop {
                    t.tick().await;
                    let _ = sock.send_to(&pkt, target).await;
                }
            });
        }
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = Instant::now();
            let el = now.duration_since(started).as_secs_f64();
            let mut w = window.lock().unwrap();
            if el >= win_from as f64 && w.cpu_start.is_none() {
                w.cpu_start = cpu_secs();
                w.t_start = Some(now);
            }
            if el >= win_to as f64 {
                w.cpu_end = cpu_secs();
                w.t_end = Some(now);
                w.rss = rss_mb();
                break;
            }
        }
    });
}

fn print_common(w: &mut Window, label: &str, conc: f64) {
    let cpu = match (w.cpu_start, w.cpu_end, w.t_start, w.t_end) {
        (Some(a), Some(b), Some(t0), Some(t1)) => {
            format!(
                "{:.0}%",
                (b - a) / t1.duration_since(t0).as_secs_f64() * 100.0
            )
        }
        _ => "n/a".into(),
    };
    let got = if w.rtp.is_empty() {
        0.0
    } else {
        w.rtp.iter().sum::<f64>() / w.rtp.len() as f64
    };
    println!("情境            {}", label);
    println!("CPU             {}（100% = 一個核心）", cpu);
    println!(
        "RSS             {}",
        w.rss.map_or("n/a".into(), |v| format!("{:.0} MB", v))
    );
    println!("RTP 實收        {:.0} pps（理論 {:.0}）", got, conc * 50.0);
}

// ─── 量測 ───────────────────────────────────────────────────────

/// 行程累計 CPU 秒數（user + system）；非 Linux 時為 None
fn cpu_secs() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // comm 欄位可能含空白，從最後一個 ')' 之後開始數
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some(ticks / 100.0) // CLK_TCK 在 Linux 幾乎都是 100
}

fn rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[derive(Default)]
struct Window {
    cpu_start: Option<f64>,
    cpu_end: Option<f64>,
    t_start: Option<Instant>,
    t_end: Option<Instant>,
    rss: Option<f64>,
    conc: Vec<f64>,
    cps: Vec<f64>,
    lag_ms: Vec<f64>,
    rtp: Vec<f64>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("mock") {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(run_mock(&args[2], args[3].parse().unwrap()));
        return;
    }
    let mode = args.get(1).cloned().unwrap_or_else(|| "caller".into());
    let n: u64 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1000);
    let rtp = args.iter().any(|a| a == "rtp");
    assert!(
        mode == "caller" || mode == "agent" || mode == "raw",
        "模式需為 caller、agent 或 raw"
    );

    // 爬升 20 秒到目標並發，之後穩定 20 秒；量測取穩定期中間 16 秒
    let ramp = 20u64;
    let hold = 20u64;
    let rate = (n as f64 / ramp as f64).ceil() as u32;
    let (win_from, win_to) = (ramp + 2, ramp + hold - 2);

    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args(["mock", &mode, &rate.to_string()])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("無法啟動 mock 子行程");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    let mut ports = first.split_whitespace().map(|p| p.parse::<u16>().unwrap());
    let (port, rtp_sink) = (ports.next().unwrap(), ports.next().unwrap());
    let started = Instant::now();
    let window = Arc::new(Mutex::new(Window::default()));
    let in_window = move |t: Instant| {
        let s = t.duration_since(started).as_secs_f64();
        s >= win_from as f64 && s < win_to as f64
    };
    {
        let w = Arc::clone(&window);
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                if let Some(v) = line
                    .strip_prefix("rtp ")
                    .and_then(|v| v.parse::<f64>().ok())
                {
                    if in_window(Instant::now()) {
                        w.lock().unwrap().rtp.push(v);
                    }
                }
            }
        });
    }

    if mode == "raw" {
        run_raw(n, rtp_sink, started, &window, win_from, win_to);
        let _ = child.kill();
        let _ = child.wait();
        let mut w = window.lock().unwrap();
        print_common(&mut w, &format!("raw × {}", n), n as f64);
        return;
    }

    let tmp = std::env::temp_dir().join(format!("sipress-load-bench-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let base = Config {
        server_addr: format!("127.0.0.1:{}", port),
        local_addr: Some("127.0.0.1".into()),
        logs_dir: tmp.to_string_lossy().into_owned(),
        enable_rtp: rtp,
        rtp_base_port: 20000,
        stats_interval_secs: 0,
        ..Default::default()
    };
    let cfg = if mode == "caller" {
        Config {
            callee_fixed: Some("2000".into()),
            cps: rate as f64,
            max_concurrent_calls: n as usize + 100,
            duration_secs: ramp + hold,
            call_duration_secs: ramp,
            ..base
        }
    } else {
        Config {
            mode: Mode::Agent,
            local_addr: None,
            duration_secs: ramp + hold,
            agent_bye_timeout_secs: 3600,
            agent_accounts: (0..n)
                .map(|i| AgentAccount {
                    extension: format!("{}", 100_000 + i),
                    username: format!("{}", 100_000 + i),
                    password: "pw".into(),
                    domain: String::new(),
                })
                .collect(),
            ..base
        }
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let report = rt.block_on(async {
        // 排程延遲探針：每 10 ms 醒來一次，記錄晚了多少
        {
            let w = Arc::clone(&window);
            tokio::spawn(async move {
                loop {
                    let due = Instant::now() + Duration::from_millis(10);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    let now = Instant::now();
                    if in_window(now) {
                        let lag = now.saturating_duration_since(due).as_secs_f64() * 1000.0;
                        w.lock().unwrap().lag_ms.push(lag);
                    }
                }
            });
        }
        let w = Arc::clone(&window);
        let cb: ProgressCallback = Arc::new(move |s: StatsSnapshot, _| {
            let now = Instant::now();
            let el = now.duration_since(started).as_secs_f64();
            let mut w = w.lock().unwrap();
            if el >= win_from as f64 && w.cpu_start.is_none() {
                w.cpu_start = cpu_secs();
                w.t_start = Some(now);
            }
            if in_window(now) {
                w.conc.push(s.calls_concurrent as f64);
                w.cps.push(s.cps);
            }
            if el >= win_to as f64 && w.cpu_end.is_none() {
                w.cpu_end = cpu_secs();
                w.t_end = Some(now);
                w.rss = rss_mb();
            }
        });
        if mode == "caller" {
            Engine::new(cfg).run(Some(cb)).await.unwrap()
        } else {
            AgentEngine::new(cfg).run(Some(cb)).await.unwrap()
        }
    });
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);

    let mut w = window.lock().unwrap();
    let avg = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let cpu = match (w.cpu_start, w.cpu_end, w.t_start, w.t_end) {
        (Some(a), Some(b), Some(t0), Some(t1)) => {
            format!(
                "{:.0}%",
                (b - a) / t1.duration_since(t0).as_secs_f64() * 100.0
            )
        }
        _ => "n/a".into(),
    };
    let lag_p50 = percentile(&mut w.lag_ms, 0.50);
    let lag_p99 = percentile(&mut w.lag_ms, 0.99);
    let lag_max = w.lag_ms.iter().cloned().fold(0.0, f64::max);
    println!(
        "情境            {} × {}{}",
        mode,
        n,
        if rtp { "（RTP）" } else { "" }
    );
    println!("穩定期平均並發  {:.0}", avg(&w.conc));
    if mode == "caller" {
        println!("穩定期每秒發起  {:.1}（目標 {}）", avg(&w.cps), rate);
    }
    println!("引擎 CPU        {}（100% = 一個核心）", cpu);
    println!(
        "RSS             {}",
        w.rss.map_or("n/a".into(), |v| format!("{:.0} MB", v))
    );
    println!(
        "排程延遲        p50 {:.2} ms / p99 {:.2} ms / max {:.1} ms",
        lag_p50, lag_p99, lag_max
    );
    if rtp {
        let exp = avg(&w.conc) * 50.0;
        let got = avg(&w.rtp);
        println!(
            "RTP 實收        {:.0} pps（理論 {:.0}，{:.1}%）",
            got,
            exp,
            if exp > 0.0 { got / exp * 100.0 } else { 0.0 }
        );
    }
    println!(
        "結果            接通 {} / 發起 {}，失敗 {}，逾時 {}，Setup p95 {:.1} ms",
        report.calls_answered,
        report.calls_initiated,
        report.calls_failed,
        report.calls_timeout,
        report.setup_p95_ms
    );
}
