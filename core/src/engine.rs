/// SIP 壓測主引擎
use crate::config::{Config, Transport};
use crate::rtp::{
    session::{RtpSession, RtpSessionConfig},
    stats::{aggregate_snapshots, RtpStatsSnapshot},
};
use crate::sip::{Dialog, DialogState, SharedUdpSocket, SipMessage, SipParser, SipResponse};
use crate::sip_logger::{Direction, SipLogger, SipRole};
use crate::stats::{DetailedStats, FinalReport, LiveStats, StatsSnapshot};
use anyhow::Result;
use rand::Rng;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time;
use tracing::{debug, warn};

pub type ProgressCallback = Arc<dyn Fn(StatsSnapshot, f64) + Send + Sync>;

pub struct Engine {
    config: Config,
    stop_tx: watch::Sender<bool>,
}

/// 內部事件：從接收 task 發給對話管理 task
#[derive(Debug)]
enum SipEvent {
    Response {
        call_id:         String,
        code:            u16,
        to_tag:          Option<String>,
        method:          Option<String>,
        /// 從 200 OK SDP 解析出的對端 RTP 地址（"ip:port"；其餘回應為 None）
        remote_rtp_addr: Option<String>,
        /// 從 200 OK Contact 標頭解析的 URI（ACK/BYE Request-URI 使用）
        contact_uri:     Option<String>,
    },
    /// 收到伺服器主動發來的 SIP 請求（RE-INVITE 刷新 session / 伺服器 BYE 掛斷）
    IncomingRequest {
        call_id: String,
        method:  String,
        /// 原始訊息（用於建構 200 OK 回應，鏡射 Via/From/To/CSeq）
        raw:     String,
    },
}

impl Engine {
    pub fn new(config: Config) -> Self {
        let (stop_tx, _) = watch::channel(false);
        Self { config, stop_tx }
    }

    pub fn stop_handle(&self) -> watch::Sender<bool> {
        self.stop_tx.clone()
    }

