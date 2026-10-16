/// 座席端壓測引擎
///
/// 每個帳號各自一個 task（`runner::AccountRunner`），行為：
///   1. 用獨立 socket REGISTER（含 401 Digest 認證重送；UDP 遺失時重傳）
///   2. 在 Expires/2 時自動 re-REGISTER 維持註冊
///   3. 監聽 INVITE → 100 Trying → 180 Ringing → 200 OK + SDP（或依行為模擬拒接 / 不接）
///   4. 等待 ACK（逾時 → 主動 BYE）
///   5. 啟動真實 RTP 收發（若 enable_rtp）
///   6. 等待 BYE（逾時或行為模擬的主動掛斷 → 主動 BYE）
///   7. （選項）通話結束後立即 re-REGISTER；預設只在 Expires 到期前刷新
///   8. 結束時送 REGISTER Expires=0 解除註冊（有結束流程時先執行它）
///   9. 座席流程（選項）：註冊後 / 每通結束後 / 結束前依序撥特殊碼（登入、就緒、登出等），
///      測試中也可手動觸發（`manual_handle`）
///
/// 模組：
/// - `runner`：單一帳號的狀態與事件處理
/// - `register`：註冊狀態、REGISTER 重傳、已註冊計數
/// - `call`：通話 / 響鈴中來電的狀態與 SIP 訊息組裝
/// - `flow`：座席流程的步驟佇列與撥碼（UAC INVITE）的狀態、SIP 訊息組裝
mod call;
mod flow;
mod register;
mod runner;

use crate::config::{Config, FlowStep};
use crate::engine::ProgressCallback;
use crate::rtp::stats::{aggregate_snapshots, RtpStats, RtpStatsSnapshot};
use crate::sip_logger::{SipLogger, SipRole};
use crate::stats::{DetailedStats, DialStat, FinalReport, LiveStats};
use crate::timeline::TimelineRecorder;
use crate::trace::TraceLog;
use anyhow::{Context, Result};
use call::DialogCtx;
use runner::AccountRunner;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch, Mutex};
use tokio::time;

pub struct AgentEngine {
    config: Config,
    stop_tx: watch::Sender<bool>,
    manual_tx: broadcast::Sender<Vec<FlowStep>>,
    traces: Arc<TraceLog>,
}

impl AgentEngine {
    pub fn new(config: Config) -> Self {
        let (stop_tx, _) = watch::channel(false);
        let (manual_tx, _) = broadcast::channel(16);
        Self {
            config,
            stop_tx,
            manual_tx,
            traces: Arc::default(),
        }
    }

    /// 每通來電的追蹤紀錄（端到端對照用），執行中與結束後都可讀取
    pub fn trace_log(&self) -> Arc<TraceLog> {
        Arc::clone(&self.traces)
    }

    /// 取得停止訊號 sender，外部呼叫 `.send(true)` 即可觸發 graceful stop
    /// （含每個 runner 的 REGISTER Expires=0 解除註冊）。
    pub fn stop_handle(&self) -> watch::Sender<bool> {
        self.stop_tx.clone()
    }

    /// 手動撥碼：`.send(steps)` 讓所有已註冊的帳號依分批註冊的順序執行這串步驟
    /// （例如 GUI 的「就緒」按鈕送一個撥 `*96` 的步驟）
    pub fn manual_handle(&self) -> broadcast::Sender<Vec<FlowStep>> {
        self.manual_tx.clone()
    }

