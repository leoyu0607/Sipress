mod args;
mod tui;

use anyhow::{Context, Result};
use args::Args;
use clap::Parser;
use sipress_core::{
    agent_engine::AgentEngine,
    config::{AgentAccount, AgentBehavior, AgentFlow, Codec, Config, Mode, Transport},
    correlate::{correlate, CorrelateOptions, CorrelationReport, MatchKind},
    correlation_html::CorrelationHtml,
    engine::{Engine, ProgressCallback},
    html_reporter::HtmlReporter,
    reporter::{OutputFormat, Reporter},
    stats::{FinalReport, StatsSnapshot},
    trace::{TraceFile, TraceLog},
};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;
use tui::dashboard::{run_tui, TuiState};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // 初始化 log（RUST_LOG 環境變數優先，否則預設 warn）
    let filter = if args.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()))
        .init();

    // ── 端到端對照（不執行壓測）──
    if let Some(files) = &args.correlate {
        return run_correlate(&args, &files[0], &files[1]);
    }

    let mode = args.mode();
    let agent_accounts = if mode == Mode::Agent {
        load_accounts(&args)?
    } else {
        Vec::new()
    };
    let mut config = build_config(&args, agent_accounts);
    if mode == Mode::Agent {
        config.agent_flow = load_agent_flow(&args)?;
    }
    config.validate()?;

    let engine = AnyEngine::new(config);
    let traces = engine.trace_log();
    // Ctrl-C → graceful stop（座席端會先解除註冊再結束）
    {
        let stop = engine.stop_handle();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                let _ = stop.send(true);
            }
        });
    }

    // ── TUI 模式 ──────────────────────────────────────────────────
    let report = if args.tui {
        let tui_state = Arc::new(Mutex::new(TuiState {
            target_cps: args.cps,
            target_duration: args.duration,
            ..Default::default()
        }));
        let tui_state_cb = Arc::clone(&tui_state);

        let (done_tx, done_rx) = mpsc::channel::<()>(1);

        // on_progress callback → 更新 TUI state
        let on_progress: ProgressCallback = Arc::new(move |snap: StatsSnapshot, progress: f64| {
            let mut st = tui_state_cb.lock().unwrap();
            st.elapsed_secs = progress * st.target_duration as f64;
            st.snapshot = snap;
            st.progress = progress;
        });

        // 啟動引擎
        let engine_stop = engine.stop_handle();
        let engine_handle = {
            let on_progress = Some(on_progress);
            tokio::spawn(async move { engine.run(on_progress).await })
        };

        let stop = engine_stop;
        // 啟動 TUI（阻塞直到 q 或結束）；按 q 離開時一併停止引擎
        run_tui(Arc::clone(&tui_state), done_rx).await?;
        let _ = stop.send(true);

        // 等待引擎完成
        let report = engine_handle.await??;
        let _ = done_tx.send(()).await;

        // TUI 退出後仍然印出最終報告
        report

    // ── 無頭模式 ─────────────────────────────────────────────────
    } else {
        let on_progress: Option<ProgressCallback> = if args.verbose {
            Some(Arc::new(|snap: StatsSnapshot, progress: f64| {
                tracing::info!(
                    "[{:.0}%] 發起={} 接通={} 失敗={} ASR={:.1}%",
                    progress * 100.0,
                    snap.calls_initiated,
                    snap.calls_answered,
                    snap.calls_failed,
                    snap.asr,
                );
            }))
        } else {
            None
        };

        engine.run(on_progress).await?
    };

    let fmt = OutputFormat::from_str(&args.format).unwrap_or(OutputFormat::Table);
    Reporter::print(&report, fmt);

    if args.html {
        let ts = report_timestamp();
        match HtmlReporter::save(&report, &args.report_dir, &ts, &args.server) {
            Ok(path) => tracing::info!("HTML 報告 → {}", path.display()),
            Err(e) => tracing::warn!("HTML 報告失敗：{}", e),
        }
    }

    if let Some(path) = &args.timeline_csv {
        write_timeline_csv(path, &report)?;
    }

    if let Some(path) = &args.trace_file {
        write_trace_file(path, &traces, mode, &args.server)?;
    }

    Ok(())
}

/// --timeline-csv：分時段統計每段一列
fn write_timeline_csv(path: &std::path::Path, report: &FinalReport) -> Result<()> {
    let mut out = String::from(sipress_core::timeline::TimelineBucket::CSV_HEADER);
    out.push('\n');
    for b in &report.timeline {
        out.push_str(&b.csv_row());
        out.push('\n');
    }
    std::fs::write(path, out).with_context(|| format!("無法寫入 {}", path.display()))?;
    tracing::info!(
        "分時段統計（{} 段）→ {}",
        report.timeline.len(),
        path.display()
    );
    Ok(())
}

