//! 單一座席帳號的 runner：一個 socket、一個註冊 dialog、進行中的通話與響鈴中的來電
//!
//! 每個帳號只有一個 task 存取這些狀態，所以都不需要上鎖。主迴圈等待下列事件之一，
//! 交給對應的方法處理：收到 SIP 訊息、註冊刷新、REGISTER 重傳、響鈴 / 通話計時器、
//! 座席流程（撥碼 / 等待）計時器、手動撥碼、停止訊號。
use super::call::{
    build_bye_from_dialog, build_response_with_sdp, pick_dummy_rtp_port, random_secs, short,
    DialogCtx, PendingInvite, RingOutcome,
};
use super::flow::{DialState, Endpoint, FlowQueue, Next, OutDial};
use super::register::{Registration, INITIAL_EXPIRES, MAX_AUTH_RETRIES};
use super::{trace_key, Shared};
use crate::config::{AgentAccount, AgentBehavior, FlowStep};
use crate::rtp::session::{RtpSession, RtpSessionConfig};
use crate::sip::{
    register::{DigestChallenge, RegisterMessage},
    sdp, SipMessage, SipParser, SipResponse, SipSocket,
};
use crate::sip_logger::Direction;
use crate::trace::{now_ms, CallOutcome, CallTrace};
use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tokio::time;

pub(super) struct AccountRunner {
    sh: Arc<Shared>,
    account: AgentAccount,
    sock: SipSocket,
    /// log 中標示的對端（SIP server 位址）
    peer: String,
    local_addr: String,
    local_ip: String,
    reg: Registration,
    /// 已接聽的通話（Call-ID → 狀態）
    dialogs: HashMap<String, DialogCtx>,
    /// 響鈴中（尚未接聽 / 拒接）的來電
    pending: HashMap<String, PendingInvite>,
    /// wrap-up：此時間之前的新來電一律 486
    wrap_until: Option<Instant>,
    /// 座席流程（撥碼 / 等待）
    flow: FlowQueue,
    /// 已執行過註冊後的流程（只在第一次註冊成功時執行）
    flow_started: bool,
    /// 這個帳號在分批註冊中的延遲；結束流程與手動撥碼也依此錯開
    start_delay: Duration,
    /// 收到停止訊號、正在執行結束流程：此時間之後不論流程是否完成都解除註冊
    stopping_until: Option<Instant>,
}

/// 沒有排程時的等待上限
fn far_future() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(3600)
}

impl AccountRunner {
    pub async fn connect(
        sh: Arc<Shared>,
        account: AgentAccount,
        server: SocketAddr,
    ) -> Result<Self> {
        let sock = SipSocket::connect(
            sh.cfg.transport,
            server,
            crate::net::unspecified_for(&server),
        )
        .await?;
        let local_addr = sock.local_addr.clone();
        let local_ip = crate::net::host(&local_addr);
        let domain = if !account.domain.is_empty() {
            account.domain.clone()
        } else {
            server.ip().to_string()
        };
        let reg = Registration::new(
            SipMessage::new_tag(),
            SipMessage::new_call_id(&domain),
            Arc::clone(&sh.live),
        );
        Ok(Self {
            sh,
            account,
            sock,
            peer: server.to_string(),
            local_addr,
            local_ip,
            reg,
            dialogs: HashMap::new(),
            pending: HashMap::new(),
            wrap_until: None,
            flow: FlowQueue::default(),
            flow_started: false,
            start_delay: Duration::ZERO,
            stopping_until: None,
        })
    }

