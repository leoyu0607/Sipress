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
    let ctype = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/sdp\r\n"
    };
    format!(
        "SIP/2.0 {status}\r\n{via}\r\n{from}\r\n{to};tag=uas\r\n{cid}\r\n{cseq}\r\n\
         Contact: <sip:2000@127.0.0.1>\r\n{ctype}Content-Length: {len}\r\n\r\n{body}",
        via = header(raw, &["via", "v"]),
        from = header(raw, &["from", "f"]),
        to = header(raw, &["to", "t"]),
        cid = header(raw, &["call-id", "i"]),
        cseq = header(raw, &["cseq"]),
        len = body.len(),
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
fn load_args<'a>(
    server: &'a str,
    logs: &'a str,
    max_calls: &'a str,
    invite_timeout: &'a str,
) -> Vec<&'a str> {
    vec![
        "--server",
        server,
        "--local",
        "127.0.0.1",
        "--logs-dir",
        logs,
        "--max-calls",
        max_calls,
        "--cps",
        "20",
        "-d",
        "30",
        "--call-duration",
        "1",
        "--invite-timeout",
        invite_timeout,
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
            '"' => {
                quoted = !quoted;
                has_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has_word {
                    words.push(std::mem::take(&mut cur));
                    has_word = false;
                }
            }
            c => {
                cur.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        words.push(cur);
    }
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
        assert_eq!(
            words.first().map(String::as_str),
            Some("./sipress"),
            "{}: 指令應以 ./sipress 開頭",
            name
        );
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
    assert!(
        checked >= 5,
        "gui/contract 應有前端產生的 cli-*.txt（目前 {} 個）",
        checked
    );
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

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout 應為 JSON 報告（{}）：\n{}",
            e,
            String::from_utf8_lossy(&out.stdout)
        )
    });
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
    assert!(
        content.contains("<html") && content.contains("ASR"),
        "HTML 報告內容不完整"
    );

    // 原始數據檔：和 HTML 同一個時間戳記，含 10 秒時間軸與測試開始時間
    let result = html[0]
        .to_string_lossy()
        .replace("_report.html", "_result.json");
    let file = sipress_core::result_file::ResultFile::load(std::path::Path::new(&result))
        .expect("應產生 result.json");
    assert!(file.report.started_at.ends_with('Z'));
    assert!(!file.report.fine_timeline.is_empty());
    assert!(file
        .report
        .fine_timeline
        .iter()
        .all(|b| b.end_secs - b.start_secs <= 10.0 + 1e-9));
    assert_eq!(
        file.report
            .fine_timeline
            .iter()
            .map(|b| b.initiated)
            .sum::<u64>(),
        file.report.calls_initiated
    );

    // 整合報告：讀 result.json 依門檻產生（不執行壓測）
    let th = dir.join("th.json");
    std::fs::write(&th, r#"{"mos": 3.5}"#).unwrap();
    let out_dir = dir.join("integrated");
    let out = run_sipress(
        &[
            "--integrate",
            &result,
            "--thresholds",
            &th.to_string_lossy(),
            "--report-dir",
            &out_dir.to_string_lossy(),
        ],
        Duration::from_secs(10),
    );
    assert_success(&out);
    let html: Vec<PathBuf> = std::fs::read_dir(&out_dir)
        .expect("應建立輸出目錄")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("_integrated_report.html"))
        .collect();
    assert_eq!(html.len(), 1);
    let content = std::fs::read_to_string(&html[0]).unwrap();
    assert!(content.contains("SIP 壓力測試報告") && content.contains("≥ 3.5"));
}

// ── 座席模式（--mode agent）────────────────────────────────────────

