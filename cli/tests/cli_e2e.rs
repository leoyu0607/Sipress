/// CLI 端到端測試：執行編譯好的 sipress 執行檔，對 loopback mock UAS 壓測，
/// 驗證參數解析 → 引擎 → 報告輸出（JSON / CSV / HTML）整條路徑
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// mock UAS 行為
#[derive(Clone, Copy)]
enum Uas {
    /// 180 → 200 OK（含 SDP），BYE / CANCEL 回 200
    Answer,
    /// 什麼都不回（模擬伺服器無回應）
    Silent,
}

fn header(raw: &str, names: &[&str]) -> String {
    raw.lines()
        .find(|l| {
            let lower = l.to_lowercase();
            names.iter().any(|n| lower.starts_with(&format!("{}:", n)))
        })
        .unwrap_or("")
        .to_string()
}

fn response(raw: &str, status: &str, body: Option<&str>) -> String {
    let body = body.unwrap_or("");
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/sdp\r\n" };
    format!(
        "SIP/2.0 {status}\r\n{via}\r\n{from}\r\n{to};tag=uas\r\n{cid}\r\n{cseq}\r\n\
         Contact: <sip:2000@127.0.0.1>\r\n{ctype}Content-Length: {len}\r\n\r\n{body}",
        via  = header(raw, &["via", "v"]),
        from = header(raw, &["from", "f"]),
        to   = header(raw, &["to", "t"]),
        cid  = header(raw, &["call-id", "i"]),
        cseq = header(raw, &["cseq"]),
        len  = body.len(),
    )
}

/// 在背景 thread 啟動 mock UAS，回傳其位址（thread 隨測試程序結束）
fn spawn_uas(behavior: Uas) -> SocketAddr {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = sock.local_addr().unwrap();
    // 對端 RTP 黑洞
    let rtp_sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sdp = format!(
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {} RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n",
        rtp_sink.local_addr().unwrap().port()
    );
    thread::spawn(move || {
        let _keep = rtp_sink;
        let mut buf = vec![0u8; 65536];
        while let Ok((n, peer)) = sock.recv_from(&mut buf) {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            if raw.starts_with("SIP/2.0") || matches!(behavior, Uas::Silent) {
                continue;
            }
            let replies: Vec<String> = match raw.split_whitespace().next().unwrap_or("") {
                "INVITE" => vec![
                    response(&raw, "180 Ringing", None),
                    response(&raw, "200 OK", Some(&sdp)),
                ],
                "BYE" | "CANCEL" => vec![response(&raw, "200 OK", None)],
                _ => vec![],
            };
            for r in replies {
                let _ = sock.send_to(r.as_bytes(), peer);
            }
        }
    });
    addr
}

/// 每個測試獨立的暫存目錄（logs / reports）
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sipress-e2e-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 執行 sipress；超過 `limit` 未結束視為卡住（kill 後 panic）
fn run_sipress(args: &[&str], limit: Duration) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sipress"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("無法執行 sipress");
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "sipress 超過 {:?} 未結束\nstdout:\n{}\nstderr:\n{}",
                limit,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// 對 mock UAS 跑一次壓測的共用參數
fn load_args<'a>(server: &'a str, logs: &'a str, max_calls: &'a str, invite_timeout: &'a str) -> Vec<&'a str> {
    vec![
        "--server", server,
        "--local", "127.0.0.1",
        "--logs-dir", logs,
        "--max-calls", max_calls,
        "--cps", "20",
        "-d", "30",
        "--call-duration", "1",
        "--invite-timeout", invite_timeout,
    ]
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "sipress 結束碼 {:?}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn help_and_version_exit_successfully() {
    let help = run_sipress(&["--help"], Duration::from_secs(10));
    assert_success(&help);
    let text = String::from_utf8_lossy(&help.stdout);
    for flag in ["--server", "--cps", "--max-calls", "--format", "--rtp"] {
        assert!(text.contains(flag), "--help 應列出 {}", flag);
    }

    let ver = run_sipress(&["--version"], Duration::from_secs(10));
    assert_success(&ver);
    assert!(
        String::from_utf8_lossy(&ver.stdout).contains(env!("CARGO_PKG_VERSION")),
        "--version 應顯示 Cargo.toml 的版本"
    );
}

/// 簡易 shell 斷詞：支援雙引號包住含空白的參數
fn shell_words(cmd: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has_word = false;
    for ch in cmd.trim().chars() {
        match ch {
            '"' => { quoted = !quoted; has_word = true; }
            c if c.is_whitespace() && !quoted => {
                if has_word { words.push(std::mem::take(&mut cur)); has_word = false; }
            }
            c => { cur.push(c); has_word = true; }
        }
    }
    if has_word { words.push(cur); }
    words
}

