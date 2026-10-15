/// Tauri commands：前端 ↔ core 橋接層
use sipress_core::{
    agent_engine::AgentEngine,
    config::{Config, Mode},
    correlate::{correlate, CorrelateOptions, CorrelationReport},
    correlation_html::CorrelationHtml,
    engine::{Engine, ProgressCallback},
    html_reporter::HtmlReporter,
    registrar::{register_once, RegisterResult},
    rtp::audio::AudioSource,
    stats::{FinalReport, StatsSnapshot},
    trace::TraceLog,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::State;
use tokio::sync::mpsc;

// ── 全域狀態 ─────────────────────────────────────────────────────

/// 單一模式（民眾端或座席端）的執行狀態
#[derive(Default)]
pub struct Session {
    pub snapshot: Mutex<Option<StatsSnapshot>>,
    pub report: Mutex<Option<FinalReport>>,
    /// 引擎執行失敗的原因（get_report 會回傳此錯誤給前端）
    pub error: Mutex<Option<String>>,
    pub stop_tx: Mutex<Option<mpsc::Sender<()>>>,
    /// 此模式是否有壓測正在執行（含停止後的收尾階段），防止重複啟動
    pub running: AtomicBool,
    /// 最近一次執行的每通追蹤紀錄（端到端對照用；執行中即時更新，下次啟動時換新）
    pub traces: Mutex<Option<Arc<TraceLog>>>,
}

impl Session {
    /// 嘗試佔用執行權；此模式已有壓測在跑時回傳 false
    fn try_begin(&self) -> bool {
        self.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// 引擎結束：先存結果，再標記 snapshot 已結束，最後釋放執行權
    /// （前端看到 finished 後才取報告，因此結果必須先就緒）
    fn finish(&self, result: Result<FinalReport, String>) {
        match result {
            Ok(report) => *self.report.lock().unwrap() = Some(report),
            Err(e) => {
                tracing::error!("Engine 錯誤: {}", e);
                *self.error.lock().unwrap() = Some(e);
            }
        }
        {
            // 進度 task 可能在引擎收尾前就已退出，這裡保證前端一定看得到 finished
            let mut snap = self.snapshot.lock().unwrap();
            match snap.as_mut() {
                Some(s) => s.finished = true,
                None => {
                    *snap = Some(StatsSnapshot {
                        finished: true,
                        ..Default::default()
                    })
                }
            }
        }
        *self.stop_tx.lock().unwrap() = None;
        self.running.store(false, Ordering::SeqCst);
    }
}

/// 跨 command 共享的執行狀態：民眾端與座席端各自獨立，可在同一台電腦同時執行
#[derive(Default)]
pub struct AppState {
    pub caller: Arc<Session>,
    pub agent: Arc<Session>,
}

impl AppState {
    fn session(&self, mode: &Mode) -> Arc<Session> {
        match mode {
            Mode::Caller => Arc::clone(&self.caller),
            Mode::Agent => Arc::clone(&self.agent),
        }
    }
}

// ── Tauri Commands ────────────────────────────────────────────────

/// 啟動壓測
/// 前端呼叫：await invoke('start_test', { config: {...} })
#[tauri::command]
pub async fn start_test(state: State<'_, Arc<AppState>>, config: Config) -> Result<String, String> {
    config.validate().map_err(|e| e.to_string())?;
    // 音檔在開始前驗證：格式不支援時直接回報，而不是引擎改送靜音
    if let (true, Some(path)) = (config.enable_rtp, config.audio_file.as_deref()) {
        AudioSource::preload(path)
            .await
            .map_err(|e| format!("{:#}", e))?;
    }
    let state = state.session(&config.mode);
    if !state.try_begin() {
        let side = if config.mode == Mode::Agent {
            "座席端"
        } else {
            "民眾端"
        };
        return Err(format!("{}已有壓測正在執行，請先停止或等待結束", side));
    }

    // 清除舊結果
    *state.snapshot.lock().unwrap() = None;
    *state.report.lock().unwrap() = None;
    *state.error.lock().unwrap() = None;
    *state.traces.lock().unwrap() = None;

    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    *state.stop_tx.lock().unwrap() = Some(stop_tx);

    let state_cb = Arc::clone(&state);
    let on_progress: ProgressCallback = Arc::new(move |snap: StatsSnapshot, _progress: f64| {
        *state_cb.snapshot.lock().unwrap() = Some(snap);
    });

    let mode = config.mode.clone();
    let state_t = Arc::clone(&state);
    // 引擎放在獨立 task 執行：即使 panic 也能由 JoinError 接住並釋放執行權
    let run = tokio::spawn(async move {
        match mode {
            Mode::Caller => {
                let engine = Engine::new(config);
                *state_t.traces.lock().unwrap() = Some(engine.trace_log());
                let stop_handle = engine.stop_handle();
                let forward = tokio::spawn(async move {
                    if stop_rx.recv().await.is_some() {
                        tracing::info!("壓測收到停止訊號 → 觸發 graceful BYE/CANCEL");
                        let _ = stop_handle.send(true);
                    }
                });
                let r = engine.run(Some(on_progress)).await;
                forward.abort();
                r
            }
            Mode::Agent => {
                let engine = AgentEngine::new(config);
                *state_t.traces.lock().unwrap() = Some(engine.trace_log());
                let stop_handle = engine.stop_handle();
                // 將前端的 stop 訊號轉發成 watch::Sender(true)
                // 讓 AgentEngine 可以走完 graceful stop（REGISTER Expires=0 + drain）
                let forward = tokio::spawn(async move {
                    if stop_rx.recv().await.is_some() {
                        tracing::info!("座席壓測收到停止訊號 → 觸發 graceful deregister");
                        let _ = stop_handle.send(true);
                    }
                });
                let r = engine.run(Some(on_progress)).await;
                forward.abort();
                r
            }
        }
    });
    tokio::spawn(async move {
        let result = match run.await {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(e) => Err(format!("壓測引擎異常終止: {}", e)),
        };
        state.finish(result);
    });

    Ok("started".to_string())
}

/// 手動停止指定模式的壓測
#[tauri::command]
pub async fn stop_test(state: State<'_, Arc<AppState>>, mode: Mode) -> Result<String, String> {
    // Take the sender out BEFORE any await so MutexGuard is not held across threads
    let tx = state.session(&mode).stop_tx.lock().unwrap().take();
    if let Some(tx) = tx {
        let _ = tx.send(()).await;
        Ok("stopped".to_string())
    } else {
        Err("沒有正在執行的壓測".to_string())
    }
}

/// 取得即時快照（前端每秒輪詢）
#[tauri::command]
pub fn get_snapshot(state: State<'_, Arc<AppState>>, mode: Mode) -> Option<StatsSnapshot> {
    state.session(&mode).snapshot.lock().unwrap().clone()
}

/// 取得最終報告（壓測完成後）；引擎執行失敗時回傳錯誤原因
#[tauri::command]
pub fn get_report(
    state: State<'_, Arc<AppState>>,
    mode: Mode,
) -> Result<Option<FinalReport>, String> {
    let session = state.session(&mode);
    if let Some(e) = session.error.lock().unwrap().clone() {
        return Err(e);
    }
    let report = session.report.lock().unwrap().clone();
    Ok(report)
}

/// 對單一帳號發起 SIP REGISTER（含 Digest 認證重送），回傳結果
/// 前端在新增帳號後立即呼叫
#[tauri::command]
pub async fn register_account(
    server: String,
    domain: Option<String>,
    username: String,
    password: String,
    expires: Option<u32>,
    transport: Option<String>,
) -> Result<RegisterResult, String> {
    let dom_owned = domain.filter(|s| !s.is_empty()).unwrap_or_else(|| {
        // 沒指定 domain → 取 server 的主機部分（IPv6 加中括號，供 SIP URI 使用）
        sipress_core::net::uri_host(&server)
    });
    let tp = transport.unwrap_or_else(|| "UDP".into());
    let exp = expires.unwrap_or(3600);

    register_once(&server, &dom_owned, &username, &password, exp, &tp)
        .await
        .map_err(|e| e.to_string())
}

/// 產生 HTML 報告字串（前端負責下載或開啟）
#[tauri::command]
pub fn get_html_report(
    state: State<'_, Arc<AppState>>,
    mode: Mode,
    server_addr: String,
    timestamp: String,
) -> Result<String, String> {
    let report = state
        .session(&mode)
        .report
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "尚無測試結果，請先完成或停止一次壓測".to_string())?;
    Ok(HtmlReporter::render(&report, &timestamp, &server_addr))
}

/// 將文字內容寫入使用者在「另存新檔」對話框選定的路徑
/// 用兩端最近一次執行的追蹤紀錄做端到端對照
fn correlation_report(app: &AppState) -> Result<CorrelationReport, String> {
    let traces = |s: &Session| s.traces.lock().unwrap().clone();
    let (Some(caller), Some(agent)) = (traces(&app.caller), traces(&app.agent)) else {
        return Err("需要民眾端與座席端都執行過（例如雙端啟動）才能產生端到端對照報告".into());
    };
    Ok(correlate(
        &caller.snapshot(),
        &agent.snapshot(),
        CorrelateOptions::default(),
    ))
}

/// 端到端對照的 HTML 報告
#[tauri::command]
pub fn get_correlation_html(
    state: State<'_, Arc<AppState>>,
    server_addr: String,
    timestamp: String,
) -> Result<String, String> {
    let report = correlation_report(&state)?;
    Ok(CorrelationHtml::render(&report, &timestamp, &server_addr))
}

#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("寫入 {} 失敗: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> FinalReport {
        serde_json::from_value(serde_json::json!({
            "calls_initiated": 1, "calls_answered": 1, "calls_completed": 1,
            "calls_failed": 0, "calls_timeout": 0, "duration_secs": 1.0,
            "asr": 100.0, "ccr": 100.0, "actual_cps": 1.0, "acd_secs": 1.0,
            "pdd_p50_ms": 0.0, "pdd_p95_ms": 0.0, "pdd_p99_ms": 0.0, "pdd_max_ms": 0.0,
            "setup_p50_ms": 0.0, "setup_p95_ms": 0.0, "setup_p99_ms": 0.0, "setup_max_ms": 0.0,
            "fail_4xx": 0, "fail_5xx": 0, "fail_6xx": 0,
            "mos": null, "loss_rate_pct": null, "jitter_ms": null,
            "rtp_sent": null, "rtp_recv": null, "rtp_out_of_order": null,
            "fail_codes": {}
        }))
        .unwrap()
    }

    #[test]
    fn second_start_is_rejected_while_running() {
        let state = Session::default();
        assert!(state.try_begin());
        assert!(!state.try_begin(), "執行中不可再次啟動");
        state.finish(Ok(sample_report()));
        assert!(state.try_begin(), "結束後應可再次啟動");
    }

    #[test]
    fn engine_error_is_kept_and_snapshot_marked_finished() {
        let state = Session::default();
        assert!(state.try_begin());
        state.finish(Err("無效的 server 位址".into()));

        assert_eq!(
            state.error.lock().unwrap().as_deref(),
            Some("無效的 server 位址")
        );
        assert!(state.report.lock().unwrap().is_none());
        let snap = state
            .snapshot
            .lock()
            .unwrap()
            .clone()
            .expect("應有 snapshot");
        assert!(
            snap.finished,
            "前端靠 finished 偵測結束，失敗時也必須設為 true"
        );
        assert!(!state.running.load(Ordering::SeqCst));
    }

    #[test]
    fn finish_marks_existing_snapshot_finished() {
        let state = Session::default();
        assert!(state.try_begin());
        *state.snapshot.lock().unwrap() = Some(StatsSnapshot {
            calls_initiated: 7,
            ..Default::default()
        });
        state.finish(Ok(sample_report()));

        let snap = state.snapshot.lock().unwrap().clone().unwrap();
        assert!(snap.finished);
        assert_eq!(snap.calls_initiated, 7, "既有的即時數據應保留");
        assert!(state.report.lock().unwrap().is_some());
    }

    #[test]
    fn caller_and_agent_run_independently() {
        let app = AppState::default();
        assert!(app.session(&Mode::Caller).try_begin());
        assert!(
            app.session(&Mode::Agent).try_begin(),
            "民眾端執行中，座席端仍應可啟動"
        );
        assert!(!app.session(&Mode::Agent).try_begin());

        app.session(&Mode::Agent).finish(Ok(sample_report()));
        assert!(
            app.caller.running.load(Ordering::SeqCst),
            "座席端結束不應影響民眾端"
        );
        assert!(app.caller.report.lock().unwrap().is_none());
        assert!(app.agent.report.lock().unwrap().is_some());
        assert!(app.caller.snapshot.lock().unwrap().is_none());
    }

    // ── 雙端並行（民眾端 / 座席端各自獨立的 Session）─────────────────

    #[test]
    fn correlation_needs_both_sides_and_pairs_by_cin() {
        use sipress_core::trace::{CallOutcome, CallTrace};
        let app = AppState::default();
        assert!(correlation_report(&app)
            .unwrap_err()
            .contains("民眾端與座席端"));

        let trace = |id: &str, agent: Option<&str>, at: u64| CallTrace {
            sip_call_id: id.into(),
            cin_call_id: Some("C1".into()),
            caller_number: "0912".into(),
            agent: agent.map(Into::into),
            invite_at_ms: at,
            answered_at_ms: Some(at + 100),
            outcome: CallOutcome::Answered,
            ..Default::default()
        };
        let caller = Arc::new(TraceLog::default());
        caller.start("c1", trace("c1", None, 1_000));
        *app.caller.traces.lock().unwrap() = Some(caller);
        assert!(correlation_report(&app).is_err(), "只有一端時不可產生");

        let agent = Arc::new(TraceLog::default());
        agent.start("a1", trace("a1", Some("1001"), 1_250));
        *app.agent.traces.lock().unwrap() = Some(agent);
        let r = correlation_report(&app).unwrap();
        assert_eq!((r.caller_calls, r.matched_by_cin, r.missed), (1, 1, 0));
        assert_eq!(r.distribution.max_ms, 250.0);
    }

    #[test]
    fn mode_deserializes_from_lowercase_json_only() {
        // Tauri 會把前端 invoke 的 `mode` 參數以 JSON 反序列化
        assert_eq!(
            serde_json::from_str::<Mode>("\"caller\"").unwrap(),
            Mode::Caller
        );
        assert_eq!(
            serde_json::from_str::<Mode>("\"agent\"").unwrap(),
            Mode::Agent
        );
        assert!(serde_json::from_str::<Mode>("\"Caller\"").is_err());
        assert!(serde_json::from_str::<Mode>("\"x\"").is_err());
    }

    #[test]
    fn session_returns_the_same_arc_per_mode() {
        let app = AppState::default();
        assert!(Arc::ptr_eq(&app.session(&Mode::Caller), &app.caller));
        assert!(Arc::ptr_eq(&app.session(&Mode::Agent), &app.agent));
        assert!(Arc::ptr_eq(
            &app.session(&Mode::Caller),
            &app.session(&Mode::Caller)
        ));
        assert!(
            !Arc::ptr_eq(&app.caller, &app.agent),
            "兩端必須是不同的 Session"
        );
    }

    #[test]
    fn one_side_error_does_not_affect_the_other() {
        let app = AppState::default();
        assert!(app.caller.try_begin());
        assert!(app.agent.try_begin());
        *app.caller.snapshot.lock().unwrap() = Some(StatsSnapshot {
            calls_initiated: 3,
            ..Default::default()
        });

        app.agent.finish(Err("座席端失敗".into()));

        assert!(
            app.caller.running.load(Ordering::SeqCst),
            "民眾端應仍在執行"
        );
        assert!(app.caller.error.lock().unwrap().is_none());
        assert!(app.caller.report.lock().unwrap().is_none());
        let snap = app.caller.snapshot.lock().unwrap().clone().unwrap();
        assert!(!snap.finished, "民眾端 snapshot 不應被標成結束");
        assert_eq!(snap.calls_initiated, 3);
        assert_eq!(
            app.agent.error.lock().unwrap().as_deref(),
            Some("座席端失敗")
        );
        assert!(!app.agent.running.load(Ordering::SeqCst));
    }

    #[test]
    fn interleaved_start_finish_rounds_keep_state_correct() {
        let app = AppState::default();
        for round in 0..5 {
            assert!(app.caller.try_begin(), "round {round}: 民眾端啟動");
            assert!(app.agent.try_begin(), "round {round}: 座席端啟動");
            assert!(!app.caller.try_begin() && !app.agent.try_begin());

            // 民眾端先結束，並在座席端仍執行中時重新啟動
            app.caller.finish(Ok(sample_report()));
            assert!(app.agent.running.load(Ordering::SeqCst));
            assert!(
                app.caller.try_begin(),
                "round {round}: 座席端執行中，民眾端應可重啟"
            );
            assert!(
                app.caller.report.lock().unwrap().is_some(),
                "try_begin 不清除舊報告（由 start_test 清除）"
            );

            app.agent.finish(Err(format!("err{round}")));
            assert!(app.caller.running.load(Ordering::SeqCst));
            app.caller.finish(Ok(sample_report()));

            assert!(
                !app.caller.running.load(Ordering::SeqCst)
                    && !app.agent.running.load(Ordering::SeqCst)
            );
            assert!(app.caller.error.lock().unwrap().is_none());
            assert_eq!(
                app.agent.error.lock().unwrap().clone(),
                Some(format!("err{round}"))
            );
            assert!(app.agent.report.lock().unwrap().is_none());
            // 清掉本輪殘留，下一輪重新開始
            *app.caller.report.lock().unwrap() = None;
            *app.agent.error.lock().unwrap() = None;
        }
    }

    #[test]
    fn concurrent_try_begin_allows_one_winner_per_mode() {
        let app = Arc::new(AppState::default());

        let wins = |mode: Mode, app: &Arc<AppState>| {
            let barrier = Arc::new(std::sync::Barrier::new(16));
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let (app, b, mode) = (Arc::clone(app), Arc::clone(&barrier), mode.clone());
                    std::thread::spawn(move || {
                        b.wait();
                        app.session(&mode).try_begin()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|w| *w)
                .count()
        };
        assert_eq!(
            wins(Mode::Caller, &app),
            1,
            "同一模式 16 個執行緒只能有一個成功"
        );
        // 民眾端已被佔用不影響座席端
        assert_eq!(wins(Mode::Agent, &app), 1);

        // 兩個模式同時競爭：各一個成功
        let app2 = Arc::new(AppState::default());
        let barrier = Arc::new(std::sync::Barrier::new(32));
        let handles: Vec<_> = (0..32)
            .map(|i| {
                let (app, b) = (Arc::clone(&app2), Arc::clone(&barrier));
                let mode = if i % 2 == 0 {
                    Mode::Caller
                } else {
                    Mode::Agent
                };
                std::thread::spawn(move || {
                    b.wait();
                    (i % 2, app.session(&mode).try_begin())
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|(m, w)| *m == 0 && *w).count(), 1);
        assert_eq!(results.iter().filter(|(m, w)| *m == 1 && *w).count(), 1);
    }

    // ── 前端 ↔ Rust 資料契約 ───────────────────────────────────────
    // fixtures 位於 gui/contract/，與前端 vitest 共用：
    //   config-*.json        前端 buildRustConfig() 的輸出（由 vitest 產生）→ 這裡驗證 Rust 能正確解析
    //   snapshot/report.json Rust 輸出的範例（由這裡產生）→ vitest 餵給 store 驗證欄位對應

    use serde_json::Value;
    use sipress_core::{
        rtp::stats::RtpStatsSnapshot,
        stats::{DetailedStats, LiveStats},
    };
    use std::path::PathBuf;

    fn contract_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contract")
    }

    /// 比對 Rust 的序列化結果與 fixture；設定 UPDATE_CONTRACT=1 時改為重新產生
    fn check_fixture(name: &str, actual: Value) {
        let path = contract_dir().join(name);
        if std::env::var_os("UPDATE_CONTRACT").is_some() {
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
            return;
        }
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("讀取 {} 失敗：{}", path.display(), e));
        let expected: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            expected, actual,
            "gui/contract/{} 與 Rust 輸出不一致。若是刻意變更欄位，請執行 \
             `UPDATE_CONTRACT=1 cargo test -p sipress-gui` 重新產生，並同步更新前端型別與 vitest",
            name
        );
    }

    /// 前端送出的每個欄位都必須是 Rust 認得的（serde 預設會默默忽略未知欄位，拼錯不會報錯）
    fn assert_known_keys(sent: &Value, known: &Value, path: &str) {
        match (sent, known) {
            (Value::Object(s), Value::Object(k)) => {
                for (key, v) in s {
                    let kv = k.get(key).unwrap_or_else(|| {
                        panic!("{}.{} 不是 Rust Config 的欄位（會被忽略）", path, key)
                    });
                    assert_known_keys(v, kv, &format!("{}.{}", path, key));
                }
            }
            (Value::Array(s), Value::Array(k)) => {
                for (i, (sv, kv)) in s.iter().zip(k).enumerate() {
                    assert_known_keys(sv, kv, &format!("{}[{}]", path, i));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn gui_config_payloads_deserialize_into_rust_config() {
        let mut checked = 0;
        for entry in std::fs::read_dir(contract_dir()).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !(name.starts_with("config-") && name.ends_with(".json")) {
                continue;
            }
            let sent: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let cfg: Config = serde_json::from_value(sent.clone())
                .unwrap_or_else(|e| panic!("{} 無法解析為 Config：{}", name, e));
            cfg.validate()
                .unwrap_or_else(|e| panic!("{} 未通過 Config::validate：{}", name, e));
            assert_known_keys(&sent, &serde_json::to_value(&cfg).unwrap(), &name);
            checked += 1;
        }
        assert!(
            checked >= 5,
            "gui/contract 應有前端產生的 config-*.json（目前 {} 個）",
            checked
        );
    }

    #[test]
    fn snapshot_fixture_matches_rust_output() {
        let live = LiveStats::default();
        for _ in 0..40 {
            live.on_invite();
        }
        for _ in 0..30 {
            live.on_answered();
        }
        for _ in 0..20 {
            live.on_completed();
        }
        for _ in 0..4 {
            live.on_failed();
        }
        for _ in 0..2 {
            live.on_timeout();
        }
        for _ in 0..10 {
            live.on_rtp_start();
        }
        live.agents_total.store(50, Ordering::Relaxed);
        for _ in 0..48 {
            live.on_agent_registered();
        }
        let mut snap = live.snapshot();
        snap.rtp_mos = Some(4.1);
        snap.rtp_loss_pct = Some(0.5);
        snap.rtp_jitter_ms = Some(12.5);
        snap.rtp_sent_packets = Some(5000);
        snap.rtp_recv_packets = Some(4975);
        snap.pdd_avg_ms = Some(150.0);
        check_fixture("snapshot.json", serde_json::to_value(&snap).unwrap());
    }

    #[test]
    fn report_fixture_matches_rust_output() {
        let live = LiveStats::default();
        for _ in 0..40 {
            live.on_invite();
        }
        for _ in 0..30 {
            live.on_answered();
        }
        for _ in 0..30 {
            live.on_completed();
        }
        for _ in 0..8 {
            live.on_failed();
        }
        for _ in 0..2 {
            live.on_timeout();
        }
        let detail = DetailedStats::default();
        for ms in [100.0, 150.0, 200.0] {
            detail.record_pdd(ms);
        }
        for ms in [300.0, 400.0] {
            detail.record_setup(ms);
        }
        for s in [20.0, 40.0] {
            detail.record_duration(s);
        }
        for code in [486, 486, 486, 486, 486, 503, 503, 404] {
            detail.record_fail_code(code);
        }
        let rtp = RtpStatsSnapshot {
            sent_packets: 9000,
            recv_packets: 8900,
            lost_packets: 100,
            loss_rate_pct: 1.1,
            jitter_ms: 8.0,
            mos: 3.9,
            out_of_order: 3,
            duplicates: 1,
        };
        let report = FinalReport::build(&live.snapshot(), &detail, 20.0, Some(&rtp));
        check_fixture("report.json", serde_json::to_value(&report).unwrap());
    }
}