    /// `start_delay`：延後多久才送第一個 REGISTER（分批註冊）；等待中收到停止訊號就直接結束。
    /// `manual`：手動撥碼（GUI 的登入 / 就緒等按鈕），每則訊息是一串要執行的步驟
    pub async fn run(
        mut self,
        mut stop: watch::Receiver<bool>,
        start_delay: Duration,
        manual: broadcast::Receiver<Vec<FlowStep>>,
    ) -> Result<()> {
        self.start_delay = start_delay;
        let mut manual = Some(manual);
        if !start_delay.is_zero() {
            tokio::select! {
                _ = time::sleep(start_delay) => {}
                _ = stop.changed() => return Ok(()),
            }
        }
        self.event("開始 REGISTER");
        if let Err(e) = self.send_register(INITIAL_EXPIRES).await {
            self.event(&format!("初始 REGISTER 送出失敗: {}", e));
            return Err(e);
        }

        loop {
            let refresh_at = tokio::time::Instant::from_std(self.reg.refresh_at());
            let retx_at = self
                .reg
                .retx_at()
                .map(tokio::time::Instant::from_std)
                .unwrap_or_else(far_future);
            let call_timer = self
                .next_call_deadline()
                .map(tokio::time::Instant::from_std)
                .unwrap_or_else(far_future);
            let flow_timer = self
                .flow
                .deadline()
                .into_iter()
                .chain(self.stopping_until)
                .min()
                .map(tokio::time::Instant::from_std)
                .unwrap_or_else(far_future);
            let manual_steps = async {
                match manual.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                res = self.sock.recv() => match res {
                    Ok(raw) => self.on_message(&raw).await,
                    Err(e) => {
                        self.event(&format!("recv 失敗: {:#}", e));
                        break;
                    }
                },
                _ = time::sleep_until(refresh_at) => self.refresh_registration().await,
                _ = time::sleep_until(retx_at) => self.retransmit_register().await,
                _ = time::sleep_until(call_timer) => self.on_call_timers().await,
                _ = time::sleep_until(flow_timer) => self.on_flow_timer().await,
                msg = manual_steps => match msg {
                    Ok(steps) => self.on_manual_steps(steps).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => manual = None,
                },
                _ = stop.changed(), if self.stopping_until.is_none() => {
                    if self.begin_stop().await {
                        break;
                    }
                }
            }
            // 結束流程跑完（或超過時限）→ 解除註冊並結束
            if let Some(until) = self.stopping_until {
                if self.flow.is_idle() || Instant::now() >= until {
                    self.deregister().await;
                    break;
                }
            }
        }
        Ok(())
    }

    // ─── 共用 ────────────────────────────────────────────────────

    fn event(&self, msg: &str) {
        self.sh.log.log_event(&self.account.extension, msg);
    }

    /// 記錄並送出一則 SIP 訊息（送出失敗只記錄，不中止 runner）
    async fn send(&self, msg: &str) -> Result<()> {
        self.sh.log.log_message(Direction::Send, msg, &self.peer);
        self.sock.send(msg).await
    }

    /// 送出 REGISTER：CSeq + 1，帶最近一次 challenge 的認證；UDP 時排定重傳
    async fn send_register(&mut self, expires: u32) -> Result<()> {
        self.reg.cseq = self.reg.cseq.wrapping_add(1);
        let server = &self.sh.cfg.server_addr;
        let auth = self.reg.challenge.as_ref().map(|c| {
            c.authorization_header(
                &self.account.username,
                &self.account.password,
                "REGISTER",
                &format!("sip:{}", server),
            )
        });
        let req = RegisterMessage::build(
            &self.account.username,
            server,
            &self.local_addr,
            self.reg.cseq,
            &SipMessage::new_branch(),
            &self.reg.from_tag,
            &self.reg.call_id,
            self.sh.cfg.transport_str(),
            expires,
            auth.as_deref(),
        );
        self.reg.last_register_at = Instant::now();
        self.send(&req).await?;
        self.reg.arm(&req, self.sock.is_reliable());
        Ok(())
    }

    /// 最近要處理的響鈴結束 / 通話逾時 / 主動掛斷時間
    fn next_call_deadline(&self) -> Option<Instant> {
        self.dialogs
            .values()
            .map(DialogCtx::deadline)
            .chain(self.pending.values().map(|p| p.due))
            .min()
    }

    // ─── 註冊 ────────────────────────────────────────────────────

    async fn refresh_registration(&mut self) {
        if self.reg.gauge.expired(self.reg.expires) {
            self.event("註冊已過期（刷新未獲 200 OK）");
            self.reg.gauge.set(false);
        }
        self.event("re-REGISTER（刷新）");
        let _ = self.send_register(self.reg.expires).await;
    }

    async fn retransmit_register(&mut self) {
        match self.reg.due(Instant::now()) {
            Ok(Some(req)) => {
                self.event("RETRANSMIT REGISTER");
                let _ = self.send(&req).await;
            }
            Ok(None) => {}
            Err(()) => self.event("REGISTER 無回應，停止重傳（等下次刷新再試）"),
        }
    }

    /// 收到停止訊號：響鈴中的來電回 480、停止 RTP；有結束流程時先執行它（回傳 false），
    /// 否則直接解除註冊（回傳 true，runner 結束）
    async fn begin_stop(&mut self) -> bool {
        for (_, p) in std::mem::take(&mut self.pending) {
            self.fail_invite(
                &p.invite_raw,
                &p.local_to_tag,
                "480 Temporarily Unavailable",
                480,
            )
            .await;
        }
        for (call_id, ctx) in self.dialogs.iter_mut() {
            self.sh
                .stop_call_rtp(call_id, ctx, &self.account.extension)
                .await;
        }
        let flow = &self.sh.cfg.agent_flow;
        if flow.on_stop.is_empty() || !self.reg.gauge.is_on() {
            self.event("收到停止訊號");
            self.deregister().await;
            return true;
        }
        // 還沒執行的流程（例如註冊後流程）不再執行；結束流程依分批註冊的順序錯開
        self.event("收到停止訊號，執行結束流程");
        let budget = flow.stop_budget() + Duration::from_secs(1);
        let steps = flow.on_stop.clone();
        self.stopping_until = Some(Instant::now() + self.start_delay + budget);
        self.flow.clear_pending();
        self.enqueue_staggered(steps).await;
        false
    }

