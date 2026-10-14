/// Tauri commands：前端 ↔ core 橋接層
use sipress_core::{
    agent_engine::AgentEngine,
    config::{Config, Mode},
    engine::{Engine, ProgressCallback},
    html_reporter::HtmlReporter,
    registrar::{register_once, RegisterResult},
    stats::{FinalReport, StatsSnapshot},
};
use std::sync::{Arc, Mutex};
use tauri::State;
use tokio::sync::mpsc;

// ── 全域狀態 ─────────────────────────────────────────────────────

/// 跨 command 共享的執行狀態
pub struct AppState {
    pub snapshot:   Mutex<Option<StatsSnapshot>>,
    pub report:     Mutex<Option<FinalReport>>,
    pub stop_tx:    Mutex<Option<mpsc::Sender<()>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(None),
            report:   Mutex::new(None),
            stop_tx:  Mutex::new(None),
        }
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

    // 清除舊結果
    *state.snapshot.lock().unwrap() = None;
    *state.report.lock().unwrap()   = None;

    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    *state.stop_tx.lock().unwrap() = Some(stop_tx);

    let state_cb = Arc::clone(&state);
    let on_progress: ProgressCallback = Arc::new(move |snap: StatsSnapshot, _progress: f64| {
        *state_cb.snapshot.lock().unwrap() = Some(snap);
    });

    let mode = config.mode.clone();
    tokio::spawn(async move {
        let result = match mode {
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
        };
        match result {
            Ok(report) => *state.report.lock().unwrap() = Some(report),
            Err(e)     => tracing::error!("Engine 錯誤: {}", e),
        }
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

/// 取得最終報告（壓測完成後）
#[tauri::command]
pub fn get_report(
    state: State<'_, Arc<AppState>>,
) -> Option<FinalReport> {
    state.report.lock().unwrap().clone()
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

/// 將文字內容寫入使用者在「另存新檔」對話框選定的路徑
#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| format!("寫入 {} 失敗: {}", path, e))
}