/// 由 CLI 參數建構壓測設定（座席帳號另由 --accounts 讀入）
fn build_config(args: &Args, agent_accounts: Vec<AgentAccount>) -> Config {
    // 解析 transport
    let transport = match args.transport.to_lowercase().as_str() {
        "tcp" => Transport::Tcp,
        _ => Transport::Udp,
    };

    Config {
        server_addr: args.server.clone(),
        local_addr: args.local_addr.clone(),
        local_domain: args.local_domain.clone(),
        caller_number: args.caller.clone(),
        caller_number_seq: args.caller_number_seq,
        caller_auth_username: args.auth_user.clone(),
        caller_auth_password: args.auth_pass.clone(),
        callee_prefix: args.callee_prefix.clone(),
        callee_range: args.callee_range,
        callee_fixed: args.callee_fixed.clone(),
        cps: args.cps,
        cps_pattern: args.cps_pattern(),
        max_concurrent_calls: args.max_concurrent,
        duration_secs: args.duration,
        call_duration_secs: args.call_duration,
        invite_timeout_secs: args.invite_timeout,
        transport,
        logs_dir: args.logs_dir.clone(),
        rtp_base_port: args.rtp_base_port,
        audio_file: args.audio_file.clone(),
        codec: if args.codec.eq_ignore_ascii_case("pcmu") {
            Codec::Pcmu
        } else {
            Codec::Pcma
        },
        enable_rtp: args.enable_rtp,
        dtmf_sequence: args.dtmf.clone(),
        max_total_calls: if args.max_calls > 0 {
            Some(args.max_calls)
        } else {
            None
        },
        mode: args.mode(),
        agent_accounts,
        agent_ack_timeout_secs: args.ack_timeout,
        agent_bye_timeout_secs: args.bye_timeout,
        agent_reregister_after_call: args.reregister_after_call,
        agent_register_interval_ms: args.register_interval,
        agent_flow: AgentFlow::default(), // 由 --agent-flow 檔案另外讀入
        refill_delay_secs: args.refill_delay,
        agent_behavior: AgentBehavior {
            ring_min_secs: args.ring.0,
            ring_max_secs: args.ring.1,
            reject_pct: args.reject_pct,
            no_answer_pct: args.no_answer_pct,
            talk_min_secs: args.talk.0,
            talk_max_secs: args.talk.1,
            wrap_up_secs: args.wrap_up,
            busy_in_call: args.busy_in_call,
        },
        stats_interval_secs: args.interval,
    }
}

fn write_trace_file(
    path: &std::path::Path,
    traces: &TraceLog,
    side: Mode,
    server: &str,
) -> Result<()> {
    let file = traces.to_file(side, server);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string(&file)?)
        .with_context(|| format!("無法寫入追蹤紀錄 {}", path.display()))?;
    if file.dropped > 0 {
        eprintln!("[sipress] 追蹤紀錄超過上限，{} 通未記錄", file.dropped);
    }
    eprintln!(
        "[sipress] 追蹤紀錄 {} 通 → {}",
        file.traces.len(),
        path.display()
    );
    Ok(())
}

fn read_trace_file(path: &std::path::Path) -> Result<TraceFile> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("無法讀取追蹤紀錄 {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| {
        format!(
            "{} 不是 sipress 追蹤紀錄（--trace-file 的輸出）",
            path.display()
        )
    })
}

/// --correlate：讀兩端追蹤紀錄（順序不拘），輸出合併報告
fn run_correlate(args: &Args, a: &std::path::Path, b: &std::path::Path) -> Result<()> {
    let (fa, fb) = (read_trace_file(a)?, read_trace_file(b)?);
    let (caller, agent) = match (&fa.side, &fb.side) {
        (Mode::Caller, Mode::Agent) => (fa, fb),
        (Mode::Agent, Mode::Caller) => (fb, fa),
        _ => anyhow::bail!(
            "--correlate 需要一份民眾端與一份座席端的追蹤紀錄（兩份都是{}）",
            if fa.side == Mode::Caller {
                "民眾端"
            } else {
                "座席端"
            }
        ),
    };
    let opt = CorrelateOptions {
        window_ms: args.correlate_window * 1000,
        ..Default::default()
    };
    let report = correlate(&caller.traces, &agent.traces, opt);

    match OutputFormat::from_str(&args.format).unwrap_or(OutputFormat::Table) {
        OutputFormat::Json => println!("{}", report.to_json()),
        OutputFormat::Csv => print_correlation_csv(&report),
        OutputFormat::Table => print_correlation_table(&report),
    }
    if args.html {
        let ts = report_timestamp();
        let server = if caller.server.is_empty() {
            &agent.server
        } else {
            &caller.server
        };
        let path = CorrelationHtml::save(&report, &args.report_dir, &ts, server)?;
        eprintln!("[sipress] 端到端對照報告 → {}", path.display());
    }
    Ok(())
}