    /// 送 REGISTER Expires=0 解除註冊
    async fn deregister(&mut self) {
        self.event("發送 REGISTER Expires=0");
        self.reg.gauge.set(false);
        let _ = self.send_register(0).await;
        let _ = time::timeout(Duration::from_millis(300), self.sock.recv()).await;
    }

    // ─── 收到 SIP 訊息 ───────────────────────────────────────────

    async fn on_message(&mut self, raw: &str) {
        self.sh.log.log_message(Direction::Recv, raw, &self.peer);
        if raw.starts_with("SIP/2.0") {
            self.on_response(raw).await;
        } else if self.on_request(raw).await {
            self.after_call_ended().await;
        }
    }

    /// REGISTER 的回應，以及座席流程撥碼的回應
    async fn on_response(&mut self, raw: &str) {
        let code = SipParser::status_code(raw).unwrap_or(0);
        let method = SipParser::cseq_method(raw);
        if SipParser::call_id(raw).is_some_and(|c| self.flow.is_dial(&c)) {
            self.on_dial_response(raw, code, method.as_deref()).await;
            return;
        }
        if method.as_deref() != Some("REGISTER") {
            // 已結束的撥碼收到重傳的錯誤回應（例如 CANCEL 後的 487）：照樣 ACK，對方才會停止重傳
            if method.as_deref() == Some("INVITE") && code >= 300 {
                let _ = self.send(&SipMessage::ack_for_non2xx(raw)).await;
            }
            return;
        }
        if code >= 200 {
            self.reg.on_final(SipParser::cseq_number(raw));
        }

        match code {
            200 => {
                self.reg.auth_retries = 0;
                self.reg.last_register_at = Instant::now();
                self.event("註冊成功");
                let expires = SipParser::expires(raw);
                if let Some(exp) = expires {
                    self.reg.expires = exp;
                }
                self.reg.gauge.on_ok(expires);
                // 第一次註冊成功 → 執行註冊後的流程（例如登入座席、就緒）
                if !self.flow_started && expires != Some(0) && self.stopping_until.is_none() {
                    self.flow_started = true;
                    let steps = self.sh.cfg.agent_flow.on_start.clone();
                    self.enqueue(steps).await;
                }
            }
            401 | 407 => {
                let Some(chal) = DigestChallenge::parse(raw) else {
                    self.event("無法解析 Digest challenge");
                    return;
                };
                self.reg.challenge = Some(chal);
                if self.reg.auth_retries >= MAX_AUTH_RETRIES {
                    // 已帶認證仍被拒 → 帳密錯誤，停止重送；等下次 refresh 計時再試
                    self.event(&format!(
                        "認證失敗（SIP {}，已重試 {} 次），停止重送 REGISTER",
                        code, self.reg.auth_retries,
                    ));
                    self.reg.gauge.set(false);
                    return;
                }
                self.reg.auth_retries += 1;
                if let Err(e) = self.send_register(self.reg.expires).await {
                    self.event(&format!("認證重送失敗: {}", e));
                }
            }
            _ => {
                self.event(&format!("REGISTER 回應 SIP {}", code));
                // 1xx 是暫時回應；其他最終回應（3xx–6xx）代表註冊被拒
                if code >= 300 {
                    self.reg.gauge.set(false);
                }
            }
        }
    }

    /// 處理 INVITE / ACK / BYE / CANCEL / OPTIONS；回傳是否有通話結束
    async fn on_request(&mut self, raw: &str) -> bool {
        let method = raw
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next())
            .map(|s| s.to_uppercase())
            .unwrap_or_default();
        let Some(call_id) = SipParser::call_id(raw) else {
            return false;
        };

