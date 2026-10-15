mod args;
mod tui;

use anyhow::{Context, Result};
use args::Args;
use clap::Parser;
use sipress_core::{
    agent_engine::AgentEngine,
    config::{AgentAccount, AgentBehavior, Config, Mode, Transport},
    engine::{Engine, ProgressCallback},
    html_reporter::HtmlReporter,
    reporter::{OutputFormat, Reporter},
    stats::{FinalReport, StatsSnapshot},
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

    // 解析 transport
    let transport = match args.transport.to_lowercase().as_str() {
        "tcp" => Transport::Tcp,
        _ => Transport::Udp, // 預設 UDP（包含 "udp" 與其他未知值）
    };

    let mode = if args.mode == "agent" {
        Mode::Agent
    } else {
        Mode::Caller
    };
    let agent_accounts = if mode == Mode::Agent {
        load_accounts(&args)?
    } else {
        Vec::new()
    };

    // 建構設定
    let config = Config {
        server_addr: args.server.clone(),
        local_addr: args.local_addr.clone(),
        local_domain: args.local_domain.clone(),
        caller_number: args.caller.clone(),
        callee_prefix: args.callee_prefix.clone(),
        callee_range: args.callee_range,
        callee_fixed: args.callee_fixed.clone(),
        cps: args.cps,
        cps_pattern: Default::default(),
        max_concurrent_calls: args.max_concurrent,
        duration_secs: args.duration,
        call_duration_secs: args.call_duration,
        invite_timeout_secs: args.invite_timeout,
        transport,
        logs_dir: args.logs_dir.clone(),
        rtp_base_port: args.rtp_base_port,
        audio_file: args.audio_file.clone(),
        codec: Default::default(),
        enable_rtp: args.enable_rtp,
        dtmf_sequence: None,
        max_total_calls: if args.max_calls > 0 {
            Some(args.max_calls)
        } else {
            None
        },
        mode,
        agent_accounts,
        agent_ack_timeout_secs: args.ack_timeout,
        agent_bye_timeout_secs: args.bye_timeout,
        agent_behavior: AgentBehavior {
            ring_min_secs: args.ring.0,
            ring_max_secs: args.ring.1,
            reject_pct: args.reject_pct,
            no_answer_pct: args.no_answer_pct,
            talk_min_secs: args.talk.0,
            talk_max_secs: args.talk.1,
            wrap_up_secs: args.wrap_up,
        },
    };
    config.validate()?;

    let engine = AnyEngine::new(config);
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
    if args.tui {
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
        let fmt = OutputFormat::from_str(&args.format).unwrap_or(OutputFormat::Table);
        Reporter::print(&report, fmt);

        if args.html {
            let ts = report_timestamp();
            match HtmlReporter::save(&report, &args.report_dir, &ts, &args.server) {
                Ok(path) => tracing::info!("HTML 報告 → {}", path.display()),
                Err(e) => tracing::warn!("HTML 報告失敗：{}", e),
            }
        }

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

        let report = engine.run(on_progress).await?;
        let fmt = OutputFormat::from_str(&args.format).unwrap_or(OutputFormat::Table);
        Reporter::print(&report, fmt);

        if args.html {
            let ts = report_timestamp();
            match HtmlReporter::save(&report, &args.report_dir, &ts, &args.server) {
                Ok(path) => tracing::info!("HTML 報告 → {}", path.display()),
                Err(e) => tracing::warn!("HTML 報告失敗：{}", e),
            }
        }
    }

    Ok(())
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
