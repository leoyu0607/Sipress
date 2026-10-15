/// 座席端壓測引擎
///
/// 每個帳號各自一個 task，行為：
///   1. 用獨立 UDP socket REGISTER（含 401 Digest 認證重送）
///   2. 在 Expires/2 時自動 re-REGISTER 維持註冊
///   3. 監聽 INVITE → 100 Trying → 180 Ringing → 200 OK + SDP
///   4. 等待 ACK（10s timeout → 主動 BYE）
///   5. 啟動真實 RTP 收發（若 enable_rtp）
///   6. 等待 BYE（150s timeout → 主動 BYE）
///   7. （選項）通話結束後立即 re-REGISTER；預設只在 Expires 到期前刷新
///   8. 結束時送 REGISTER Expires=0 解除註冊
///
/// 設計原則：簡單、可觀察。用單一 socket 收發，避免多 socket 同步問題。
use crate::config::{AgentAccount, AgentBehavior, Config};
use crate::engine::ProgressCallback;
use crate::rtp::session::{RtpSession, RtpSessionConfig};
use crate::rtp::stats::{aggregate_snapshots, RtpStats, RtpStatsSnapshot};
use crate::sip::{
    register::{DigestChallenge, RegisterMessage},
    sdp, SipMessage, SipParser, SipResponse, SipSocket,
};
use crate::sip_logger::{Direction, SipLogger, SipRole};
use crate::stats::{DetailedStats, FinalReport, LiveStats};
use crate::trace::{now_ms, CallOutcome, CallTrace, TraceLog};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};
use tokio::time;

/// 同一輪 REGISTER 收到 401/407 後最多帶認證重送幾次（避免密碼錯誤時無限重送）
const MAX_AUTH_RETRIES: u32 = 2;

pub struct AgentEngine {
    config: Config,
    stop_tx: watch::Sender<bool>,
    traces: Arc<TraceLog>,
}

