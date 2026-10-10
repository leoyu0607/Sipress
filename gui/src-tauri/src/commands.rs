/// Tauri commands：前端 ↔ core 橋接層
use sipress_core::{
    agent_engine::AgentEngine,
    config::{Config, Mode},
    engine::{Engine, ProgressCallback},
    html_reporter::HtmlReporter,
    registrar::{register_once, RegisterResult},
    stats::{FinalReport, StatsSnapshot},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::State;
use tokio::sync::mpsc;

// ── 全域狀態 ─────────────────────────────────────────────────────

/// 跨 command 共享的執行狀態
#[derive(Default)]
pub struct AppState {
    pub snapshot:   Mutex<Option<StatsSnapshot>>,
    pub report:     Mutex<Option<FinalReport>>,
    /// 引擎執行失敗的原因（get_report 會回傳此錯誤給前端）
    pub error:      Mutex<Option<String>>,
    pub stop_tx:    Mutex<Option<mpsc::Sender<()>>>,
    /// 是否有壓測正在執行（含停止後的收尾階段），防止重複啟動
    pub running:    AtomicBool,
}

impl AppState {
    /// 嘗試佔用執行權；已有壓測在跑時回傳 false
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
                None    => *snap = Some(StatsSnapshot { finished: true, ..Default::default() }),
            }
        }
        *self.stop_tx.lock().unwrap() = None;
        self.running.store(false, Ordering::SeqCst);
    }
}

// ── Tauri Commands ────────────────────────────────────────────────

/// 啟動壓測
/// 前端呼叫：await invoke('start_test', { config: {...} })
#[tauri::command]
pub async fn start_test(
    state:  State<'_, Arc<AppState>>,
    config: Config,
) -> Result<String, String> {
    let state = Arc::clone(&state);

    config.validate().map_err(|e| e.to_string())?;
    if !state.try_begin() {
        return Err("已有壓測正在執行，請先停止或等待結束".to_string());
    }

    // 清除舊結果
    *state.snapshot.lock().unwrap() = None;
    *state.report.lock().unwrap()   = None;
    *state.error.lock().unwrap()    = None;

    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    *state.stop_tx.lock().unwrap() = Some(stop_tx);

    let state_cb = Arc::clone(&state);
    let on_progress: ProgressCallback = Arc::new(move |snap: StatsSnapshot, _progress: f64| {
        *state_cb.snapshot.lock().unwrap() = Some(snap);
    });

    let mode = config.mode.clone();
    // 引擎放在獨立 task 執行：即使 panic 也能由 JoinError 接住並釋放執行權
    let run = tokio::spawn(async move {
        match mode {
            Mode::Caller => {
                let engine      = Engine::new(config);
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
                let engine      = AgentEngine::new(config);
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
            Ok(r)  => r.map_err(|e| e.to_string()),
            Err(e) => Err(format!("壓測引擎異常終止: {}", e)),
        };
        state.finish(result);
    });

    Ok("started".to_string())
}

/// 手動停止壓測
#[tauri::command]
pub async fn stop_test(
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    // Take the sender out BEFORE any await so MutexGuard is not held across threads
    let tx = state.stop_tx.lock().unwrap().take();
    if let Some(tx) = tx {
        let _ = tx.send(()).await;
        Ok("stopped".to_string())
    } else {
        Err("沒有正在執行的壓測".to_string())
    }
}

/// 取得即時快照（前端每秒輪詢）
#[tauri::command]
pub fn get_snapshot(
    state: State<'_, Arc<AppState>>,
) -> Option<StatsSnapshot> {
    state.snapshot.lock().unwrap().clone()
}

/// 取得最終報告（壓測完成後）；引擎執行失敗時回傳錯誤原因
#[tauri::command]
pub fn get_report(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<FinalReport>, String> {
    if let Some(e) = state.error.lock().unwrap().clone() {
        return Err(e);
    }
    Ok(state.report.lock().unwrap().clone())
}

/// 對單一帳號發起 SIP REGISTER（含 Digest 認證重送），回傳結果
/// 前端在新增帳號後立即呼叫
#[tauri::command]
pub async fn register_account(
    server:    String,
    domain:    Option<String>,
    username:  String,
    password:  String,
    expires:   Option<u32>,
    transport: Option<String>,
) -> Result<RegisterResult, String> {
    let dom_owned = domain
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            // 沒指定 domain → 取 server 的 IP 部分
            server.split(':').next().unwrap_or(&server).to_string()
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
    state:       State<'_, Arc<AppState>>,
    server_addr: String,
    timestamp:   String,
) -> Result<String, String> {
    let report = state.report.lock().unwrap().clone()
        .ok_or_else(|| "尚無測試結果，請先完成或停止一次壓測".to_string())?;
    Ok(HtmlReporter::render(&report, &timestamp, &server_addr))
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
        let state = AppState::default();
        assert!(state.try_begin());
        assert!(!state.try_begin(), "執行中不可再次啟動");
        state.finish(Ok(sample_report()));
        assert!(state.try_begin(), "結束後應可再次啟動");
    }

    #[test]
    fn engine_error_is_kept_and_snapshot_marked_finished() {
        let state = AppState::default();
        assert!(state.try_begin());
        state.finish(Err("無效的 server 位址".into()));

        assert_eq!(state.error.lock().unwrap().as_deref(), Some("無效的 server 位址"));
        assert!(state.report.lock().unwrap().is_none());
        let snap = state.snapshot.lock().unwrap().clone().expect("應有 snapshot");
        assert!(snap.finished, "前端靠 finished 偵測結束，失敗時也必須設為 true");
        assert!(!state.running.load(Ordering::SeqCst));
    }

    #[test]
    fn finish_marks_existing_snapshot_finished() {
        let state = AppState::default();
        assert!(state.try_begin());
        *state.snapshot.lock().unwrap() = Some(StatsSnapshot { calls_initiated: 7, ..Default::default() });
        state.finish(Ok(sample_report()));

        let snap = state.snapshot.lock().unwrap().clone().unwrap();
        assert!(snap.finished);
        assert_eq!(snap.calls_initiated, 7, "既有的即時數據應保留");
        assert!(state.report.lock().unwrap().is_some());
    }
}