        match method.as_str() {
            "INVITE" if self.dialogs.contains_key(&call_id) => {
                self.on_reinvite(raw, &call_id).await
            }
            "INVITE" if self.flow.is_dial(&call_id) => {
                // 撥碼中的 RE-INVITE（例如交換機切換提示音的媒體）：照樣接受，不當成新來電
                let ok = SipMessage::ok_for_server_reinvite(
                    raw,
                    &self.local_addr,
                    pick_dummy_rtp_port(),
                    &self.account.username,
                    self.sh.cfg.codec.payload_type(),
                );
                let _ = self.send(&ok).await;
            }
            "INVITE" => {
                if let Some(p) = self.pending.get(&call_id) {
                    // 響鈴中收到重傳的 INVITE → 重送 180
                    let ringing =
                        SipMessage::response_with_to_tag(raw, "180 Ringing", &p.local_to_tag);
                    let _ = self.send(&ringing).await;
                } else {
                    self.on_new_invite(raw, call_id).await;
                }
            }
            "ACK" => {
                if let Some(ctx) = self.dialogs.get_mut(&call_id) {
                    ctx.ack_received = true;
                }
            }
            "BYE" => {
                let _ = self.send(&SipMessage::ok_for_server_bye(raw)).await;
                if let Some(ctx) = self.dialogs.remove(&call_id) {
                    self.end_call(&call_id, ctx).await;
                    self.event(&format!("[{}] 通話結束", short(&call_id)));
                    return true;
                }
                if self.flow.is_dial(&call_id) {
                    // 交換機掛斷撥碼（例如播完提示音）
                    self.event("[撥碼] 交換機掛斷");
                    self.finish_dial().await;
                }
            }
            "CANCEL" => self.on_cancel(raw, &call_id).await,
            "OPTIONS" => {
                let _ = self
                    .send(&SipMessage::response_with_to_tag(raw, "200 OK", ""))
                    .await;
            }
            _ => self.event(&format!("忽略未知請求: {}", method)),
        }
        false
    }

    /// 新來電：100 Trying → 180 Ringing →（響鈴延遲）→ 200 OK + SDP / 486 / 不接
    async fn on_new_invite(&mut self, raw: &str, call_id: String) {
        // PDD / 建立時間只會量到本機處理時間（或刻意的響鈴延遲），故不記錄
        self.sh.live.on_invite();
        self.sh.traces.start(
            &trace_key(&self.account.extension, &call_id),
            CallTrace {
                sip_call_id: call_id.clone(),
                cin_call_id: SipParser::cin_call_id(raw),
                caller_number: SipParser::from_user(raw).unwrap_or_default(),
                agent: Some(self.account.extension.clone()),
                invite_at_ms: now_ms(),
                ..Default::default()
            },
        );

        let local_to_tag = SipMessage::new_tag();
        let behavior = &self.sh.cfg.agent_behavior;

        if self.stopping_until.is_some() {
            self.event(&format!("[{}] 結束流程中 → 480", short(&call_id)));
            self.fail_invite(raw, &local_to_tag, "480 Temporarily Unavailable", 480)
                .await;
            return;
        }

        // wrap-up 休息中，或通話中忙線（已有通話 / 響鈴中）→ 直接忙線
        let busy_reason = if self.wrap_until.is_some_and(|t| Instant::now() < t) {
            Some("休息中")
        } else if behavior.busy_in_call && (!self.dialogs.is_empty() || !self.pending.is_empty()) {
            Some("通話中")
        } else {
            None
        };
        if let Some(reason) = busy_reason {
            self.event(&format!("[{}] {} → 486", short(&call_id), reason));
            self.fail_invite(raw, &local_to_tag, "486 Busy Here", 486)
                .await;
            return;
        }

        // offer 沒有任何 G.711 → 488（真實話機也會在響鈴前拒絕）
        if sdp::negotiate(&sdp::audio_payloads(raw), self.sh.cfg.codec.payload_type()).is_none() {
            self.event(&format!(
                "[{}] offer 沒有 PCMA / PCMU → 488",
                short(&call_id)
            ));
            self.fail_invite(raw, &local_to_tag, "488 Not Acceptable Here", 488)
                .await;
            return;
        }

        let remote_from_tag = SipParser::from_tag(raw).unwrap_or_default();
        let _ = self
            .send(&SipMessage::response_with_to_tag(raw, "100 Trying", ""))
            .await;
        let _ = self
            .send(&SipMessage::response_with_to_tag(
                raw,
                "180 Ringing",
                &local_to_tag,
            ))
            .await;

        let behavior = &self.sh.cfg.agent_behavior;
        let outcome = RingOutcome::decide(behavior, rand::random::<f64>() * 100.0);
        let ring = match outcome {
            RingOutcome::NoAnswer => Some(Duration::from_secs(AgentBehavior::NO_ANSWER_CAP_SECS)),
            _ => random_secs(behavior.ring_min_secs, behavior.ring_max_secs),
        };
        match (outcome, ring) {
            (RingOutcome::Answer, None) => {
                let ctx = self
                    .answer_call(raw, &call_id, local_to_tag, remote_from_tag)
                    .await;
                self.dialogs.insert(call_id, ctx);
            }
            (outcome, ring) => {
                self.event(&format!(
                    "[{}] 響鈴中（{:?}，{:.1}s）",
                    short(&call_id),
                    outcome,
                    ring.unwrap_or_default().as_secs_f64()
                ));
                self.pending.insert(
                    call_id,
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

    /// RE-INVITE：回 200 OK + SDP，並更新 RTP 目標（對端可能換 port）
    async fn on_reinvite(&mut self, raw: &str, call_id: &str) {
        let Some(ctx) = self.dialogs.get(call_id) else {
            return;
        };
        let port = ctx
            .rtp_session
            .as_ref()
            .map(|r| r.local_port())
            .unwrap_or_else(pick_dummy_rtp_port);
        let ok = SipMessage::ok_for_server_reinvite(
            raw,
            &self.local_addr,
            port,
            &self.account.username,
            ctx.codec_pt,
        );
        let _ = self.send(&ok).await;

        let cfg = &self.sh.cfg;
        let new_rtp_addr = cfg
            .enable_rtp
            .then(|| SipResponse::sdp_rtp_addr(raw, &crate::net::host(&cfg.server_addr)))
            .flatten();
        if let (Some(addr), Some(rtp)) = (new_rtp_addr, &ctx.rtp_session) {
            match rtp.update_remote(&addr).await {
                Ok(()) => self.event(&format!("[{}] RE-INVITE RTP → {}", short(call_id), addr)),
                Err(e) => self.event(&format!(
                    "[{}] RE-INVITE RTP 更新失敗: {}",
                    short(call_id),
                    e
                )),
            }
        }
        self.event(&format!("[{}] 回應 RE-INVITE", short(call_id)));
    }

    async fn on_cancel(&mut self, raw: &str, call_id: &str) {
        if let Some(p) = self.pending.remove(call_id) {
            // 響鈴中被取消（主叫掛斷或 ACD 逾時改派）：CANCEL 回 200，INVITE 回 487
            let _ = self
                .send(&SipMessage::response_for_request(raw, "200 OK"))
                .await;
            self.event(&format!("[{}] 響鈴中被取消 → 487", short(call_id)));
            self.fail_invite(
                &p.invite_raw,
                &p.local_to_tag,
                "487 Request Terminated",
                487,
            )
            .await;
            return;
        }
        // 不在響鈴中 → INVITE transaction 已完成（已接聽或已回最終回應）：
        // 依 RFC 3261 §9.2，CANCEL 回 200 OK 但不影響已建立的通話（由後續 BYE 結束）；
        // 找不到對應通話則回 481
        let known = self.dialogs.contains_key(call_id);
        let status = if known {
            "200 OK"
        } else {
            "481 Call/Transaction Does Not Exist"
        };
        let _ = self
            .send(&SipMessage::response_for_request(raw, status))
            .await;
        if known {
            self.event(&format!(
                "[{}] 已接聽後收到 CANCEL，忽略（等待 BYE）",
                short(call_id)
            ));
        }
    }

    // ─── 計時器：響鈴結束、通話逾時 / 主動掛斷 ──────────────────

    async fn on_call_timers(&mut self) {
        let now = Instant::now();
        let rung: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.due)
            .map(|(k, _)| k.clone())
            .collect();
        for call_id in rung {
            let Some(p) = self.pending.remove(&call_id) else {
                continue;
            };
            match p.outcome {
                RingOutcome::Answer => {
                    let ctx = self
                        .answer_call(&p.invite_raw, &call_id, p.local_to_tag, p.remote_from_tag)
                        .await;
                    self.dialogs.insert(call_id, ctx);
                }
                RingOutcome::Reject => {
                    self.event(&format!("[{}] 拒接 → 486", short(&call_id)));
                    self.fail_invite(&p.invite_raw, &p.local_to_tag, "486 Busy Here", 486)
                        .await;
                }
                RingOutcome::NoAnswer => {
                    self.event(&format!(
                        "[{}] 響鈴 {}s 未被取消 → 480",
                        short(&call_id),
                        AgentBehavior::NO_ANSWER_CAP_SECS
                    ));
                    self.fail_invite(
                        &p.invite_raw,
                        &p.local_to_tag,
                        "480 Temporarily Unavailable",
                        480,
                    )
                    .await;
                }
            }
        }

        let due: Vec<String> = self
            .dialogs
            .iter()
            .filter(|(_, ctx)| now >= ctx.deadline())
            .map(|(k, _)| k.clone())
            .collect();
        for call_id in due {
            let Some(ctx) = self.dialogs.remove(&call_id) else {
                continue;
            };
            self.event(&format!(
                "[{}] {} → 主動 BYE",
                short(&call_id),
                ctx.hangup_reason(now)
            ));
            let bye = build_bye_from_dialog(
                &ctx.invite_raw,
                &ctx.local_to_tag,
                &ctx.remote_from_tag,
                &self.local_addr,
                &self.account,
            );
            let _ = self.send(&bye).await;
            self.end_call(&call_id, ctx).await;
            self.after_call_ended().await;
        }
    }

    // ─── 通話 ────────────────────────────────────────────────────

    /// 接聽來電：分配 RTP port、送 200 OK + SDP、啟動 RTP，回傳 dialog
    async fn answer_call(
        &self,
        raw: &str,
        call_id: &str,
        local_to_tag: String,
        remote_from_tag: String,
    ) -> DialogCtx {
        let cfg = &self.sh.cfg;
        // 分配真實 RTP port（若啟用）
        let (rtp_port, pre_bound) = if cfg.enable_rtp {
            match RtpSession::allocate_port(&self.sh.port_counter, &self.local_ip).await {
                Ok((p, s)) => (p, Some(s)),
                Err(e) => {
                    self.event(&format!("RTP port 分配失敗: {}", e));
                    (pick_dummy_rtp_port(), None)
                }
            }
        } else {
            (pick_dummy_rtp_port(), None)
        };

        // 解析 INVITE SDP 中的遠端 RTP 地址
        let remote_rtp_addr = SipResponse::sdp_rtp_addr(raw, &crate::net::host(&cfg.server_addr));

        // 依 offer 選 codec（響鈴前已排除沒有 G.711 的 offer），answer 只宣告選中的那一個
        let offer = sdp::audio_payloads(raw);
        let preferred = cfg.codec.payload_type();
        let codec_pt = sdp::negotiate(&offer, preferred).unwrap_or(preferred);

        let ok = build_response_with_sdp(
            raw,
            "200 OK",
            &local_to_tag,
            &self.account.username,
            &self.local_addr,
            rtp_port,
            &sdp::answer_payloads(codec_pt, &offer),
        );
        let _ = self.send(&ok).await;

        let answered_at = Instant::now();
        self.sh.live.on_answered();
        self.sh
            .traces
            .update(&trace_key(&self.account.extension, call_id), |t| {
                t.answered_at_ms = Some(now_ms());
                t.outcome = CallOutcome::Answered;
                t.response_code = Some(200);
            });

        let rtp_session = match (cfg.enable_rtp, remote_rtp_addr) {
            (false, _) => None,
            (true, None) => {
                self.event(&format!(
                    "[{}] INVITE SDP 無 RTP 地址，跳過 RTP",
                    short(call_id)
                ));
                None
            }
            (true, Some(remote_addr)) => {
                let rtp_cfg = RtpSessionConfig {
                    base_port: cfg.rtp_base_port,
                    local_ip: self.local_ip.clone(),
                    remote_addr: remote_addr.clone(),
                    audio_file: cfg.audio_file.clone(),
                    ssrc: None,
                    local_port: Some(rtp_port),
                    codec_pt,
                };
                match RtpSession::start(rtp_cfg, Arc::clone(&self.sh.port_counter), pre_bound).await
                {
                    Ok(session) => {
                        // 登記為進行中（用於即時品質聚合）
                        self.sh
                            .rtp_registry
                            .lock()
                            .await
                            .active
                            .insert(call_id.to_string(), Arc::clone(&session.stats));
                        self.sh.live.on_rtp_start();
                        self.event(&format!(
                            "[{}] RTP 啟動 port={} → {}",
                            short(call_id),
                            rtp_port,
                            remote_addr
                        ));
                        Some(session)
                    }
                    Err(e) => {
                        self.event(&format!("[{}] RTP 啟動失敗: {}", short(call_id), e));
                        None
                    }
                }
            }
        };

        let behavior = &cfg.agent_behavior;
        let hangup_at =
            random_secs(behavior.talk_min_secs, behavior.talk_max_secs).map(|d| answered_at + d);
        self.event(&format!("[{}] 接聽來電", short(call_id)));
        DialogCtx {
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
        }
    }

    /// 對 INVITE 送最終失敗回應（486 / 487 / 488 / 480），計為失敗通話
    async fn fail_invite(
        &self,
        invite_raw: &str,
        local_to_tag: &str,
        status_line: &str,
        code: u16,
    ) {
        if let Some(call_id) = SipParser::call_id(invite_raw) {
            self.sh
                .traces
                .update(&trace_key(&self.account.extension, &call_id), |t| {
                    t.outcome = CallOutcome::Failed;
                    t.response_code = Some(code);
                    t.ended_at_ms = Some(now_ms());
                });
        }
        let resp = SipMessage::response_with_to_tag(invite_raw, status_line, local_to_tag);
        let _ = self.send(&resp).await;
        self.sh.live.on_failed();
        self.sh.detail.record_fail_code(code);
    }

    /// 通話結束（對方或座席 BYE）：停止 RTP、記錄通話時長
    async fn end_call(&self, call_id: &str, mut ctx: DialogCtx) {
        self.sh
            .stop_call_rtp(call_id, &mut ctx, &self.account.extension)
            .await;
        self.sh
            .detail
            .record_duration(ctx.answered_at.elapsed().as_secs_f64());
        self.sh.live.on_completed();
        self.sh
            .traces
            .update(&trace_key(&self.account.extension, call_id), |t| {
                t.ended_at_ms = Some(now_ms())
            });
    }

    /// 通話結束後：進入 wrap-up；需要時立即重新註冊
    async fn after_call_ended(&mut self) {
        let wrap_up = Duration::from_secs_f64(self.sh.cfg.agent_behavior.wrap_up_secs);
        self.wrap_until = Some(Instant::now() + wrap_up);
        if self.sh.cfg.agent_reregister_after_call {
            let _ = self.send_register(self.reg.expires).await;
            self.event("通話結束 → re-REGISTER");
        }
        if self.stopping_until.is_none() {
            let steps = self.sh.cfg.agent_flow.after_call.clone();
            self.enqueue(steps).await;
        }
    }

    // ─── 座席流程：撥特殊碼（登入 / 就緒 / 未就緒 / 登出）────────

    fn endpoint(&self) -> Endpoint<'_> {
        Endpoint {
            username: &self.account.username,
            server: &self.sh.cfg.server_addr,
            local_addr: &self.local_addr,
            transport: self.sh.cfg.transport_str(),
            codec_pt: self.sh.cfg.codec.payload_type(),
        }
    }

    async fn enqueue(&mut self, steps: Vec<FlowStep>) {
        if steps.is_empty() {
            return;
        }
        self.flow.push(steps);
        self.advance_flow().await;
    }

    /// 依這個帳號在分批註冊中的順序延後執行（大量座席時避免同時撥進交換機）
    async fn enqueue_staggered(&mut self, steps: Vec<FlowStep>) {
        if steps.is_empty() {
            return;
        }
        if !self.start_delay.is_zero() {
            self.flow.push([FlowStep::Wait {
                secs: self.start_delay.as_secs_f64(),
            }]);
        }
        self.enqueue(steps).await;
    }

    /// 手動撥碼：只對已註冊、不在結束流程中的帳號執行
    async fn on_manual_steps(&mut self, steps: Vec<FlowStep>) {
        if self.reg.gauge.is_on() && self.stopping_until.is_none() {
            self.enqueue_staggered(steps).await;
        }
    }

    /// 沒有撥碼進行中時，執行下一個步驟
    async fn advance_flow(&mut self) {
        let Next::Dial {
            code: template,
            label,
        } = self.flow.next(Instant::now())
        else {
            return;
        };
        let now = Instant::now();
        let code = self
            .sh
            .cfg
            .agent_flow
            .expand(&template, &self.account.extension);
        let mut dial = OutDial::new(template, label, code, pick_dummy_rtp_port(), now);
        let invite = dial.invite(&self.endpoint(), None);
        dial.arm_retx(invite.clone(), self.sock.is_reliable(), now);
        self.event(&format!("[撥碼] {}：撥出", dial.name()));
        self.flow.dial = Some(dial);
        let _ = self.send(&invite).await;
    }

    async fn on_dial_response(&mut self, raw: &str, code: u16, method: Option<&str>) {
        let now = Instant::now();
        let Some(dial) = self.flow.dial.as_ref() else {
            return;
        };
        let state = dial.state;
        match method {
            Some("INVITE") => {
                // 上一個 transaction（帶認證重送前）重傳的回應
                if SipParser::cseq_number(raw) != Some(dial.cseq) {
                    if code >= 300 {
                        let _ = self.send(&SipMessage::ack_for_non2xx(raw)).await;
                    }
                    return;
                }
                match code {
                    100..=199 => {
                        if let Some(d) = self.flow.dial.as_mut() {
                            d.provisional = true;
                            d.stop_retx();
                        }
                    }
                    200..=299 => self.on_dial_answered(raw, code, now).await,
                    401 | 407 if !dial.auth_tried && state == DialState::Calling => {
                        let _ = self.send(&SipMessage::ack_for_non2xx(raw)).await;
                        match DigestChallenge::parse(raw) {
                            Some(chal) => self.resend_dial_with_auth(&chal, now).await,
                            None => self.fail_dial(code, "無法解析認證要求").await,
                        }
                    }
                    300.. => {
                        let _ = self.send(&SipMessage::ack_for_non2xx(raw)).await;
                        if state == DialState::Calling {
                            self.fail_dial(code, "").await;
                        }
                    }
                    _ => {}
                }
            }
            Some("BYE") if state == DialState::Terminating && code >= 200 => {
                self.finish_dial().await;
            }
            _ => {}
        }
    }

    /// 撥碼接通：送 ACK，保持 `hold_secs` 後掛斷（200 OK 重傳時只重送 ACK）
    async fn on_dial_answered(&mut self, raw: &str, code: u16, now: Instant) {
        let hold = Duration::from_secs_f64(self.sh.cfg.agent_flow.hold_secs);
        let Some(dial) = self.flow.dial.as_mut() else {
            return;
        };
        if dial.state == DialState::Calling {
            dial.on_answered(raw, hold, now);
            let (name, template, label) = (
                dial.name().to_string(),
                dial.template.clone(),
                dial.label.clone(),
            );
            self.event(&format!("[撥碼] {}：成功（SIP {}）", name, code));
            self.sh.record_dial(&template, &label, None);
        }
        if let Some(ack) = self.flow.dial.as_ref().map(|d| d.ack(&self.endpoint())) {
            let _ = self.send(&ack).await;
        }
    }

    /// 401 / 407：帶座席帳密的 Digest 認證重送一次
    async fn resend_dial_with_auth(&mut self, chal: &DigestChallenge, now: Instant) {
        let reliable = self.sock.is_reliable();
        let Some(dial) = self.flow.dial.as_mut() else {
            return;
        };
        dial.restart(now);
        let uri = format!("sip:{}@{}", dial.code, self.sh.cfg.server_addr);
        let auth = chal.authorization_header(
            &self.account.username,
            &self.account.password,
            "INVITE",
            &uri,
        );
        let Some(invite) = self
            .flow
            .dial
            .as_ref()
            .map(|d| d.invite(&self.endpoint(), Some(&auth)))
        else {
            return;
        };
        if let Some(d) = self.flow.dial.as_mut() {
            d.arm_retx(invite.clone(), reliable, now);
        }
        let _ = self.send(&invite).await;
    }

    /// 撥碼失敗（`code` = 0 表示沒有回應）
    async fn fail_dial(&mut self, code: u16, why: &str) {
        if let Some(dial) = self.flow.dial.as_ref() {
            let detail = match (code, why.is_empty()) {
                (0, _) => "沒有回應".to_string(),
                (c, true) => format!("SIP {}", c),
                (c, false) => format!("SIP {}，{}", c, why),
            };
            self.event(&format!("[撥碼] {}：失敗（{}）", dial.name(), detail));
            self.sh.record_dial(&dial.template, &dial.label, Some(code));
        }
        self.finish_dial().await;
    }

    /// 撥碼結束（成功或失敗），繼續下一個步驟
    async fn finish_dial(&mut self) {
        self.flow.dial = None;
        self.advance_flow().await;
    }

    /// 等待結束、撥碼重傳 / 逾時、保持時間到要掛斷
    async fn on_flow_timer(&mut self) {
        let now = Instant::now();
        let Some(dial) = self.flow.dial.as_mut() else {
            self.advance_flow().await;
            return;
        };
        if let Some(invite) = dial.due_retx(now) {
            let _ = self.send(&invite).await;
            return;
        }
        match dial.state {
            DialState::Calling if now >= dial.give_up_at => {
                let provisional = dial.provisional;
                if provisional {
                    let cancel = self.flow.dial.as_ref().map(|d| d.cancel(&self.endpoint()));
                    if let Some(cancel) = cancel {
                        let _ = self.send(&cancel).await;
                    }
                }
                self.fail_dial(0, "").await;
            }
            DialState::Answered { hangup_at } if now >= hangup_at => {
                // Endpoint 只借用帳號 / 設定欄位，與 self.flow 不重疊
                let ep = Endpoint {
                    username: &self.account.username,
                    server: &self.sh.cfg.server_addr,
                    local_addr: &self.local_addr,
                    transport: self.sh.cfg.transport_str(),
                    codec_pt: self.sh.cfg.codec.payload_type(),
                };
                let bye = dial.bye(&ep, now);
                let _ = self.send(&bye).await;
            }
            DialState::Terminating if now >= dial.give_up_at => {
                self.finish_dial().await;
            }
            _ => {}
        }
    }
}