    pub async fn run(&self, on_progress: Option<ProgressCallback>) -> Result<FinalReport> {
        let cfg = self.config.clone();
        let live = Arc::new(LiveStats::default());
        let detail = Arc::new(DetailedStats::default());
        let start = Instant::now();

        cfg.validate()?;
        // 停止後的收尾時間：解除註冊約 2 秒；有結束流程時再加上分批錯開與流程本身的時間
        let register_interval = Duration::from_millis(cfg.agent_register_interval_ms);
        let stop_grace = Duration::from_secs(2)
            + if cfg.agent_flow.on_stop.is_empty() {
                Duration::ZERO
            } else {
                register_interval * cfg.agent_accounts.len().saturating_sub(1) as u32
                    + cfg.agent_flow.stop_budget()
                    + Duration::from_secs(1)
            };
        // 音檔有問題時在開始前就失敗，而不是每通電話改送靜音
        if let (true, Some(path)) = (cfg.enable_rtp, cfg.audio_file.as_deref()) {
            crate::rtp::audio::AudioSource::preload(path).await?;
        }
        live.agents_total.store(
            cfg.agent_accounts.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );

        let sip_log = Arc::new(
            SipLogger::new(&cfg.logs_dir, SipRole::Agent).unwrap_or_else(|e| {
                eprintln!("[sipress agent] 警告：無法建立 SIP log：{}", e);
                let tmp = std::env::temp_dir();
                SipLogger::new(&tmp.to_string_lossy(), SipRole::Agent)
                    .expect("無法在系統暫存目錄建立 SIP log")
            }),
        );
        eprintln!("[sipress agent] SIP log → {}", sip_log.path.display());

        let server: SocketAddr = cfg
            .server_addr
            .parse()
            .with_context(|| format!("無效的 server 位址: {}", cfg.server_addr))?;

        if cfg.agent_accounts.is_empty() {
            anyhow::bail!("座席模式需要至少一個 agent account");
        }

        // RTP 統計登記表（進行中 → 即時品質；已結束 → 最終報告）
        let rtp_registry: SharedRtpRegistry = Arc::default();

        // ── 進度回報 task ──
        let engine_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(cb) = on_progress {
            let live2 = Arc::clone(&live);
            let detail2 = Arc::clone(&detail);
            let finished = Arc::clone(&engine_finished);
            let duration = cfg.duration_secs as f64;
            // 時間到後引擎還會收尾（CANCEL / BYE / 解除註冊），期間繼續回報，直到引擎結束；
            // 寬限時間只是保險，避免引擎異常結束時 task 不會停
            let drain_grace = 3.0 + stop_grace.as_secs_f64();
            let unlimited = cfg.duration_secs == 0;
            let registry = Arc::clone(&rtp_registry);
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(1));
                let mut cps_meter = crate::stats::RateMeter::new(start);
                loop {
                    interval.tick().await;
                    let elapsed = start.elapsed().as_secs_f64();
                    let progress = if unlimited {
                        0.0
                    } else {
                        (elapsed / duration).min(1.0)
                    };

                    // 聚合即時 RTP 品質（只含進行中的通話）
                    let rtp_agg = registry.lock().await.live_aggregate();

                    let mut snap = live2.snapshot();
                    detail2.fill_live(&mut snap);
                    snap.cps = cps_meter.rate(std::time::Instant::now(), snap.calls_initiated);
                    snap.finished = finished.load(std::sync::atomic::Ordering::Relaxed);
                    if let Some(agg) = &rtp_agg {
                        snap.rtp_mos = Some(agg.mos);
                        snap.rtp_loss_pct = Some(agg.loss_rate_pct);
                        snap.rtp_jitter_ms = Some(agg.jitter_ms);
                        snap.rtp_sent_packets = Some(agg.sent_packets);
                        snap.rtp_recv_packets = Some(agg.recv_packets);
                    }
                    let done = snap.finished;
                    cb(snap, progress);
                    if !unlimited && elapsed >= duration + drain_grace {
                        break;
                    }
                    if done {
                        break;
                    }
                }
            });
        }

        // ── 分時段統計：每秒取樣，跨過時段邊界時結算（收尾時取出結算最後一段）──
        let timeline = Arc::new(std::sync::Mutex::new(Some(TimelineRecorder::new(
            cfg.stats_interval_secs,
        ))));
        if cfg.stats_interval_secs > 0 {
            let tl = Arc::clone(&timeline);
            let live = Arc::clone(&live);
            let detail = Arc::clone(&detail);
            let src = Arc::clone(&rtp_registry);
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(1));
                loop {
                    interval.tick().await;
                    let mos = src.lock().await.live_aggregate().map(|a| a.mos);
                    let mut guard = tl.lock().unwrap();
                    let Some(rec) = guard.as_mut() else { break };
                    rec.tick(start.elapsed().as_secs_f64(), &live, &detail, mos);
                }
            });
        }

        // ── 為每個帳號 spawn 一個 runner ──
        // 所有 runner 共用一份資源；設定不含帳號清單（每個 runner 各複製一份完整清單時，
        // 記憶體與啟動時間會隨帳號數平方成長：5000 個帳號約 5 GB）
        let shared = Arc::new(Shared {
            cfg: Config {
                agent_accounts: Vec::new(),
                ..cfg.clone()
            },
            live: Arc::clone(&live),
            detail: Arc::clone(&detail),
            log: Arc::clone(&sip_log),
            port_counter: Arc::new(Mutex::new(cfg.rtp_base_port)),
            rtp_registry: Arc::clone(&rtp_registry),
            traces: Arc::clone(&self.traces),
            dials: std::sync::Mutex::default(),
        });
        // 第 i 個帳號延後 i × 間隔才開始註冊，避免大量 REGISTER 同時湧入交換機
        let mut handles = Vec::new();
        for (i, acc) in cfg.agent_accounts.iter().enumerate() {
            let acc = acc.clone();
            let sh = Arc::clone(&shared);
            let stop_rx = self.stop_tx.subscribe();
            let manual_rx = self.manual_tx.subscribe();
            let start_delay = register_interval * i as u32;
            let h = tokio::spawn(async move {
                let res = match AccountRunner::connect(sh, acc, server).await {
                    Ok(runner) => runner.run(stop_rx, start_delay, manual_rx).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = res {
                    eprintln!("[sipress agent] runner 結束: {}", e);
                }
            });
            handles.push(h);
        }

        // ── 等待：測試時間到 OR 外部停止訊號 ──
        let unlimited_time = cfg.duration_secs == 0;
        let mut main_stop_rx = self.stop_tx.subscribe();
        if unlimited_time {
            let _ = main_stop_rx.changed().await;
        } else {
            tokio::select! {
                _ = time::sleep(cfg.duration()) => {}
                _ = main_stop_rx.changed()      => {}
            }
        }

        // 通知所有 runner 結束（執行結束流程後送 REGISTER Expires=0）
        let _ = self.stop_tx.send(true);
        // 等 runner 收尾（結束流程 + deregister + 停止 RTP），超過收尾時間仍未結束就中止
        let deadline = time::Instant::now() + stop_grace;
        for h in handles.iter_mut() {
            let _ = time::timeout_at(deadline, h).await;
        }
        for h in handles {
            h.abort();
        }

        engine_finished.store(true, std::sync::atomic::Ordering::Relaxed);

        // ── 收集 RTP 統計（已結束 + 收尾時仍在進行的通話）──
        let rtp_final = rtp_registry.lock().await.final_aggregate();

        // ── 產生最終報告 ──
        let snap = live.snapshot();
        let elapsed = start.elapsed().as_secs_f64();

        sip_log.log_summary(&format!(
            "[座席] 來電={} 接聽={} 完成={} 失敗={} 時長={:.1}s",
            snap.calls_initiated,
            snap.calls_answered,
            snap.calls_completed,
            snap.calls_failed,
            elapsed,
        ));

        let timeline = timeline
            .lock()
            .unwrap()
            .take()
            .map(|r| r.finish(elapsed, &live, &detail))
            .unwrap_or_default();
        let mut report = FinalReport::build(&snap, &detail, elapsed, rtp_final.as_ref());
        report.timeline = timeline;
        report.agent_dials = shared.dials.lock().unwrap().clone();
        Ok(report)
    }
}