    pub async fn run(
        &self,
        on_progress: Option<ProgressCallback>,
    ) -> Result<FinalReport> {
        let cfg    = &self.config;
        let live   = Arc::new(LiveStats::default());
        let detail = Arc::new(DetailedStats::default());
        let start  = Instant::now();

        cfg.validate()?;

        // ── TCP transport 目前尚未實作 ──
        if cfg.transport == Transport::Tcp {
            return Err(anyhow::anyhow!(
                "TCP transport 尚未實作，請使用 UDP（--transport udp）"
            ));
        }

        // ── 建立 SIP log 記錄器 ──
        let sip_log = Arc::new(
            SipLogger::new(&cfg.logs_dir, SipRole::Agent).unwrap_or_else(|e| {
                warn!("無法建立 SIP log：{}，改寫至系統暫存目錄", e);
                let tmp = std::env::temp_dir();
                let tmp_str = tmp.to_string_lossy();
                SipLogger::new(tmp_str.as_ref(), SipRole::Agent)
                    .expect("無法在系統暫存目錄建立 SIP log")
            }),
        );
        tracing::info!("SIP log → {}", sip_log.path.display());

        // 解析 server 地址
        let server_addr: SocketAddr = cfg.server_addr.parse()?;

        // 本機 IP：透過 connect() 探測真實出口 IP，禁止用 0.0.0.0
        let local_ip_owned: String = match cfg.local_addr.as_deref() {
            Some(ip) => ip.to_string(),
            None => {
                let probe = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
                probe.connect(&cfg.server_addr).await?;
                probe.local_addr()?.ip().to_string()
            }
        };
        let local_ip = local_ip_owned.as_str();

        // 建立共用 UDP socket
        let udp        = Arc::new(SharedUdpSocket::new(server_addr, local_ip).await?);
        let local_addr = udp.local_addr.clone();

        let local_domain_owned = cfg.local_domain.clone().unwrap_or_else(|| {
            local_addr.split(':').next().unwrap_or("127.0.0.1").to_string()
        });
        let local_domain = local_domain_owned.as_str();

        // 對話表（Call-ID → Dialog）
        let dialogs: Arc<Mutex<HashMap<String, Dialog>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // RTP session 表（Call-ID → RtpSession）
        let rtp_sessions: Arc<Mutex<HashMap<String, RtpSession>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // RTP port 分配計數器
        let rtp_port_counter = Arc::new(Mutex::new(
            if cfg.rtp_base_port % 2 == 0 { cfg.rtp_base_port }
            else { cfg.rtp_base_port + 1 },
        ));

        // 預分配但尚未啟動的 RTP socket（Call-ID → 已綁定 UdpSocket）
        // 解決 TOCTOU：INVITE 前綁定，200 OK 後移交 RtpSession
        let pre_bound_rtp: Arc<Mutex<HashMap<String, TokioUdpSocket>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // 累計 RTP 統計
        let rtp_snapshots: Arc<Mutex<Vec<RtpStatsSnapshot>>> =
            Arc::new(Mutex::new(Vec::new()));

        // 事件 channel
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<SipEvent>();

        // ── Task 1：接收 SIP 回應 ──
        let udp_recv  = Arc::clone(&udp);
        let ev_tx2    = ev_tx.clone();
        let log_recv  = Arc::clone(&sip_log);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let n = match udp_recv.socket.recv(&mut buf).await {
                    Ok(n)  => n,
                    Err(_) => break,
                };
                let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
                debug!("< {}", raw.lines().next().unwrap_or(""));

                log_recv.log_message(Direction::Recv, &raw, &udp_recv.server.to_string());

                // SipParser 支援 compact form（"i:" = Call-ID）
                let call_id = SipParser::call_id(&raw);

                if raw.starts_with("SIP/2.0") {
                    // ── SIP 回應 ──────────────────────────────────────────
                    let code            = SipParser::status_code(&raw);
                    let to_tag          = SipParser::to_tag(&raw);
                    let method          = SipParser::cseq_method(&raw);
                    // 取完整 "ip:port"（從 SDP c= + m= 行），fallback 用 SIP server IP
                    let remote_rtp_addr = if code == Some(200) && method.as_deref() == Some("INVITE") {
                        let sip_ip = udp_recv.server.ip().to_string();
                        SipResponse::sdp_rtp_addr(&raw, &sip_ip)
                    } else {
                        None
                    };
                    let contact_uri = if code == Some(200) { SipParser::contact_uri(&raw) } else { None };

                    // INVITE 的非 2xx 最終回應（含重傳、CANCEL 後的 487）一律在此回 ACK：
                    // 屬於 transaction 層行為，不依賴 dialog 是否仍存在
                    if method.as_deref() == Some("INVITE") && code.is_some_and(|c| c >= 300) {
                        let ack = SipMessage::ack_for_non2xx(&raw);
                        log_recv.log_message(Direction::Send, &ack, &udp_recv.server.to_string());
                        let _ = udp_recv.send(&ack).await;
                    }

                    if let (Some(call_id), Some(code)) = (call_id, code) {
                        let _ = ev_tx2.send(SipEvent::Response {
                            call_id, code, to_tag, method, remote_rtp_addr, contact_uri,
                        });
                    }
                } else {
                    // ── SIP 請求（伺服器主動送來：RE-INVITE / BYE 等）──────
                    let method_str = raw.lines().next()
                        .and_then(|l| l.split_whitespace().next())
                        .map(|s| s.to_uppercase())
                        .unwrap_or_default();

                    if method_str == "INVITE" || method_str == "BYE" {
                        if let Some(call_id) = call_id {
                            let _ = ev_tx2.send(SipEvent::IncomingRequest {
                                call_id, method: method_str, raw,
                            });
                        }
                    }
                }
            }
        });

        // ── Task 2：進度回報（含即時 RTP 品質） ──
        let engine_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Some(ref cb) = on_progress {
            let cb        = Arc::clone(cb);
            let live      = Arc::clone(&live);
            let rtp_sess  = Arc::clone(&rtp_sessions);
            let finished  = Arc::clone(&engine_finished);
            let duration  = cfg.duration_secs as f64;
            let unlimited = cfg.duration_secs == 0;
            tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(1));
                loop {
                    interval.tick().await;
                    let elapsed  = start.elapsed().as_secs_f64();
                    let progress = if unlimited { 0.0 } else { (elapsed / duration).min(1.0) };
                    let mut snap = live.snapshot();

                    {
                        let sessions = rtp_sess.lock().await;
                        let snaps: Vec<_> = sessions.values().map(|s| s.stats.snapshot()).collect();
                        if let Some(agg) = aggregate_snapshots(&snaps) {
                            snap.rtp_mos          = Some(agg.mos);
                            snap.rtp_loss_pct     = Some(agg.loss_rate_pct);
                            snap.rtp_jitter_ms    = Some(agg.jitter_ms);
                            snap.rtp_sent_packets = Some(agg.sent_packets);
                            snap.rtp_recv_packets = Some(agg.recv_packets);
                        }
                    }
                    snap.finished = finished.load(std::sync::atomic::Ordering::Relaxed);
                    let done = snap.finished;
                    cb(snap, progress);
                    if !unlimited && elapsed >= duration { break; }
                    if done { break; }
                }
            });
        }

        // ── 主控迴圈 ──
        let cps_interval   = Duration::from_secs_f64(1.0 / cfg.cps);
        let unlimited_time = cfg.duration_secs == 0;
        // None = 不限時（只靠手動停止或 max_total_calls 結束）
        let deadline: Option<Instant> = if unlimited_time {
            None
        } else {
            Some(start + cfg.duration())
        };
        let invite_to = cfg.invite_timeout();
        let call_dur  = Duration::from_secs(cfg.call_duration_secs);
        let stop_rx = self.stop_tx.subscribe();
        let mut stopped = false;
        // 收尾階段：已對所有通話送出 BYE/CANCEL，繼續處理回應直到全部結束或逾時
        let mut draining_until: Option<Instant> = None;

        let mut next_call = Instant::now();

        loop {
            let now = Instant::now();

            // 檢查外部停止訊號
            if !stopped && stop_rx.has_changed().unwrap_or(false) {
                stopped = true;
                sip_log.log_event("ENGINE", "收到停止訊號，開始 graceful shutdown");
            }

            // ① 處理所有收到的 SIP 事件（回應 / 伺服器請求）
            while let Ok(ev) = ev_rx.try_recv() {
                match ev {
                SipEvent::Response { call_id, code, to_tag, method, remote_rtp_addr, contact_uri } => {
                // 先取出預分配的 RTP socket（若有），再鎖 dialogs，消除 TOCTOU
                let pre_socket = if cfg.enable_rtp && code == 200 && method.as_deref() == Some("INVITE") {
                    pre_bound_rtp.lock().await.remove(&call_id)
                } else {
                    None
                };

                let mut dialogs = dialogs.lock().await;

                if let Some(dialog) = dialogs.get_mut(&call_id) {
                    match code {
                        100 => dialog.on_trying(),
                        180 | 183 => {
                            dialog.on_ringing();
                            if let Some(pdd) = dialog.pdd_ms() {
                                detail.record_pdd(pdd);
                            }
                        }
                        200 if method.as_deref() == Some("INVITE") => {
                            let first_answer = matches!(
                                dialog.state,
                                DialogState::Calling | DialogState::Trying | DialogState::Ringing
                            );
                            if !first_answer {
                                // 200 OK 重傳（對端沒收到 ACK）→ 只重送 ACK，不重複計數 / 啟動 RTP
                                if dialog.state == DialogState::Connected {
                                    let ack = SipMessage::ack(
                                        &dialog.call_id,
                                        &cfg.caller_number,
                                        local_domain,
                                        &dialog.callee,
                                        dialog.to_tag.as_deref().unwrap_or(""),
                                        &cfg.server_addr,
                                        &local_addr,
                                        dialog.cseq,
                                        &SipMessage::new_branch(),
                                        &dialog.from_tag,
                                        "UDP",
                                        dialog.remote_contact.as_deref(),
                                    );
                                    sip_log.log_message(Direction::Send, &ack, &cfg.server_addr);
                                    let _ = udp.send(&ack).await;
                                }
                                continue;
                            }
                            let tag = to_tag.unwrap_or_default();
                            dialog.on_ok(tag.clone());
                            // 儲存對端 Contact URI（BYE/ACK Request-URI 使用）
                            dialog.remote_contact = contact_uri.clone();
                            live.on_answered();

                            if let Some(ms) = dialog.setup_time_ms() {
                                detail.record_setup(ms);
                            }

                            // 啟動 RTP session
                            if cfg.enable_rtp {
                                let pure_ip    = local_ip.split(':').next().unwrap_or(local_ip).to_string();
                                let server_ip  = cfg.server_addr.split(':').next().unwrap_or("127.0.0.1");
                                // SDP c= + m= 解析出完整 "ip:port"；fallback 為 SIP server IP:16384
                                let remote_addr = remote_rtp_addr
                                    .clone()
                                    .unwrap_or_else(|| format!("{}:16384", server_ip));
                                let local_pre  = dialog.local_rtp_port;
                                let audio_path = cfg.audio_file.clone();

                                let rtp_info = format!(
                                    "RTP START local={}:{} remote={} audio={:?} sdp_parsed={}",
                                    pure_ip,
                                    if local_pre > 0 { local_pre } else { cfg.rtp_base_port },
                                    remote_addr,
                                    audio_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "silence".into()),
                                    remote_rtp_addr.is_some(),
                                );
                                sip_log.log_event(&call_id, &rtp_info);
                                tracing::info!("{}", rtp_info);

                                let rtp_cfg = RtpSessionConfig {
                                    base_port:   cfg.rtp_base_port,
                                    local_ip:    pure_ip,
                                    remote_addr,
                                    audio_file:  audio_path,
                                    ssrc:        None,
                                    local_port:  if local_pre > 0 { Some(local_pre) } else { None },
                                };
                                // 同步啟動並登記：確保之後處理的 BYE 一定找得到這個 session
                                let pc = Arc::clone(&rtp_port_counter);
                                match RtpSession::start(rtp_cfg, pc, pre_socket).await {
                                    Ok(session) => {
                                        live.on_rtp_start();
                                        rtp_sessions.lock().await.insert(call_id.clone(), session);
                                    }
                                    Err(e) => {
                                        let msg = format!("RTP session 啟動失敗: {}", e);
                                        warn!("{}", msg);
                                        sip_log.log_event(&call_id, &msg);
                                    }
                                }
                            }

                            // 送 ACK（2xx ACK 使用 Contact URI 作為 Request-URI）
                            let ack = SipMessage::ack(
                                &dialog.call_id,
                                &cfg.caller_number,
                                local_domain,
                                &dialog.callee,
                                &tag,
                                &cfg.server_addr,
                                &local_addr,
                                dialog.cseq,
                                &SipMessage::new_branch(),
                                &dialog.from_tag,
                                "UDP",
                                dialog.remote_contact.as_deref(),
                            );
                            let udp_ack  = Arc::clone(&udp);
                            let log_ack  = Arc::clone(&sip_log);
                            let server   = cfg.server_addr.clone();
                            let ack_copy = ack.clone();
                            tokio::spawn(async move {
                                log_ack.log_message(Direction::Send, &ack_copy, &server);
                                let _ = udp_ack.send(&ack_copy).await;
                            });
                        }
                        // BYE 的任何最終回應（200，或 481/408 等錯誤）都代表 dialog 已結束
                        200..=699 if method.as_deref() == Some("BYE")
                            && dialog.state == DialogState::Terminating => {
                            if let Some(a) = dialog.answered_at {
                                detail.record_duration(a.elapsed().as_secs_f64());
                            }
                            if cfg.enable_rtp {
                                stop_rtp_session(&call_id, &rtp_sessions, &rtp_snapshots, &live).await;
                            }
                            dialog.on_bye_ok();
                            live.on_completed();
                        }
                        // INVITE 被拒（3xx–6xx）：ACK 已由接收 task 送出，這裡只做統計；
                        // 只在早期狀態處理一次，避免重傳的錯誤回應重複計數
                        300..=699 if method.as_deref() == Some("INVITE")
                            && matches!(dialog.state,
                                DialogState::Calling | DialogState::Trying | DialogState::Ringing) => {
                            // 清理預分配但未使用的 RTP socket
                            if cfg.enable_rtp {
                                pre_bound_rtp.lock().await.remove(&call_id);
                            }
                            detail.record_fail_code(code);
                            dialog.on_error(code);
                            live.on_failed();
                        }
                        _ => {}
                    }
                }
                } // SipEvent::Response
                SipEvent::IncomingRequest { call_id, method, raw: req_raw } => {
                    let mut dialogs = dialogs.lock().await;
                    if let Some(dialog) = dialogs.get_mut(&call_id) {
                        match method.as_str() {
                            "INVITE" => {
                                // RE-INVITE（Session-Expires 刷新）→ 200 OK 保持通話
                                sip_log.log_event(&call_id, "收到 RE-INVITE（Session-Expires），回應 200 OK");
                                let local_port = if dialog.local_rtp_port > 0 {
                                    dialog.local_rtp_port
                                } else {
                                    cfg.rtp_base_port
                                };
                                let ok      = SipMessage::ok_for_server_reinvite(
                                    &req_raw, &local_addr, local_port, &cfg.caller_number,
                                );
                                let udp_ok  = Arc::clone(&udp);
                                let log_ok  = Arc::clone(&sip_log);
                                let server  = cfg.server_addr.clone();
                                let ok_copy = ok.clone();
                                tokio::spawn(async move {
                                    log_ok.log_message(Direction::Send, &ok_copy, &server);
                                    let _ = udp_ok.send(&ok_copy).await;
                                });

                                // 解析 RE-INVITE SDP 中的新 RTP 地址，更新 RTP session 目標
                                if cfg.enable_rtp {
                                    let sip_ip = cfg.server_addr.split(':').next().unwrap_or("127.0.0.1");
                                    if let Some(new_rtp_addr) = SipResponse::sdp_rtp_addr(&req_raw, sip_ip) {
                                        let sessions = rtp_sessions.lock().await;
                                        if let Some(session) = sessions.get(&call_id) {
                                            match session.update_remote(&new_rtp_addr).await {
                                                Ok(()) => sip_log.log_event(&call_id,
                                                    &format!("RE-INVITE RTP 目標更新 → {}", new_rtp_addr)),
                                                Err(e) => sip_log.log_event(&call_id,
                                                    &format!("RE-INVITE RTP 更新失敗: {}", e)),
                                            }
                                        }
                                    }
                                }
                            }
                            "BYE" => {
                                // 伺服器主動掛斷 → 200 OK 並結束本通話
                                sip_log.log_event(&call_id, "收到伺服器 BYE，回應 200 OK");
                                let ok      = SipMessage::ok_for_server_bye(&req_raw);
                                let udp_ok  = Arc::clone(&udp);
                                let log_ok  = Arc::clone(&sip_log);
                                let server  = cfg.server_addr.clone();
                                let ok_copy = ok.clone();
                                tokio::spawn(async move {
                                    log_ok.log_message(Direction::Send, &ok_copy, &server);
                                    let _ = udp_ok.send(&ok_copy).await;
                                });

                                if matches!(dialog.state, DialogState::Connected | DialogState::Terminating) {
                                    if let Some(a) = dialog.answered_at {
                                        detail.record_duration(a.elapsed().as_secs_f64());
                                    }
                                }
                                if cfg.enable_rtp && matches!(dialog.state, DialogState::Connected | DialogState::Terminating) {
                                    stop_rtp_session(&call_id, &rtp_sessions, &rtp_snapshots, &live).await;
                                }

                                if matches!(dialog.state, DialogState::Connected | DialogState::Terminating) {
                                    dialog.on_bye_ok();
                                    live.on_completed();
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // 找不到對應 dialog（已結束或未知）仍須回最終回應，否則伺服器會持續重傳
                        // 對話內請求 → 481（RFC 3261 §12.2.2 / §15.1.2）；Caller 模式不接受新來電 → 603
                        let status = if method == "INVITE" && SipParser::to_tag(&req_raw).is_none() {
                            "603 Decline"
                        } else {
                            "481 Call/Transaction Does Not Exist"
                        };
                        sip_log.log_event(&call_id, &format!("收到未知 dialog 的 {}，回應 {}", method, status));
                        let resp = SipMessage::response_for_request(&req_raw, status);
                        sip_log.log_message(Direction::Send, &resp, &cfg.server_addr);
                        let _ = udp.send(&resp).await;
                    }
                } // SipEvent::IncomingRequest
                } // match ev
            }

            // ② 掃描逾時 & 應該 BYE 的通話
            {
                let mut dialogs = dialogs.lock().await;
                let mut to_send: Vec<String> = Vec::new();

                for dialog in dialogs.values_mut() {
                    match &dialog.state {
                        // INVITE 逾時 → 發 CANCEL
                        DialogState::Calling | DialogState::Trying | DialogState::Ringing => {
                            if now.duration_since(dialog.invite_sent_at) > invite_to {
                                sip_log.log_event(&dialog.call_id, "TIMEOUT — 發送 CANCEL");
                                let cancel = SipMessage::cancel(
                                    &dialog.call_id,
                                    &cfg.caller_number,
                                    local_domain,
                                    &dialog.callee,
                                    &cfg.server_addr,
                                    &local_addr,
                                    dialog.cseq,
                                    &dialog.branch,
                                    &dialog.from_tag,
                                    "UDP",
                                );
                                to_send.push(cancel);
                                dialog.on_timeout();
                                live.on_timeout();
                                if cfg.enable_rtp {
                                    pre_bound_rtp.lock().await.remove(&dialog.call_id);
                                }
                            }
                        }
                        // 通話時長到 → 發 BYE（使用 Contact URI 作為 Request-URI）
                        DialogState::Connected => {
                            if let Some(ans) = dialog.answered_at {
                                if now.duration_since(ans) >= call_dur && call_dur.as_secs() > 0 {
                                    let bye_branch = SipMessage::new_branch();
                                    let bye = SipMessage::bye(
                                        &dialog.call_id,
                                        &cfg.caller_number,
                                        local_domain,
                                        &dialog.callee,
                                        dialog.to_tag.as_deref().unwrap_or(""),
                                        &cfg.server_addr,
                                        &local_addr,
                                        dialog.cseq + 1,
                                        &bye_branch,
                                        &dialog.from_tag,
                                        "UDP",
                                        dialog.remote_contact.as_deref(),
                                    );
                                    dialog.on_bye_sent();
                                    to_send.push(bye);
                                }
                            }
                        }
                        // BYE 逾時（對端未回 200 OK）→ 強制結束通話
                        DialogState::Terminating => {
                            if let Some(bye_sent) = dialog.bye_sent_at {
                                if now.duration_since(bye_sent) > invite_to {
                                    sip_log.log_event(&dialog.call_id, "BYE TIMEOUT — 強制結束通話");
                                    if cfg.enable_rtp {
                                        stop_rtp_session(&dialog.call_id, &rtp_sessions, &rtp_snapshots, &live).await;
                                    }
                                    if let Some(a) = dialog.answered_at {
                                        detail.record_duration(a.elapsed().as_secs_f64());
                                    }
                                    dialog.on_bye_ok(); // 強制標記為 Completed
                                    live.on_completed();
                                }
                            }
                        }
                        _ => {}
                    }
                }

                for msg in to_send {
                    let udp_s   = Arc::clone(&udp);
                    let log_bye = Arc::clone(&sip_log);
                    let server  = cfg.server_addr.clone();
                    let msg_log = msg.clone();
                    tokio::spawn(async move {
                        log_bye.log_message(Direction::Send, &msg_log, &server);
                        let _ = udp_s.send(&msg_log).await;
                    });
                }

                // 清理已結束的 dialog
                dialogs.retain(|_, d| matches!(
                    d.state,
                    DialogState::Calling  | DialogState::Trying |
                    DialogState::Ringing  | DialogState::Connected |
                    DialogState::Terminating
                ));
            }

            // ③ 發起新通話（停止中不再發起）
            let total_limit_reached = cfg.max_total_calls
                .map_or(false, |max| live.calls_initiated.load(std::sync::atomic::Ordering::Relaxed) >= max);

            let deadline_reached = deadline.is_some_and(|d| now >= d);

            if draining_until.is_none() && !stopped && !total_limit_reached
                && !deadline_reached && now >= next_call {
                let concurrent = {
                    let dialogs = dialogs.lock().await;
                    dialogs.values().filter(|d| matches!(
                        d.state,
                        DialogState::Calling | DialogState::Trying |
                        DialogState::Ringing | DialogState::Connected
                    )).count()
                };

                if concurrent < cfg.max_concurrent_calls {
                    let callee = match &cfg.callee_fixed {
                        Some(num) if !num.is_empty() => num.clone(),
                        _ => format!(
                            "{}{}",
                            cfg.callee_prefix,
                            rand::thread_rng().gen_range(0..=cfg.callee_range)
                        ),
                    };

                    let call_id  = SipMessage::new_call_id(local_domain);
                    let from_tag = SipMessage::new_tag();
                    let branch   = SipMessage::new_branch();

                    // 預分配 RTP port（保持 socket 綁定以避免 TOCTOU）
                    let (rtp_port, pre_socket) = if cfg.enable_rtp {
                        let pc = Arc::clone(&rtp_port_counter);
                        let ip = local_ip.split(':').next().unwrap_or(local_ip).to_string();
                        match RtpSession::allocate_port(&pc, &ip).await {
                            Ok((p, sock)) => (p, Some(sock)),
                            Err(e) => {
                                warn!("RTP port 預分配失敗: {}", e);
                                (cfg.rtp_base_port, None)
                            }
                        }
                    } else {
                        (0, None)
                    };

                    // 儲存預分配的 socket
                    if let Some(sock) = pre_socket {
                        pre_bound_rtp.lock().await.insert(call_id.clone(), sock);
                    }

                    let invite = SipMessage::invite(
                        &call_id,
                        &cfg.caller_number,
                        local_domain,
                        &callee,
                        &cfg.server_addr,
                        &local_addr,
                        1,
                        &branch,
                        &from_tag,
                        "UDP",
                        if rtp_port > 0 { rtp_port } else { 9 },
                    );

                    let dialog = Dialog::new(call_id.clone(), from_tag, branch, callee, rtp_port);
                    dialogs.lock().await.insert(call_id, dialog);
                    live.on_invite();

                    let udp_inv    = Arc::clone(&udp);
                    let log_invite = Arc::clone(&sip_log);
                    let server     = cfg.server_addr.clone();
                    let inv_log    = invite.clone();
                    tokio::spawn(async move {
                        log_invite.log_message(Direction::Send, &inv_log, &server);
                        let _ = udp_inv.send(&inv_log).await;
                    });

                    next_call = now + cps_interval;
                }
            }

            // ④ 測試結束條件（含手動停止）
            let all_done = total_limit_reached
                && live.calls_active.load(std::sync::atomic::Ordering::Relaxed) <= 0;

            if draining_until.is_none() && (deadline_reached || all_done || stopped) {
                // 對所有進行中通話發 BYE / 未接通的發 CANCEL
                {
                    let mut dialogs = dialogs.lock().await;
                    let mut to_send: Vec<String> = Vec::new();
                    let local_domain = local_domain_owned.as_str();

                    for dialog in dialogs.values_mut() {
                        match &dialog.state {
                            DialogState::Calling | DialogState::Trying | DialogState::Ringing => {
                                let cancel = SipMessage::cancel(
                                    &dialog.call_id,
                                    &cfg.caller_number,
                                    local_domain,
                                    &dialog.callee,
                                    &cfg.server_addr,
                                    &local_addr,
                                    dialog.cseq,
                                    &dialog.branch,
                                    &dialog.from_tag,
                                    "UDP",
                                );
                                to_send.push(cancel);
                                dialog.on_timeout();
                                live.on_timeout();
                                if cfg.enable_rtp {
                                    pre_bound_rtp.lock().await.remove(&dialog.call_id);
                                }
                            }
                            DialogState::Connected => {
                                let bye_branch = SipMessage::new_branch();
                                let bye = SipMessage::bye(
                                    &dialog.call_id,
                                    &cfg.caller_number,
                                    local_domain,
                                    &dialog.callee,
                                    dialog.to_tag.as_deref().unwrap_or(""),
                                    &cfg.server_addr,
                                    &local_addr,
                                    dialog.cseq + 1,
                                    &bye_branch,
                                    &dialog.from_tag,
                                    "UDP",
                                    dialog.remote_contact.as_deref(),
                                );
                                to_send.push(bye);
                                dialog.on_bye_sent();
                            }
                            _ => {}
                        }
                    }

                    for msg in to_send {
                        let udp_s   = Arc::clone(&udp);
                        let log_s   = Arc::clone(&sip_log);
                        let server  = cfg.server_addr.clone();
                        let msg_log = msg.clone();
                        tokio::spawn(async move {
                            log_s.log_message(Direction::Send, &msg_log, &server);
                            let _ = udp_s.send(&msg_log).await;
                        });
                    }
                }

                // 進入收尾：主迴圈繼續處理 BYE 的 200 OK（最多 invite_timeout 秒）
                draining_until = Some(Instant::now() + invite_to);
            }

            if let Some(until) = draining_until {
                let active = live.calls_active.load(std::sync::atomic::Ordering::Relaxed);
                if active <= 0 || now >= until { break; }
            }

            time::sleep(Duration::from_micros(500)).await;
        }

        engine_finished.store(true, std::sync::atomic::Ordering::Relaxed);

        // ── 產生最終報告 ──
        let snap    = live.snapshot();
        let elapsed = start.elapsed().as_secs_f64();

        sip_log.log_summary(&format!(
            "發起={} 接通={} 完成={} 失敗={} 逾時={} ASR={:.1}% 時長={:.1}s",
            snap.calls_initiated, snap.calls_answered, snap.calls_completed,
            snap.calls_failed, snap.calls_timeout, snap.asr, elapsed,
        ));

        let rtp_agg = if cfg.enable_rtp {
            {
                let mut sessions = rtp_sessions.lock().await;
                let mut snaps    = rtp_snapshots.lock().await;
                for (_, s) in sessions.drain() { snaps.push(s.stop()); }
            }
            let snaps = rtp_snapshots.lock().await;
            aggregate_snapshots(&snaps)
        } else {
            None
        };

        // HDR histogram 數據（std::sync::MutexGuard，不可跨 await）
        let us_to_ms = |h: &hdrhistogram::Histogram<u64>, q: f64| {
            h.value_at_quantile(q) as f64 / 1000.0
        };
        let (pdd_p50, pdd_p95, pdd_p99, pdd_max,
             setup_p50, setup_p95, setup_p99, setup_max,
             acd_secs) = {
            let pdd_h   = detail.pdd_hist.lock().unwrap();
            let setup_h = detail.setup_hist.lock().unwrap();
            let dur_h   = detail.dur_hist.lock().unwrap();
            let acd = if snap.calls_completed > 0 { dur_h.mean() / 1000.0 } else { 0.0 };
            (
                us_to_ms(&pdd_h, 0.50), us_to_ms(&pdd_h, 0.95),
                us_to_ms(&pdd_h, 0.99), pdd_h.max() as f64 / 1000.0,
                us_to_ms(&setup_h, 0.50), us_to_ms(&setup_h, 0.95),
                us_to_ms(&setup_h, 0.99), setup_h.max() as f64 / 1000.0,
                acd,
            )
        };

        let fail_codes = detail.fail_codes.lock().unwrap().clone();

        Ok(FinalReport {
            calls_initiated: snap.calls_initiated,
            calls_answered:  snap.calls_answered,
            calls_completed: snap.calls_completed,
            calls_failed:    snap.calls_failed,
            calls_timeout:   snap.calls_timeout,
            duration_secs:   elapsed,
            asr:             snap.asr,
            ccr: if snap.calls_initiated > 0 {
                snap.calls_completed as f64 / snap.calls_initiated as f64 * 100.0
            } else { 0.0 },
            actual_cps:   snap.calls_initiated as f64 / elapsed,
            pdd_p50_ms:   pdd_p50,
            pdd_p95_ms:   pdd_p95,
            pdd_p99_ms:   pdd_p99,
            pdd_max_ms:   pdd_max,
            setup_p50_ms: setup_p50,
            setup_p95_ms: setup_p95,
            setup_p99_ms: setup_p99,
            setup_max_ms: setup_max,
            acd_secs,
            fail_4xx: detail.fail_4xx.load(std::sync::atomic::Ordering::Relaxed),
            fail_5xx: detail.fail_5xx.load(std::sync::atomic::Ordering::Relaxed),
            fail_6xx: detail.fail_6xx.load(std::sync::atomic::Ordering::Relaxed),
            fail_codes,
            mos:              rtp_agg.as_ref().map(|a| a.mos),
            loss_rate_pct:    rtp_agg.as_ref().map(|a| a.loss_rate_pct),
            jitter_ms:        rtp_agg.as_ref().map(|a| a.jitter_ms),
            rtp_sent:         rtp_agg.as_ref().map(|a| a.sent_packets),
            rtp_recv:         rtp_agg.as_ref().map(|a| a.recv_packets),
            rtp_out_of_order: rtp_agg.as_ref().map(|a| a.out_of_order),
        })
    }
}

/// 停止並移除指定通話的 RTP session，將最終統計併入累計快照
async fn stop_rtp_session(
    call_id:   &str,
    sessions:  &Mutex<HashMap<String, RtpSession>>,
    snapshots: &Mutex<Vec<RtpStatsSnapshot>>,
    live:      &LiveStats,
) {
    let session = sessions.lock().await.remove(call_id);
    if let Some(session) = session {
        let snap = session.stop();
        live.on_rtp_stop();
        snapshots.lock().await.push(snap);
    }
}