/// GUI 的 CLI 預覽（gui/contract/cli-*.txt，由前端 vitest 產生）必須是 CLI 能解析的合法指令
#[test]
fn gui_cli_preview_commands_are_accepted_by_cli() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../gui/contract");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with("cli-") && name.ends_with(".txt")) {
            continue;
        }
        let words = shell_words(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(words.first().map(String::as_str), Some("./sipress"), "{}: 指令應以 ./sipress 開頭", name);
        // 加上 --help：clap 會先完整解析其餘參數（未知參數 / 錯誤的值會失敗），通過後才印說明並結束
        let mut args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
        args.push("--help");
        let out = run_sipress(&args, Duration::from_secs(10));
        assert!(
            out.status.success(),
            "{} 的參數無法被 CLI 解析：\n{}",
            name,
            String::from_utf8_lossy(&out.stderr)
        );
        checked += 1;
    }
    assert!(checked >= 4, "gui/contract 應有前端產生的 cli-*.txt（目前 {} 個）", checked);
}

#[test]
fn shell_words_handles_quotes() {
    assert_eq!(
        shell_words(r#"./sipress --audio "C:/a b.wav" -c 1"#),
        vec!["./sipress", "--audio", "C:/a b.wav", "-c", "1"]
    );
}

#[test]
fn unknown_argument_is_rejected() {
    let out = run_sipress(&["--no-such-flag"], Duration::from_secs(10));
    assert!(!out.status.success(), "未知參數應以非 0 結束");
}

#[test]
fn json_report_counts_every_answered_call() {
    let server = spawn_uas(Uas::Answer).to_string();
    let dir = temp_dir("json");
    let logs = dir.join("logs").to_string_lossy().into_owned();

    let mut args = load_args(&server, &logs, "5", "2");
    args.extend(["--format", "json"]);
    let out = run_sipress(&args, Duration::from_secs(30));
    assert_success(&out);

    let report: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout 應為 JSON 報告（{}）：\n{}", e, String::from_utf8_lossy(&out.stdout)));
    assert_eq!(report["calls_initiated"], 5);
    assert_eq!(report["calls_answered"], 5);
    assert_eq!(report["calls_completed"], 5);
    assert_eq!(report["calls_failed"], 0);
    assert_eq!(report["calls_timeout"], 0);
    assert_eq!(report["asr"], 100.0);
    assert_eq!(report["ccr"], 100.0);
    // 每通都有 180 → PDD 有樣本；call-duration 1s → ACD 約 1 秒
    assert!(report["pdd_p50_ms"].as_f64().unwrap() > 0.0, "應記錄 PDD");
    let acd = report["acd_secs"].as_f64().unwrap();
    assert!((0.9..2.0).contains(&acd), "ACD 應約 1 秒（實際 {}）", acd);
    // 未啟用 RTP → 無聲音品質指標
    assert!(report["mos"].is_null());
}

#[test]
fn csv_report_has_header_and_matching_row() {
    let server = spawn_uas(Uas::Answer).to_string();
    let dir = temp_dir("csv");
    let logs = dir.join("logs").to_string_lossy().into_owned();

    let mut args = load_args(&server, &logs, "2", "2");
    args.extend(["--format", "csv"]);
    let out = run_sipress(&args, Duration::from_secs(30));
    assert_success(&out);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "CSV 應為 header + 1 列：\n{}", stdout);
    let cols: Vec<&str> = lines[0].split(',').collect();
    let vals: Vec<&str> = lines[1].split(',').collect();
    assert_eq!(cols.len(), vals.len(), "header 與資料欄位數應一致");
    let get = |name: &str| vals[cols.iter().position(|c| *c == name).unwrap()];
    assert_eq!(get("calls_initiated"), "2");
    assert_eq!(get("calls_answered"), "2");
    assert_eq!(get("asr"), "100.00");
}

#[test]
fn unresponsive_server_counts_timeouts() {
    let server = spawn_uas(Uas::Silent).to_string();
    let dir = temp_dir("timeout");
    let logs = dir.join("logs").to_string_lossy().into_owned();

    let mut args = load_args(&server, &logs, "2", "1");
    args.extend(["--format", "json"]);
    let out = run_sipress(&args, Duration::from_secs(30));
    assert_success(&out);

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["calls_initiated"], 2);
    assert_eq!(report["calls_answered"], 0);
    assert_eq!(report["calls_timeout"], 2);
    assert_eq!(report["asr"], 0.0);
}

#[test]
fn html_report_is_written_to_report_dir() {
    let server = spawn_uas(Uas::Answer).to_string();
    let dir = temp_dir("html");
    let logs = dir.join("logs").to_string_lossy().into_owned();
    let reports = dir.join("reports");
    let reports_s = reports.to_string_lossy().into_owned();

    let mut args = load_args(&server, &logs, "1", "2");
    args.extend(["--format", "json", "--html", "--report-dir", &reports_s]);
    let out = run_sipress(&args, Duration::from_secs(30));
    assert_success(&out);

    let html: Vec<PathBuf> = std::fs::read_dir(&reports)
        .expect("應建立 report 目錄")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    assert_eq!(html.len(), 1, "應產生一份 HTML 報告");
    let content = std::fs::read_to_string(&html[0]).unwrap();
    assert!(content.contains("<html") && content.contains("ASR"), "HTML 報告內容不完整");
}