/// 所有帳號 runner 共用的資源
struct Shared {
    /// 不含帳號清單的設定
    cfg: Config,
    live: Arc<LiveStats>,
    detail: Arc<DetailedStats>,
    log: Arc<SipLogger>,
    /// RTP port 計數器
    port_counter: Arc<Mutex<u16>>,
    rtp_registry: SharedRtpRegistry,
    traces: Arc<TraceLog>,
    /// 座席流程每個撥碼範本的結果（依第一次出現的順序）
    dials: std::sync::Mutex<Vec<DialStat>>,
}

impl Shared {
    /// 記錄一次撥碼結果：`fail` = None 為成功，Some(回應碼) 為失敗（0 = 沒有回應）
    fn record_dial(&self, template: &str, label: &str, fail: Option<u16>) {
        self.live.on_agent_dial(fail.is_none());
        let mut dials = self.dials.lock().unwrap();
        let idx = match dials.iter().position(|d| d.code == template) {
            Some(i) => i,
            None => {
                dials.push(DialStat {
                    code: template.to_string(),
                    label: label.to_string(),
                    ..Default::default()
                });
                dials.len() - 1
            }
        };
        let d = &mut dials[idx];
        match fail {
            None => d.ok += 1,
            Some(code) => {
                d.failed += 1;
                *d.fail_codes.entry(code).or_default() += 1;
            }
        }
    }

    /// 停止通話的 RTP session，將最終統計移入登記表的已結束列表
    async fn stop_call_rtp(&self, call_id: &str, ctx: &mut DialogCtx, ext: &str) {
        if let Some(rtp) = ctx.rtp_session.take() {
            let snap = rtp.stop();
            self.traces
                .update(&trace_key(ext, call_id), |t| t.set_rtp(&snap));
            let mut reg = self.rtp_registry.lock().await;
            reg.active.remove(call_id);
            reg.finished.push(snap);
            self.live.on_rtp_stop();
        }
    }
}

/// 所有 runner 共用的 RTP 統計登記表
#[derive(Default)]
struct RtpRegistry {
    /// 進行中通話（Call-ID → 即時統計），用於即時品質聚合
    active: HashMap<String, Arc<RtpStats>>,
    /// 已結束通話的最終快照，用於最終報告
    finished: Vec<RtpStatsSnapshot>,
}

type SharedRtpRegistry = Arc<Mutex<RtpRegistry>>;

impl RtpRegistry {
    /// 即時品質：只聚合進行中的通話
    fn live_aggregate(&self) -> Option<RtpStatsSnapshot> {
        let snaps: Vec<_> = self.active.values().map(|s| s.snapshot()).collect();
        aggregate_snapshots(&snaps)
    }

    /// 最終報告：已結束通話 + 仍在進行的通話
    fn final_aggregate(&self) -> Option<RtpStatsSnapshot> {
        let mut snaps = self.finished.clone();
        snaps.extend(self.active.values().map(|s| s.snapshot()));
        aggregate_snapshots(&snaps)
    }
}

/// 追蹤紀錄的鍵：同一個 SIP Call-ID 可能分別送到不同座席帳號
fn trace_key(ext: &str, call_id: &str) -> String {
    format!("{}\u{1f}{}", ext, call_id)
}