fn print_correlation_table(r: &CorrelationReport) {
    let opt = |v: Option<f64>| v.map_or("-".to_string(), |m| format!("{:.2}", m));
    println!();
    println!("sipress 端到端對照報告");
    println!("──────────────────────────────────────────────");
    println!(
        "民眾端發起 / 接通    {} / {}",
        r.caller_calls, r.caller_answered
    );
    println!(
        "座席收到 / 接起      {} / {}",
        r.delivered, r.agent_answered
    );
    println!(
        "漏接                 {}（{:.2}%，其中 {} 通沒有座席收到）",
        r.missed, r.miss_rate_pct, r.not_delivered
    );
    println!(
        "ACD 改派             {} 通（多送 {} 個 INVITE）",
        r.rerouted, r.extra_offers
    );
    println!(
        "配對                 P-CIN-CallId {}・主叫號碼 {}・不確定 {}",
        r.matched_by_cin, r.matched_by_number, r.ambiguous
    );
    println!("未配對座席來電       {}", r.orphans.len());
    println!("──────────────────────────────────────────────");
    for (label, d) in [("分配延遲", &r.distribution), ("座席接起", &r.agent_answer)] {
        println!(
            "{}  n={:<6} 平均 {:>7.0} ms  P50 {:>7.0} ms  P95 {:>7.0} ms  MAX {:>7.0} ms",
            label, d.count, d.mean_ms, d.p50_ms, d.p95_ms, d.max_ms
        );
    }
    println!(
        "MOS 座席→民眾 平均 {} 最低 {}；民眾→座席 平均 {} 最低 {}（{} 通）",
        opt(r.mos.caller_avg),
        opt(r.mos.caller_min),
        opt(r.mos.agent_avg),
        opt(r.mos.agent_min),
        r.mos.pairs
    );
    if r.caller_calls > 0 && r.caller_cin_pct < 100.0 {
        println!(
            "注意：民眾端只有 {:.0}% 的通話取得 P-CIN-CallId，其餘以主叫號碼配對",
            r.caller_cin_pct
        );
    }
    if r.ambiguous > 0 {
        println!("注意：{} 通以主叫號碼配對時有多通同號碼候選，配對可能錯誤（民眾端加 --from-seq 可一對一配對）", r.ambiguous);
    }
    println!();
}

fn print_correlation_csv(r: &CorrelationReport) {
    println!("caller_invite_at_ms,caller_number,caller_outcome,cin_call_id,matched_by,ambiguous,offers,answered_by,distribution_ms,agent_answer_ms,caller_mos,agent_mos");
    let o = |v: Option<i64>| v.map_or(String::new(), |x| x.to_string());
    let m = |v: Option<f64>| v.map_or(String::new(), |x| format!("{:.2}", x));
    for p in &r.pairs {
        let answered = p.answered_offer();
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            p.caller.invite_at_ms,
            csv_field(&p.caller.caller_number),
            serde_json::to_value(p.caller.outcome)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
            csv_field(p.caller.cin_call_id.as_deref().unwrap_or("")),
            match p.matched_by {
                Some(MatchKind::CinCallId) => "cin_call_id",
                Some(MatchKind::CallerNumber) => "caller_number",
                None => "",
            },
            p.ambiguous,
            p.offers.len(),
            csv_field(answered.and_then(|a| a.agent.as_deref()).unwrap_or("")),
            o(p.distribution_ms()),
            o(p.agent_answer_ms()),
            m(p.caller.mos),
            m(answered.and_then(|a| a.mos)),
        );
    }
}

/// 含逗號 / 引號時加引號（號碼與 ID 來自網路，不能假設乾淨）
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 民眾端 / 座席端引擎（介面相同：run + stop_handle）
enum AnyEngine {
    Caller(Engine),
    Agent(AgentEngine),
}

impl AnyEngine {
    fn new(config: Config) -> Self {
        match config.mode {
            Mode::Caller => Self::Caller(Engine::new(config)),
            Mode::Agent => Self::Agent(AgentEngine::new(config)),
        }
    }

    fn trace_log(&self) -> Arc<TraceLog> {
        match self {
            Self::Caller(e) => e.trace_log(),
            Self::Agent(e) => e.trace_log(),
        }
    }

    fn stop_handle(&self) -> watch::Sender<bool> {
        match self {
            Self::Caller(e) => e.stop_handle(),
            Self::Agent(e) => e.stop_handle(),
        }
    }

    async fn run(&self, on_progress: Option<ProgressCallback>) -> Result<FinalReport> {
        match self {
            Self::Caller(e) => e.run(on_progress).await,
            Self::Agent(e) => e.run(on_progress).await,
        }
    }
}