/// mock SIPX：REGISTER 一律 200；第一個帳號註冊後對它撥入一通 INVITE → ACK → BYE。
/// 回傳 server 位址與收到的所有訊息（供檢查 REGISTER / 解除註冊）
fn spawn_agent_server() -> (SocketAddr, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::{Arc, Mutex};
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
    let addr = sock.local_addr().unwrap();
    let received: Arc<Mutex<Vec<String>>> = Arc::default();
    let received_rx = Arc::clone(&received);
    thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        let mut call_started = false;
        while let Ok((n, agent)) = sock.recv_from(&mut buf) {
            let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
            received_rx.lock().unwrap().push(raw.clone());
            if !raw.starts_with("REGISTER") {
                continue;
            }
            let _ = sock.send_to(response(&raw, "200 OK", None).as_bytes(), agent);
            if std::mem::replace(&mut call_started, true) {
                continue;
            }
            let sock = Arc::clone(&sock);
            thread::spawn(move || {
                let req = |method: &str, cseq: u32, body: &str| {
                    let ctype = if body.is_empty() {
                        ""
                    } else {
                        "Content-Type: application/sdp\r\n"
                    };
                    format!(
                        "{method} sip:1001@{agent} SIP/2.0\r\n\
                         Via: SIP/2.0/UDP {addr};branch=z9hG4bK-{method}\r\n\
                         Max-Forwards: 70\r\n\
                         From: <sip:caller@{addr}>;tag=mocktag\r\n\
                         To: <sip:1001@{agent}>\r\n\
                         Call-ID: cli-agent-call@mock\r\n\
                         CSeq: {cseq} {method}\r\n\
                         Contact: <sip:caller@{addr}>\r\n\
                         {ctype}Content-Length: {len}\r\n\r\n{body}",
                        len = body.len(),
                    )
                };
                let sdp =
                    "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
                           m=audio 9 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n";
                for (delay_ms, msg) in [
                    (0, req("INVITE", 1, sdp)),
                    (200, req("ACK", 1, "")),
                    (500, req("BYE", 2, "")),
                ] {
                    thread::sleep(Duration::from_millis(delay_ms));
                    let _ = sock.send_to(msg.as_bytes(), agent);
                }
            });
        }
    });
    (addr, received)
}

#[test]
fn agent_mode_registers_answers_and_deregisters() {
    let (server, received) = spawn_agent_server();
    let server = server.to_string();
    let dir = temp_dir("agent");
    let logs = dir.join("logs").to_string_lossy().into_owned();
    let accounts = dir.join("accounts.csv");
    std::fs::write(
        &accounts,
        "extension,username,password\n1001,1001,pw\n1002,1002,pw\n",
    )
    .unwrap();
    let accounts_s = accounts.to_string_lossy().into_owned();

    let out = run_sipress(
        &[
            "--mode",
            "agent",
            "--accounts",
            &accounts_s,
            "--server",
            &server,
            "--logs-dir",
            &logs,
            "-d",
            "3",
            "--format",
            "json",
        ],
        Duration::from_secs(30),
    );
    assert_success(&out);

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout 應為 JSON 報告（{}）：\n{}",
            e,
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(report["calls_answered"], 1);
    assert_eq!(report["calls_completed"], 1);
    assert_eq!(report["calls_failed"], 0);

    let received = received.lock().unwrap();
    for ext in ["1001", "1002"] {
        let regs: Vec<_> = received
            .iter()
            .filter(|m| m.starts_with("REGISTER") && m.contains(&format!("<sip:{}@", ext)))
            .collect();
        assert!(!regs.is_empty(), "帳號 {} 應送出 REGISTER", ext);
        assert!(
            regs.iter().any(|m| m.to_lowercase().contains("expires: 0")),
            "帳號 {} 結束時應解除註冊（Expires: 0）",
            ext
        );
    }
}

#[test]
fn agent_mode_requires_valid_accounts_file() {
    let out = run_sipress(&["--mode", "agent", "-d", "1"], Duration::from_secs(10));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--accounts"),
        "應提示需要 --accounts"
    );

    let dir = temp_dir("agent-bad");
    let bad = dir.join("bad.txt");
    std::fs::write(&bad, "1001\n").unwrap();
    let out = run_sipress(
        &[
            "--mode",
            "agent",
            "--accounts",
            &bad.to_string_lossy(),
            "-d",
            "1",
        ],
        Duration::from_secs(10),
    );
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("第 1 行"),
        "應指出錯誤的行號"
    );

    let out = run_sipress(&["--mode", "pbx"], Duration::from_secs(10));
    assert!(!out.status.success(), "未知模式應被拒絕");
}

// ── 端到端對照（--trace-file / --correlate）────────────────────────