impl AgentEngine {
    pub fn new(config: Config) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self {
            config,
            stop_tx,
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

    pub async fn run(&self, on_progress: Option<ProgressCallback>) -> Result<FinalReport> {
        let cfg = self.config.clone();
        let live = Arc::new(LiveStats::default());
        let detail = Arc::new(DetailedStats::default());
        let start = Instant::now();

        cfg.validate()?;
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

        // RTP port 計數器（所有 runner 共用）
        let rtp_port_counter = Arc::new(Mutex::new(cfg.rtp_base_port));

        // RTP 統計登記表（進行中 → 即時品質；已結束 → 最終報告）
        let rtp_registry: SharedRtpRegistry = Arc::default();

        // ── 進度回報 task ──
        let engine_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(cb) = on_progress {
            let live2 = Arc::clone(&live);
            let detail2 = Arc::clone(&detail);
            let finished = Arc::clone(&engine_finished);
            let duration = cfg.duration_secs as f64;
            let unlimited = cfg.duration_secs == 0;
            let registry = Arc::clone(&rtp_registry);
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(1));
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
                    snap.pdd_avg_ms = detail2.pdd_mean_ms();
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
                    if !unlimited && elapsed >= duration {
                        break;
                    }
                    if done {
                        break;
                    }
                }
            });
        }

        // ── 為每個帳號 spawn 一個 runner ──
        let mut handles = Vec::new();
        for acc in &cfg.agent_accounts {
            let acc = acc.clone();
            let cfg = cfg.clone();
            let live = Arc::clone(&live);
            let detail_r = Arc::clone(&detail);
            let log = Arc::clone(&sip_log);
            let stop_rx = self.stop_tx.subscribe();
            let port_ctr = Arc::clone(&rtp_port_counter);
            let registry = Arc::clone(&rtp_registry);
            let traces = Arc::clone(&self.traces);
            let h = tokio::spawn(async move {
                if let Err(e) = account_runner(
                    acc, server, cfg, live, detail_r, log, stop_rx, port_ctr, registry, traces,
                )
                .await
                {
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

        // 通知所有 runner 結束（會送 REGISTER Expires=0）
        let _ = self.stop_tx.send(true);
        // 給 runner 時間 deregister + 停止 RTP
        time::sleep(Duration::from_secs(2)).await;
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

        Ok(FinalReport::build(
            &snap,
            &detail,
            elapsed,
            rtp_final.as_ref(),
        ))
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

/// 停止通話的 RTP session，將最終統計移入登記表的已結束列表
async fn stop_call_rtp(
    call_id: &str,
    ctx: &mut DialogCtx,
    registry: &Mutex<RtpRegistry>,
    live: &LiveStats,
    traces: &TraceLog,
    ext: &str,
) {
    if let Some(rtp) = ctx.rtp_session.take() {
        let snap = rtp.stop();
        traces.update(&trace_key(ext, call_id), |t| t.set_rtp(&snap));
        let mut reg = registry.lock().await;
        reg.active.remove(call_id);
        reg.finished.push(snap);
        live.on_rtp_stop();
    }
}

// ─── 單一帳號 runner ──────────────────────────────────────────────

struct DialogCtx {
    invite_raw: String,
    local_to_tag: String,
    answered_at: Instant,
    remote_from_tag: String,
    rtp_session: Option<RtpSession>,
    ack_received: bool,
    ack_timeout_secs: u64,
    bye_timeout_secs: u64,
    /// 座席主動掛斷的時間（行為模擬；None = 等對方 BYE）
    hangup_at: Option<Instant>,
    /// 協商出的 codec（RTP 與 RE-INVITE 的 answer 都用它）
    codec_pt: u8,
}

impl DialogCtx {
    fn timeout_at(&self) -> Instant {
        let secs = if self.ack_received {
            self.bye_timeout_secs
        } else {
            self.ack_timeout_secs
        };
        self.answered_at + Duration::from_secs(secs)
    }

    /// 已收到 ACK 且到了主動掛斷時間
    fn agent_hangup_due(&self, now: Instant) -> bool {
        self.ack_received && self.hangup_at.is_some_and(|t| now >= t)
    }

    fn deadline(&self) -> Instant {
        let timeout = self.timeout_at();
        match self.hangup_at {
            Some(t) if self.ack_received => timeout.min(t),
            _ => timeout,
        }
    }
}

// ─── 座席行為模擬 ─────────────────────────────────────────────────

/// 響鈴結束後要做的事
#[derive(Debug, Clone, Copy, PartialEq)]
enum RingOutcome {
    Answer,
    /// 486 Busy Here
    Reject,
    /// 不接：等 CANCEL，逾時回 480
    NoAnswer,
}

impl RingOutcome {
    /// `roll` 為 [0, 100) 的隨機數
    fn decide(b: &AgentBehavior, roll: f64) -> Self {
        if roll < b.reject_pct {
            Self::Reject
        } else if roll < b.reject_pct + b.no_answer_pct {
            Self::NoAnswer
        } else {
            Self::Answer
        }
    }
}

/// [min, max] 秒內均勻隨機；max = 0 → None（停用）
fn random_secs(min: f64, max: f64) -> Option<Duration> {
    use rand::Rng;
    if max <= 0.0 {
        return None;
    }
    let secs = if max > min {
        rand::thread_rng().gen_range(min..=max)
    } else {
        max
    };
    Some(Duration::from_secs_f64(secs))
}

/// 已送 180 Ringing、尚未送最終回應的來電
struct PendingInvite {
    invite_raw: String,
    local_to_tag: String,
    remote_from_tag: String,
    outcome: RingOutcome,
    /// 響鈴結束（送出最終回應）的時間
    due: Instant,
}

#[allow(clippy::too_many_arguments)]
async fn account_runner(
    account: AgentAccount,
    server: SocketAddr,
    cfg: Config,
    live: Arc<LiveStats>,
    detail: Arc<DetailedStats>,
    log: Arc<SipLogger>,
    mut stop: watch::Receiver<bool>,
    port_counter: Arc<Mutex<u16>>,
    rtp_registry: SharedRtpRegistry,
    traces: Arc<TraceLog>,
) -> Result<()> {
    let sock =
        SipSocket::connect(cfg.transport, server, crate::net::unspecified_for(&server)).await?;
    let local_addr = sock.local_addr.clone();
    let local_ip = crate::net::host(&local_addr);

    let domain = if !account.domain.is_empty() {
        account.domain.clone()
    } else {
        server.ip().to_string()
    };

    let reg_from_tag = SipMessage::new_tag();
    let reg_call_id = SipMessage::new_call_id(&domain);
    let reg_state = Arc::new(Mutex::new(RegState {
        cseq: 0,
        challenge: None,
        auth_retries: 0,
    }));

    let initial_expires = 240u32;
    let mut current_expires = initial_expires;
    let server_addr_str = cfg.server_addr.clone();
    let transport_str = cfg.transport_str();

    let dialogs: Arc<Mutex<HashMap<String, DialogCtx>>> = Arc::new(Mutex::new(HashMap::new()));
    // 響鈴中（尚未接聽 / 拒接）的來電
    let mut pending: HashMap<String, PendingInvite> = HashMap::new();
    // wrap-up：此時間之前的新來電一律 486
    let mut wrap_until: Option<Instant> = None;
    let wrap_up = Duration::from_secs_f64(cfg.agent_behavior.wrap_up_secs);
    // runner 結束或被 abort 時 drop → 自動取消註冊計數
    let mut registered = RegisteredGauge::new(Arc::clone(&live));

    log.log_event(&account.extension, "開始 REGISTER");

    if let Err(e) = send_register(
        &sock,
        &log,
        &server_addr_str,
        &local_addr,
        &account,
        &reg_from_tag,
        &reg_call_id,
        transport_str,
        initial_expires,
        None,
        &mut *reg_state.lock().await,
    )
    .await
    {
        log.log_event(
            &account.extension,
            &format!("初始 REGISTER 送出失敗: {}", e),
        );
        return Err(e);
    }

    let mut last_register_at = Instant::now();

    loop {
        let refresh_at =
            last_register_at + Duration::from_secs((current_expires as u64 / 2).max(60));

        let next_timeout = {
            let dlgs = dialogs.lock().await;
            dlgs.values()
                .map(|ctx| ctx.deadline())
                .chain(pending.values().map(|p| p.due))
                .map(tokio::time::Instant::from_std)
                .min()
                .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(3600))
        };

        tokio::select! {
            res = sock.recv() => {
                let raw = match res {
                    Ok(raw) => raw,
                    Err(e) => { log.log_event(&account.extension, &format!("recv 失敗: {:#}", e)); break; }
                };
                log.log_message(Direction::Recv, &raw, &server.to_string());

                if raw.starts_with("SIP/2.0") {
                    handle_response(
                        &raw, &sock, &log, &cfg, &server_addr_str, &local_addr,
                        &account, &reg_from_tag, &reg_call_id, transport_str,
                        &mut current_expires, &mut last_register_at,
                        Arc::clone(&reg_state), &mut registered,
                    ).await;
                } else {
                    let call_ended = handle_request(
                        &raw, &sock, &log, &local_addr, &local_ip,
                        &account, &cfg, &live, &detail, Arc::clone(&dialogs),
                        &port_counter, &rtp_registry, &mut pending, wrap_until, &traces,
                    ).await;
                    if call_ended {
                        wrap_until = Some(Instant::now() + wrap_up);
                        if cfg.agent_reregister_after_call {
                            trigger_re_register(
                                &sock, &log, &server_addr_str, &local_addr,
                                &account, &reg_from_tag, &reg_call_id, transport_str,
                                current_expires, Arc::clone(&reg_state),
                            ).await;
                            last_register_at = Instant::now();
                        }
                    }
                }
            }

            _ = time::sleep_until(tokio::time::Instant::from_std(refresh_at)) => {
                if registered.expired(current_expires) {
                    log.log_event(&account.extension, "註冊已過期（刷新未獲 200 OK）");
                    registered.set(false);
                }
                log.log_event(&account.extension, "re-REGISTER（刷新）");
                last_register_at = Instant::now();
                let mut st = reg_state.lock().await;
                st.cseq = st.cseq.wrapping_add(1);
                let cseq = st.cseq;
                let challenge = st.challenge.clone();
                drop(st);
                let auth = challenge.as_ref().map(|c| {
                    c.authorization_header(&account.username, &account.password, "REGISTER",
                                          &format!("sip:{}", server_addr_str))
                });
                let req = RegisterMessage::build(
                    &account.username, &server_addr_str, &local_addr,
                    cseq, &SipMessage::new_branch(), &reg_from_tag, &reg_call_id,
                    transport_str, current_expires, auth.as_deref(),
                );
                log.log_message(Direction::Send, &req, &server.to_string());
                let _ = sock.send(&req).await;
            }

            // 響鈴結束（接聽 / 拒接 / 不接逾時）；通話 timeout（ACK / BYE）或主動掛斷 → 座席主動 BYE
            _ = time::sleep_until(next_timeout) => {
                let now = Instant::now();
                let due: Vec<String> = pending.iter()
                    .filter(|(_, p)| now >= p.due)
                    .map(|(k, _)| k.clone())
                    .collect();
                for call_id in due {
                    let Some(p) = pending.remove(&call_id) else { continue };
                    match p.outcome {
                        RingOutcome::Answer => {
                            let ctx = answer_call(
                                &p.invite_raw, &call_id, p.local_to_tag, p.remote_from_tag,
                                &sock, &log, &local_addr, &local_ip, &account, &cfg, &live,
                                &port_counter, &rtp_registry, &traces,
                            ).await;
                            dialogs.lock().await.insert(call_id, ctx);
                        }
                        RingOutcome::Reject => {
                            log.log_event(&account.extension, &format!("[{}] 拒接 → 486", short(&call_id)));
                            fail_invite(&p.invite_raw, &p.local_to_tag, "486 Busy Here", 486, &sock, &log, &live, &detail, &traces, &account.extension).await;
                        }
                        RingOutcome::NoAnswer => {
                            log.log_event(&account.extension, &format!(
                                "[{}] 響鈴 {}s 未被取消 → 480", short(&call_id), AgentBehavior::NO_ANSWER_CAP_SECS));
                            fail_invite(&p.invite_raw, &p.local_to_tag, "480 Temporarily Unavailable", 480, &sock, &log, &live, &detail, &traces, &account.extension).await;
                        }
                    }
                }

                let mut dlgs = dialogs.lock().await;
                let timed_out: Vec<String> = dlgs.iter()
                    .filter(|(_, ctx)| now >= ctx.deadline())
                    .map(|(k, _)| k.clone())
                    .collect();
                for call_id in timed_out {
                    if let Some(mut ctx) = dlgs.remove(&call_id) {
                        let reason = if ctx.agent_hangup_due(now) {
                            "座席掛斷"
                        } else if !ctx.ack_received {
                            "ACK timeout"
                        } else {
                            "BYE timeout"
                        };
                        log.log_event(&account.extension,
                            &format!("[{}] {} → 主動 BYE", short(&call_id), reason));
                        // 建構 agent-initiated BYE
                        let bye = build_bye_from_dialog(
                            &ctx.invite_raw, &ctx.local_to_tag, &ctx.remote_from_tag,
                            &local_addr, &account,
                        );
                        log.log_message(Direction::Send, &bye, &server.to_string());
                        let _ = sock.send(&bye).await;
                        stop_call_rtp(&call_id, &mut ctx, &rtp_registry, &live, &traces, &account.extension).await;
                        detail.record_duration(ctx.answered_at.elapsed().as_secs_f64());
                        live.on_completed();
                        traces.update(&trace_key(&account.extension, &call_id), |t| t.ended_at_ms = Some(now_ms()));
                        wrap_until = Some(Instant::now() + wrap_up);
                        if cfg.agent_reregister_after_call {
                            trigger_re_register(
                                &sock, &log, &server_addr_str, &local_addr,
                                &account, &reg_from_tag, &reg_call_id, transport_str,
                                current_expires, Arc::clone(&reg_state),
                            ).await;
                            last_register_at = Instant::now();
                        }
                    }
                }
            }

            _ = stop.changed() => {
                log.log_event(&account.extension, "收到停止訊號，發送 REGISTER Expires=0");
                registered.set(false);
                // 響鈴中的來電 → 480（座席下線）
                for (_, p) in pending.drain() {
                    fail_invite(&p.invite_raw, &p.local_to_tag, "480 Temporarily Unavailable", 480, &sock, &log, &live, &detail, &traces, &account.extension).await;
                }
                // 停止所有 RTP sessions
                {
                    let mut dlgs = dialogs.lock().await;
                    for (call_id, ctx) in dlgs.iter_mut() {
                        stop_call_rtp(call_id, ctx, &rtp_registry, &live, &traces, &account.extension).await;
                    }
                }
                let mut st = reg_state.lock().await;
                st.cseq = st.cseq.wrapping_add(1);
                let cseq = st.cseq;
                let challenge = st.challenge.clone();
                drop(st);
                let auth = challenge.as_ref().map(|c| {
                    c.authorization_header(&account.username, &account.password, "REGISTER",
                                          &format!("sip:{}", server_addr_str))
                });
                let req = RegisterMessage::build(
                    &account.username, &server_addr_str, &local_addr,
                    cseq, &SipMessage::new_branch(), &reg_from_tag, &reg_call_id,
                    transport_str, 0, auth.as_deref(),
                );
                log.log_message(Direction::Send, &req, &server.to_string());
                let _ = sock.send(&req).await;
                let _ = time::timeout(Duration::from_millis(300), sock.recv()).await;
                break;
            }
        }
    }

    Ok(())
}

// ─── REGISTER 狀態 ────────────────────────────────────────────────

/// 單一帳號的註冊狀態；狀態改變時才增減 `LiveStats::agents_registered`，drop 時自動取消
struct RegisteredGauge {
    live: Arc<LiveStats>,
    on: bool,
    /// 最近一次 REGISTER 200 OK 的時間
    last_ok: Instant,
}

impl RegisteredGauge {
    fn new(live: Arc<LiveStats>) -> Self {
        Self {
            live,
            on: false,
            last_ok: Instant::now(),
        }
    }

    fn set(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        self.on = on;
        if on {
            self.live.on_agent_registered()
        } else {
            self.live.on_agent_unregistered()
        }
    }

    /// 收到 REGISTER 200 OK（Expires=0 代表伺服器已移除註冊）
    fn on_ok(&mut self, expires: Option<u32>) {
        self.last_ok = Instant::now();
        self.set(expires != Some(0));
    }

    /// 已註冊但超過 expires 秒都沒有再收到 200 OK
    fn expired(&self, expires: u32) -> bool {
        self.on && self.last_ok.elapsed() > Duration::from_secs(expires as u64)
    }
}

impl Drop for RegisteredGauge {
    fn drop(&mut self) {
        self.set(false);
    }
}

struct RegState {
    cseq: u32,
    challenge: Option<DigestChallenge>,
    /// 自上次 200 OK 以來連續帶認證重送的次數
    auth_retries: u32,
}

#[allow(clippy::too_many_arguments)]
async fn send_register(
    sock: &SipSocket,
    log: &SipLogger,
    server_addr: &str,
    local_addr: &str,
    account: &AgentAccount,
    from_tag: &str,
    call_id: &str,
    transport: &str,
    expires: u32,
    auth: Option<&str>,
    state: &mut RegState,
) -> Result<()> {
    state.cseq = state.cseq.wrapping_add(1);
    let req = RegisterMessage::build(
        &account.username,
        server_addr,
        local_addr,
        state.cseq,
        &SipMessage::new_branch(),
        from_tag,
        call_id,
        transport,
        expires,
        auth,
    );
    log.log_message(Direction::Send, &req, server_addr);
    sock.send(&req).await?;
    Ok(())
}

/// 通話結束後立即 re-REGISTER（`agent_reregister_after_call` 開啟時）
#[allow(clippy::too_many_arguments)]
async fn trigger_re_register(
    sock: &SipSocket,
    log: &SipLogger,
    server_addr: &str,
    local_addr: &str,
    account: &AgentAccount,
    from_tag: &str,
    call_id: &str,
    transport: &str,
    expires: u32,
    reg_state: Arc<Mutex<RegState>>,
) {
    let mut st = reg_state.lock().await;
    st.cseq = st.cseq.wrapping_add(1);
    let cseq = st.cseq;
    let challenge = st.challenge.clone();
    drop(st);
    let auth = challenge.as_ref().map(|c| {
        c.authorization_header(
            &account.username,
            &account.password,
            "REGISTER",
            &format!("sip:{}", server_addr),
        )
    });
    let req = RegisterMessage::build(
        &account.username,
        server_addr,
        local_addr,
        cseq,
        &SipMessage::new_branch(),
        from_tag,
        call_id,
        transport,
        expires,
        auth.as_deref(),
    );
    log.log_message(Direction::Send, &req, server_addr);
    let _ = sock.send(&req).await;
    log.log_event(&account.extension, "通話結束 → re-REGISTER");
}

// ─── 收到 SIP 回應的處理 ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_response(
    raw: &str,
    sock: &SipSocket,
    log: &SipLogger,
    _cfg: &Config,
    server_addr_str: &str,
    local_addr: &str,
    account: &AgentAccount,
    reg_from_tag: &str,
    reg_call_id: &str,
    transport_str: &str,
    current_expires: &mut u32,
    last_register_at: &mut Instant,
    reg_state: Arc<Mutex<RegState>>,
    registered: &mut RegisteredGauge,
) {
    let code = SipParser::status_code(raw).unwrap_or(0);
    let method = SipParser::cseq_method(raw);

    if method.as_deref() != Some("REGISTER") {
        return;
    }

    match code {
        200 => {
            reg_state.lock().await.auth_retries = 0;
            *last_register_at = Instant::now();
            log.log_event(&account.extension, "註冊成功");
            let expires = SipParser::expires(raw);
            if let Some(exp) = expires {
                *current_expires = exp;
            }
            registered.on_ok(expires);
        }
        401 | 407 => {
            if let Some(chal) = DigestChallenge::parse(raw) {
                let auth = chal.authorization_header(
                    &account.username,
                    &account.password,
                    "REGISTER",
                    &format!("sip:{}", server_addr_str),
                );
                {
                    let mut st = reg_state.lock().await;
                    st.challenge = Some(chal);
                    if st.auth_retries >= MAX_AUTH_RETRIES {
                        // 已帶認證仍被拒 → 帳密錯誤，停止重送；等下次 refresh 計時再試
                        log.log_event(
                            &account.extension,
                            &format!(
                                "認證失敗（SIP {}，已重試 {} 次），停止重送 REGISTER",
                                code, st.auth_retries,
                            ),
                        );
                        registered.set(false);
                        return;
                    }
                    st.auth_retries += 1;
                    if let Err(e) = send_register(
                        sock,
                        log,
                        server_addr_str,
                        local_addr,
                        account,
                        reg_from_tag,
                        reg_call_id,
                        transport_str,
                        *current_expires,
                        Some(&auth),
                        &mut st,
                    )
                    .await
                    {
                        log.log_event(&account.extension, &format!("認證重送失敗: {}", e));
                    }
                }
            } else {
                log.log_event(&account.extension, "無法解析 Digest challenge");
            }
        }
        _ => {
            log.log_event(&account.extension, &format!("REGISTER 回應 SIP {}", code));
            // 1xx 是暫時回應；其他最終回應（3xx–6xx）代表註冊被拒
            if code >= 300 {
                registered.set(false);
            }
        }
    }
}

// ─── 收到 SIP 請求的處理（INVITE / BYE / ACK / RE-INVITE）；回傳是否有通話結束 ──────────

#[allow(clippy::too_many_arguments)]
async fn handle_request(
    raw: &str,
    sock: &SipSocket,
    log: &SipLogger,
    local_addr: &str,
    local_ip: &str,
    account: &AgentAccount,
    cfg: &Config,
    live: &LiveStats,
    detail: &DetailedStats,
    dialogs: Arc<Mutex<HashMap<String, DialogCtx>>>,
    port_counter: &Arc<Mutex<u16>>,
    rtp_registry: &SharedRtpRegistry,
    pending: &mut HashMap<String, PendingInvite>,
    wrap_until: Option<Instant>,
    traces: &TraceLog,
) -> bool {
    let method = raw
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .map(|s| s.to_uppercase())
        .unwrap_or_default();

    let call_id = match SipParser::call_id(raw) {
        Some(c) => c,
        None => return false,
    };

    match method.as_str() {
        "INVITE" => {
            let mut dlgs = dialogs.lock().await;
            let existing = dlgs.contains_key(&call_id);
            if existing {
                // RE-INVITE：回 200 OK + SDP，並更新 RTP 目標
                let ctx = dlgs.get_mut(&call_id).unwrap();
                let port = ctx
                    .rtp_session
                    .as_ref()
                    .map(|r| r.local_port())
                    .unwrap_or_else(|| pick_dummy_rtp_port(local_ip));
                let ok = SipMessage::ok_for_server_reinvite(
                    raw,
                    local_addr,
                    port,
                    &account.extension,
                    ctx.codec_pt,
                );
                log.log_message(Direction::Send, &ok, "server");
                let _ = sock.send(&ok).await;
                // 更新 RTP 目標地址（對端可能換 port）
                if cfg.enable_rtp {
                    let sip_ip = crate::net::host(&cfg.server_addr);
                    if let Some(new_rtp_addr) = SipResponse::sdp_rtp_addr(raw, &sip_ip) {
                        if let Some(rtp) = &ctx.rtp_session {
                            match rtp.update_remote(&new_rtp_addr).await {
                                Ok(()) => log.log_event(
                                    &account.extension,
                                    &format!(
                                        "[{}] RE-INVITE RTP → {}",
                                        short(&call_id),
                                        new_rtp_addr
                                    ),
                                ),
                                Err(e) => log.log_event(
                                    &account.extension,
                                    &format!("[{}] RE-INVITE RTP 更新失敗: {}", short(&call_id), e),
                                ),
                            }
                        }
                    }
                }
                log.log_event(
                    &account.extension,
                    &format!("[{}] 回應 RE-INVITE", short(&call_id)),
                );
            } else if let Some(p) = pending.get(&call_id) {
                // 響鈴中收到重傳的 INVITE → 重送 180
                let ringing = SipMessage::response_with_to_tag(raw, "180 Ringing", &p.local_to_tag);
                log.log_message(Direction::Send, &ringing, "server");
                let _ = sock.send(&ringing).await;
            } else {
                // ── 新通話：100 Trying → 180 Ringing →（響鈴延遲）→ 200 OK + SDP / 486 / 不接 ──
                // PDD / 建立時間只會量到本機處理時間（或刻意的響鈴延遲），故不記錄
                live.on_invite();
                traces.start(
                    &trace_key(&account.extension, &call_id),
                    CallTrace {
                        sip_call_id: call_id.clone(),
                        cin_call_id: SipParser::cin_call_id(raw),
                        caller_number: SipParser::from_user(raw).unwrap_or_default(),
                        agent: Some(account.extension.clone()),
                        invite_at_ms: now_ms(),
                        ..Default::default()
                    },
                );

                let local_to_tag = SipMessage::new_tag();
                let behavior = &cfg.agent_behavior;

                // wrap-up 休息中，或通話中忙線（已有通話 / 響鈴中）→ 直接忙線
                let busy_reason = if wrap_until.is_some_and(|t| Instant::now() < t) {
                    Some("休息中")
                } else if behavior.busy_in_call && (!dlgs.is_empty() || !pending.is_empty()) {
                    Some("通話中")
                } else {
                    None
                };
                if let Some(reason) = busy_reason {
                    log.log_event(
                        &account.extension,
                        &format!("[{}] {} → 486", short(&call_id), reason),
                    );
                    fail_invite(
                        raw,
                        &local_to_tag,
                        "486 Busy Here",
                        486,
                        sock,
                        log,
                        live,
                        detail,
                        traces,
                        &account.extension,
                    )
                    .await;
                    return false;
                }

                // offer 沒有任何 G.711 → 488（真實話機也會在響鈴前拒絕）
                if sdp::negotiate(&sdp::audio_payloads(raw), cfg.codec.payload_type()).is_none() {
                    log.log_event(
                        &account.extension,
                        &format!("[{}] offer 沒有 PCMA / PCMU → 488", short(&call_id)),
                    );
                    fail_invite(
                        raw,
                        &local_to_tag,
                        "488 Not Acceptable Here",
                        488,
                        sock,
                        log,
                        live,
                        detail,
                        traces,
                        &account.extension,
                    )
                    .await;
                    return false;
                }

                // 從 INVITE 解析遠端 From-tag
                let remote_from_tag = SipParser::from_tag(raw).unwrap_or_default();

                // 100 Trying
                let trying = SipMessage::response_with_to_tag(raw, "100 Trying", "");
                log.log_message(Direction::Send, &trying, "server");
                let _ = sock.send(&trying).await;

                // 180 Ringing
                let ringing = SipMessage::response_with_to_tag(raw, "180 Ringing", &local_to_tag);
                log.log_message(Direction::Send, &ringing, "server");
                let _ = sock.send(&ringing).await;

                let outcome = RingOutcome::decide(behavior, rand::random::<f64>() * 100.0);
                let ring = match outcome {
                    RingOutcome::NoAnswer => {
                        Some(Duration::from_secs(AgentBehavior::NO_ANSWER_CAP_SECS))
                    }
                    _ => random_secs(behavior.ring_min_secs, behavior.ring_max_secs),
                };
                match (outcome, ring) {
                    (RingOutcome::Answer, None) => {
                        let ctx = answer_call(
                            raw,
                            &call_id,
                            local_to_tag,
                            remote_from_tag,
                            sock,
                            log,
                            local_addr,
                            local_ip,
                            account,
                            cfg,
                            live,
                            port_counter,
                            rtp_registry,
                            traces,
                        )
                        .await;
                        dlgs.insert(call_id.clone(), ctx);
                    }
                    (outcome, ring) => {
                        log.log_event(
                            &account.extension,
                            &format!(
                                "[{}] 響鈴中（{:?}，{:.1}s）",
                                short(&call_id),
                                outcome,
                                ring.unwrap_or_default().as_secs_f64()
                            ),
                        );
                        pending.insert(
                            call_id.clone(),
                            PendingInvite {
                                invite_raw: raw.to_string(),
                                local_to_tag,
                                remote_from_tag,
                                outcome,
                                due: Instant::now() + ring.unwrap_or_default(),
                            },
                        );
                    }
                }
            }
        }
        "ACK" => {
            let mut dlgs = dialogs.lock().await;
            if let Some(ctx) = dlgs.get_mut(&call_id) {
                ctx.ack_received = true;
            }
        }
        "BYE" => {
            let ok = SipMessage::ok_for_server_bye(raw);
            log.log_message(Direction::Send, &ok, "server");
            let _ = sock.send(&ok).await;

            let mut dlgs = dialogs.lock().await;
            if let Some(mut ctx) = dlgs.remove(&call_id) {
                stop_call_rtp(
                    &call_id,
                    &mut ctx,
                    rtp_registry,
                    live,
                    traces,
                    &account.extension,
                )
                .await;
                detail.record_duration(ctx.answered_at.elapsed().as_secs_f64());
                live.on_completed();
                traces.update(&trace_key(&account.extension, &call_id), |t| {
                    t.ended_at_ms = Some(now_ms())
                });
                log.log_event(
                    &account.extension,
                    &format!("[{}] 通話結束", short(&call_id)),
                );
                return true;
            }
        }
        "CANCEL" if pending.contains_key(&call_id) => {
            // 響鈴中被取消（主叫掛斷或 ACD 逾時改派）：CANCEL 回 200，INVITE 回 487
            let p = pending.remove(&call_id).unwrap();
            let resp = SipMessage::response_for_request(raw, "200 OK");
            log.log_message(Direction::Send, &resp, "server");
            let _ = sock.send(&resp).await;
            log.log_event(
                &account.extension,
                &format!("[{}] 響鈴中被取消 → 487", short(&call_id)),
            );
            fail_invite(
                &p.invite_raw,
                &p.local_to_tag,
                "487 Request Terminated",
                487,
                sock,
                log,
                live,
                detail,
                traces,
                &account.extension,
            )
            .await;
        }
        "CANCEL" => {
            // 不在響鈴中 → INVITE transaction 已完成（已接聽或已回最終回應）：
            // 依 RFC 3261 §9.2，CANCEL 回 200 OK 但不影響已建立的通話（由後續 BYE 結束）；
            // 找不到對應通話則回 481
            let known = dialogs.lock().await.contains_key(&call_id);
            let status = if known {
                "200 OK"
            } else {
                "481 Call/Transaction Does Not Exist"
            };
            let resp = SipMessage::response_for_request(raw, status);
            log.log_message(Direction::Send, &resp, "server");
            let _ = sock.send(&resp).await;
            if known {
                log.log_event(
                    &account.extension,
                    &format!(
                        "[{}] 已接聽後收到 CANCEL，忽略（等待 BYE）",
                        short(&call_id)
                    ),
                );
            }
        }
        "OPTIONS" => {
            let ok = SipMessage::response_with_to_tag(raw, "200 OK", "");
            log.log_message(Direction::Send, &ok, "server");
            let _ = sock.send(&ok).await;
        }
        _ => {
            log.log_event(&account.extension, &format!("忽略未知請求: {}", method));
        }
    }
    false
}

/// 接聽來電：分配 RTP port、送 200 OK + SDP、啟動 RTP，回傳 dialog
#[allow(clippy::too_many_arguments)]
async fn answer_call(
    raw: &str,
    call_id: &str,
    local_to_tag: String,
    remote_from_tag: String,
    sock: &SipSocket,
    log: &SipLogger,
    local_addr: &str,
    local_ip: &str,
    account: &AgentAccount,
    cfg: &Config,
    live: &LiveStats,
    port_counter: &Arc<Mutex<u16>>,
    rtp_registry: &SharedRtpRegistry,
    traces: &TraceLog,
) -> DialogCtx {
    let call_id = call_id.to_string();
    // 分配真實 RTP port（若啟用）
    let (rtp_port, pre_bound) = if cfg.enable_rtp {
        match RtpSession::allocate_port(port_counter, local_ip).await {
            Ok((p, s)) => (p, Some(s)),
            Err(e) => {
                log.log_event(&account.extension, &format!("RTP port 分配失敗: {}", e));
                (pick_dummy_rtp_port(local_ip), None)
            }
        }
    } else {
        (pick_dummy_rtp_port(local_ip), None)
    };

    // 解析 INVITE SDP 中的遠端 RTP 地址
    let sip_ip = crate::net::host(&cfg.server_addr);
    let remote_rtp_addr = SipResponse::sdp_rtp_addr(raw, &sip_ip);

    // 依 offer 選 codec（響鈴前已排除沒有 G.711 的 offer），answer 只宣告選中的那一個
    let offer = sdp::audio_payloads(raw);
    let preferred = cfg.codec.payload_type();
    let codec_pt = sdp::negotiate(&offer, preferred).unwrap_or(preferred);

    // 200 OK + SDP
    let ok = build_response_with_sdp(
        raw,
        "200 OK",
        &local_to_tag,
        local_addr,
        rtp_port,
        &sdp::answer_payloads(codec_pt, &offer),
    );
    log.log_message(Direction::Send, &ok, "server");
    let _ = sock.send(&ok).await;

    let answered_at = Instant::now();
    live.on_answered();
    traces.update(&trace_key(&account.extension, &call_id), |t| {
        t.answered_at_ms = Some(now_ms());
        t.outcome = CallOutcome::Answered;
        t.response_code = Some(200);
    });

    // 啟動 RTP session（若啟用且有遠端地址）
    let rtp_session = if cfg.enable_rtp {
        if let Some(ref remote_addr) = remote_rtp_addr {
            let rtp_cfg = RtpSessionConfig {
                base_port: cfg.rtp_base_port,
                local_ip: local_ip.to_string(),
                remote_addr: remote_addr.clone(),
                audio_file: cfg.audio_file.clone(),
                ssrc: None,
                local_port: Some(rtp_port),
                codec_pt,
            };
            match RtpSession::start(rtp_cfg, Arc::clone(port_counter), pre_bound).await {
                Ok(session) => {
                    // 登記為進行中（用於即時品質聚合）
                    rtp_registry
                        .lock()
                        .await
                        .active
                        .insert(call_id.clone(), Arc::clone(&session.stats));
                    live.on_rtp_start();
                    log.log_event(
                        &account.extension,
                        &format!(
                            "[{}] RTP 啟動 port={} → {}",
                            short(&call_id),
                            rtp_port,
                            remote_addr
                        ),
                    );
                    Some(session)
                }
                Err(e) => {
                    log.log_event(
                        &account.extension,
                        &format!("[{}] RTP 啟動失敗: {}", short(&call_id), e),
                    );
                    None
                }
            }
        } else {
            log.log_event(
                &account.extension,
                &format!("[{}] INVITE SDP 無 RTP 地址，跳過 RTP", short(&call_id)),
            );
            None
        }
    } else {
        None
    };

    let hangup_at = random_secs(
        cfg.agent_behavior.talk_min_secs,
        cfg.agent_behavior.talk_max_secs,
    )
    .map(|d| answered_at + d);
    let ctx = DialogCtx {
        invite_raw: raw.to_string(),
        local_to_tag,
        answered_at,
        remote_from_tag,
        rtp_session,
        ack_received: false,
        ack_timeout_secs: cfg.agent_ack_timeout_secs,
        bye_timeout_secs: cfg.agent_bye_timeout_secs,
        hangup_at,
        codec_pt,
    };
    log.log_event(
        &account.extension,
        &format!("[{}] 接聽來電", short(&call_id)),
    );
    ctx
}

/// 對 INVITE 送最終失敗回應（486 / 487 / 480），計為失敗通話
#[allow(clippy::too_many_arguments)]
async fn fail_invite(
    invite_raw: &str,
    local_to_tag: &str,
    status_line: &str,
    code: u16,
    sock: &SipSocket,
    log: &SipLogger,
    live: &LiveStats,
    detail: &DetailedStats,
    traces: &TraceLog,
    ext: &str,
) {
    let resp = SipMessage::response_with_to_tag(invite_raw, status_line, local_to_tag);
    if let Some(call_id) = SipParser::call_id(invite_raw) {
        traces.update(&trace_key(ext, &call_id), |t| {
            t.outcome = CallOutcome::Failed;
            t.response_code = Some(code);
            t.ended_at_ms = Some(now_ms());
        });
    }
    log.log_message(Direction::Send, &resp, "server");
    let _ = sock.send(&resp).await;
    live.on_failed();
    detail.record_fail_code(code);
}

// ─── 共用：建構回應訊息 ─────────────────────────────────────────────

fn build_response_with_sdp(
    raw_request: &str,
    status_line: &str,
    to_tag: &str,
    local_addr: &str,
    rtp_port: u16,
    payloads: &[u8],
) -> String {
    let (via, from, to, call_id, cseq) = SipMessage::extract_request_headers(raw_request);
    let to_with_tag = crate::sip::message::inject_to_tag_if_missing(&to, to_tag);
    let sdp = sdp::body(&crate::net::host(local_addr), rtp_port, payloads);
    let sdp_len = sdp.len();
    let user_part = from
        .split_once("sip:")
        .map(|(_, s)| s.split('@').next().unwrap_or("agent"))
        .unwrap_or("agent");
    format!(
        "SIP/2.0 {status}\r\n\
         {via}\r\n\
         {from}\r\n\
         {to}\r\n\
         {call_id}\r\n\
         {cseq}\r\n\
         Contact: <sip:{user}@{local};transport={tp}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {sdp_len}\r\n\
         \r\n\
         {sdp}",
        status = status_line,
        tp = SipParser::via_transport(raw_request).to_ascii_lowercase(),
        via = via,
        from = from,
        to = to_with_tag,
        call_id = call_id,
        cseq = cseq,
        user = user_part,
        local = local_addr,
        sdp_len = sdp_len,
        sdp = sdp,
    )
}

/// 從 INVITE 原始訊息建構座席端主動 BYE
fn build_bye_from_dialog(
    invite_raw: &str,
    local_to_tag: &str,
    remote_from_tag: &str,
    local_addr: &str,
    account: &AgentAccount,
) -> String {
    let (_, from, _to, call_id_line, _) = SipMessage::extract_request_headers(invite_raw);
    let call_id_val = call_id_line
        .split_once(':')
        .map(|x| x.1)
        .map(|s| s.trim())
        .unwrap_or("");
    let remote_uri = extract_uri(&from).unwrap_or_default();
    let my_uri = format!("sip:{}@{}", account.extension, local_addr);
    let branch = SipMessage::new_branch();
    let my_tag = local_to_tag;
    // BYE Request-URI = 對方 Contact URI（簡化為 From URI）
    format!(
        "BYE {remote_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{tp} {local};branch={branch}\r\n\
         Max-Forwards: 70\r\n\
         From: <{my_uri}>;tag={my_tag}\r\n\
         To: <{remote_uri}>;tag={remote_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 BYE\r\n\
         Content-Length: 0\r\n\
         \r\n",
        remote_uri = remote_uri,
        tp = SipParser::via_transport(invite_raw),
        local = local_addr,
        branch = branch,
        my_uri = my_uri,
        my_tag = my_tag,
        remote_tag = remote_from_tag,
        call_id = call_id_val,
    )
}

fn extract_uri(header_line: &str) -> Option<String> {
    let start = header_line.find('<')? + 1;
    let end = header_line[start..].find('>')? + start;
    Some(header_line[start..end].to_string())
}

fn pick_dummy_rtp_port(_local_ip: &str) -> u16 {
    use rand::Rng;
    let p = rand::thread_rng().gen_range(16000..32000);
    if p % 2 == 0 {
        p
    } else {
        p + 1
    }
}

fn short(call_id: &str) -> String {
    call_id.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn registered(live: &LiveStats) -> u64 {
        live.agents_registered.load(Ordering::Relaxed)
    }

    #[test]
    fn ring_outcome_follows_percentages() {
        let b = AgentBehavior {
            reject_pct: 20.0,
            no_answer_pct: 30.0,
            ..Default::default()
        };
        assert_eq!(RingOutcome::decide(&b, 0.0), RingOutcome::Reject);
        assert_eq!(RingOutcome::decide(&b, 19.9), RingOutcome::Reject);
        assert_eq!(RingOutcome::decide(&b, 20.0), RingOutcome::NoAnswer);
        assert_eq!(RingOutcome::decide(&b, 49.9), RingOutcome::NoAnswer);
        assert_eq!(RingOutcome::decide(&b, 50.0), RingOutcome::Answer);
        assert_eq!(RingOutcome::decide(&b, 99.99), RingOutcome::Answer);
        // 預設行為一律接聽
        assert_eq!(
            RingOutcome::decide(&AgentBehavior::default(), 0.0),
            RingOutcome::Answer
        );
    }

    #[test]
    fn random_secs_stays_in_range() {
        assert_eq!(random_secs(0.0, 0.0), None);
        assert_eq!(random_secs(2.0, 2.0), Some(Duration::from_secs(2)));
        for _ in 0..100 {
            let d = random_secs(1.0, 3.0).unwrap().as_secs_f64();
            assert!((1.0..=3.0).contains(&d), "{}", d);
        }
    }

    #[test]
    fn dialog_deadline_uses_agent_hangup_only_after_ack() {
        let now = Instant::now();
        let mut ctx = DialogCtx {
            invite_raw: String::new(),
            local_to_tag: String::new(),
            answered_at: now,
            remote_from_tag: String::new(),
            rtp_session: None,
            ack_received: false,
            ack_timeout_secs: 10,
            bye_timeout_secs: 150,
            hangup_at: Some(now + Duration::from_secs(3)),
            codec_pt: sdp::PT_PCMA,
        };
        assert_eq!(
            ctx.deadline(),
            now + Duration::from_secs(10),
            "未收到 ACK 前依 ACK 逾時"
        );
        assert!(!ctx.agent_hangup_due(now + Duration::from_secs(5)));
        ctx.ack_received = true;
        assert_eq!(ctx.deadline(), now + Duration::from_secs(3));
        assert!(ctx.agent_hangup_due(now + Duration::from_secs(3)));
        ctx.hangup_at = Some(now + Duration::from_secs(500));
        assert_eq!(
            ctx.deadline(),
            now + Duration::from_secs(150),
            "BYE 逾時較早時以逾時為準"
        );
    }

    #[test]
    fn gauge_counts_only_state_changes() {
        let live = Arc::new(LiveStats::default());
        let mut a = RegisteredGauge::new(Arc::clone(&live));
        let mut b = RegisteredGauge::new(Arc::clone(&live));
        a.on_ok(Some(240));
        a.on_ok(Some(240)); // 刷新成功不重複計數
        b.on_ok(None);
        assert_eq!(registered(&live), 2);
        a.set(false);
        a.set(false);
        assert_eq!(registered(&live), 1);
        b.on_ok(Some(0)); // Expires=0 → 伺服器已移除註冊
        assert_eq!(registered(&live), 0);
    }

    #[test]
    fn gauge_drop_unregisters() {
        let live = Arc::new(LiveStats::default());
        {
            let mut g = RegisteredGauge::new(Arc::clone(&live));
            g.on_ok(Some(60));
            assert_eq!(registered(&live), 1);
        }
        assert_eq!(
            registered(&live),
            0,
            "runner 結束或被 abort 時應取消註冊計數"
        );
    }

    #[test]
    fn gauge_expired_only_when_registered_and_stale() {
        let live = Arc::new(LiveStats::default());
        let mut g = RegisteredGauge::new(live);
        assert!(!g.expired(0), "未註冊時不算過期");
        g.on_ok(Some(60));
        assert!(!g.expired(60));
        g.last_ok = Instant::now() - Duration::from_secs(61);
        assert!(g.expired(60));
    }
}