/// 讀取 --accounts 指定的座席帳號檔
fn load_accounts(args: &Args) -> Result<Vec<AgentAccount>> {
    let path = args
        .accounts_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("--mode agent 需要以 --accounts 指定座席帳號檔"))?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("無法讀取帳號檔 {}", path.display()))?;
    let accounts = AgentAccount::parse_list(&text, &args.account_domain)
        .with_context(|| format!("帳號檔 {}", path.display()))?;
    if accounts.is_empty() {
        anyhow::bail!("帳號檔 {} 沒有任何帳號", path.display());
    }
    Ok(accounts)
}

/// 讀取 --agent-flow 指定的座席流程檔（未指定 = 沒有流程）
fn load_agent_flow(args: &Args) -> Result<AgentFlow> {
    let Some(path) = args.agent_flow_file.as_ref() else {
        return Ok(AgentFlow::default());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("無法讀取座席流程檔 {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("座席流程檔 {} 格式錯誤", path.display()))
}

/// 取得當前時間字串（用於報告檔名）
fn report_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // 複用 SipLogger 的時間格式
    sipress_core::sip_logger::SipLogger::timestamp_for_filename(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipress_core::config::CpsPattern;

    /// 依空白切開指令，雙引號內視為同一個參數
    fn words(cmd: &str) -> Vec<String> {
        let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
        for ch in cmd.trim().chars() {
            match ch {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// GUI 的 CLI 預覽（gui/contract/cli-*.txt）交給 CLI 解析後，要得到與 GUI 實際送給
    /// start_test 的設定（gui/contract/config-*.json）相同的壓測設定
    #[test]
    fn gui_cli_preview_reproduces_gui_config() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../gui/contract");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let Some(case) = name
                .strip_prefix("cli-")
                .and_then(|n| n.strip_suffix(".txt"))
            else {
                continue;
            };
            let cmd = std::fs::read_to_string(&path).unwrap();
            let args = Args::try_parse_from(words(&cmd))
                .unwrap_or_else(|e| panic!("{}：CLI 無法解析：{}", name, e));
            let mut cli = build_config(&args, Vec::new());
            let mut gui: Config = serde_json::from_str(
                &std::fs::read_to_string(dir.join(format!("config-{}.json", case))).unwrap(),
            )
            .unwrap();

            // 預覽不會、也不需要帶的部分：帳號在 CSV 檔、流程在 JSON 檔、密碼以佔位字代替
            gui.agent_accounts.clear();
            gui.agent_flow = AgentFlow::default();
            assert_eq!(
                cli.caller_auth_password.is_some(),
                gui.caller_auth_password.is_some(),
                "{}",
                name
            );
            cli.caller_auth_password = gui.caller_auth_password.clone();
            // 沒有 RTP 時不使用 RTP port
            if !gui.enable_rtp {
                cli.rtp_base_port = gui.rtp_base_port;
            }
            // 非固定 CPS 模式時 --cps 不使用
            if !matches!(gui.cps_pattern, CpsPattern::Flat) {
                cli.cps = gui.cps;
            }
            if gui.mode == Mode::Caller {
                // 民眾端不使用座席端的設定（GUI 一律帶上，預覽不會出現）
                for c in [&mut cli, &mut gui] {
                    c.agent_behavior = AgentBehavior::default();
                    c.agent_reregister_after_call = false;
                    c.agent_register_interval_ms = 0;
                }
            }
            if gui.mode == Mode::Agent {
                // 座席端不使用民眾端的撥號設定
                for c in [&mut cli, &mut gui] {
                    c.caller_number.clear();
                    c.caller_number_seq = false;
                    c.caller_auth_username = None;
                    c.caller_auth_password = None;
                    c.callee_fixed = None;
                    c.callee_prefix.clear();
                    c.callee_range = 0;
                    c.cps = 0.0;
                    c.cps_pattern = CpsPattern::Flat;
                    c.max_concurrent_calls = 0;
                    c.max_total_calls = None;
                    c.call_duration_secs = 0;
                    c.dtmf_sequence = None;
                }
            }

            let (cli, gui) = (
                serde_json::to_value(&cli).unwrap(),
                serde_json::to_value(&gui).unwrap(),
            );
            let diff: Vec<String> = gui
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| cli.get(k.as_str()) != Some(v))
                .map(|(k, v)| format!("  {}: GUI={} CLI={}", k, v, cli[k.as_str()]))
                .collect();
            assert!(
                diff.is_empty(),
                "{} 的預覽與 GUI 設定不一致：\n{}",
                name,
                diff.join("\n")
            );
            checked += 1;
        }
        assert!(checked >= 5, "gui/contract 應有前端產生的 cli-*.txt");
    }
}