#[test]
fn trace_file_records_each_call_with_sequential_caller_numbers() {
    let server = spawn_uas(Uas::Answer).to_string();
    let dir = temp_dir("trace");
    let logs = dir.join("logs").to_string_lossy().into_owned();
    let trace = dir.join("out").join("caller.json");
    let trace_s = trace.to_string_lossy().into_owned();

    let mut args = load_args(&server, &logs, "3", "2");
    args.extend([
        "--format",
        "json",
        "--from",
        "0912000098",
        "--from-seq",
        "--trace-file",
        &trace_s,
    ]);
    let out = run_sipress(&args, Duration::from_secs(30));
    assert_success(&out);

    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&trace).unwrap()).unwrap();
    assert_eq!(file["side"], "caller");
    let traces = file["traces"].as_array().unwrap();
    let numbers: Vec<&str> = traces
        .iter()
        .map(|t| t["caller_number"].as_str().unwrap())
        .collect();
    assert_eq!(numbers, ["0912000098", "0912000099", "0912000100"]);
    for t in traces {
        assert_eq!(t["outcome"], "answered");
        assert_eq!(t["response_code"], 200);
        assert!(t["answered_at_ms"].as_u64().unwrap() >= t["invite_at_ms"].as_u64().unwrap());
        assert!(t["ended_at_ms"].is_u64(), "BYE 完成後應記錄結束時間");
    }
}

/// 手寫一份追蹤紀錄
fn write_trace(path: &std::path::Path, side: &str, traces: serde_json::Value) {
    let file = serde_json::json!({ "side": side, "server": "10.0.0.1:5060", "traces": traces });
    std::fs::write(path, file.to_string()).unwrap();
}

fn trace(
    id: &str,
    cin: Option<&str>,
    number: &str,
    agent: Option<&str>,
    at: u64,
    answered: bool,
) -> serde_json::Value {
    serde_json::json!({
        "sip_call_id": id, "cin_call_id": cin, "caller_number": number, "agent": agent,
        "invite_at_ms": at, "answered_at_ms": if answered { Some(at + 300) } else { None },
        "ended_at_ms": null, "outcome": if answered { "answered" } else { "failed" },
        "response_code": if answered { 200 } else { 486 }, "mos": null, "loss_pct": null, "jitter_ms": null,
    })
}

#[test]
fn correlate_merges_trace_files_into_json_and_html() {
    let dir = temp_dir("correlate");
    let caller = dir.join("caller.json");
    let agent = dir.join("agent.json");
    write_trace(
        &caller,
        "caller",
        serde_json::json!([
            trace("c1", Some("C1"), "0912000000", None, 10_000, true),
            trace("c2", None, "0912000001", None, 10_100, true),
            trace("c3", Some("C3"), "0912000002", None, 10_200, true),
        ]),
    );
    write_trace(
        &agent,
        "agent",
        serde_json::json!([
            trace("a1", Some("C1"), "0912000000", Some("1001"), 10_400, true),
            trace("a2", None, "0912000001", Some("1002"), 10_900, true),
        ]),
    );
    let reports = dir.join("reports").to_string_lossy().into_owned();
    let (c, a) = (
        caller.to_string_lossy().into_owned(),
        agent.to_string_lossy().into_owned(),
    );

    // 檔案順序不拘
    let out = run_sipress(
        &[
            "--correlate",
            &a,
            &c,
            "--format",
            "json",
            "--html",
            "--report-dir",
            &reports,
        ],
        Duration::from_secs(10),
    );
    assert_success(&out);
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "應輸出 JSON（{}）：\n{}",
            e,
            String::from_utf8_lossy(&out.stdout)
        )
    });
    assert_eq!(r["caller_calls"], 3);
    assert_eq!(r["matched_by_cin"], 1);
    assert_eq!(r["matched_by_number"], 1);
    assert_eq!(r["missed"], 1);
    assert_eq!(r["distribution"]["max_ms"], 800.0);

    let html: Vec<PathBuf> = std::fs::read_dir(&reports)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with("_e2e_report.html"))
        .collect();
    assert_eq!(html.len(), 1);
    assert!(std::fs::read_to_string(&html[0])
        .unwrap()
        .contains("漏接通話（1）"));

    let table = run_sipress(&["--correlate", &c, &a], Duration::from_secs(10));
    assert_success(&table);
    assert!(String::from_utf8_lossy(&table.stdout).contains("漏接"));

    let csv = run_sipress(
        &["--correlate", &c, &a, "--format", "csv"],
        Duration::from_secs(10),
    );
    assert_success(&csv);
    assert_eq!(
        String::from_utf8_lossy(&csv.stdout).lines().count(),
        4,
        "header + 3 通"
    );

    let same = run_sipress(&["--correlate", &c, &c], Duration::from_secs(10));
    assert!(!same.status.success(), "兩份都是民眾端應失敗");
    assert!(String::from_utf8_lossy(&same.stderr).contains("一份民眾端與一份座席端"));
}
