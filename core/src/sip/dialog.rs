/// SIP 對話狀態機
use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

/// RFC 3261 §17.1.1.1：RTT 估計值，UDP 重傳的起始間隔
pub const T1: Duration = Duration::from_millis(500);
/// RFC 3261 §17.1.2.2：非 INVITE 請求的最大重傳間隔
pub const T2: Duration = Duration::from_secs(4);

/// 單一通話的完整狀態
#[derive(Debug, Clone)]
pub struct Dialog {
    pub call_id:    String,
    pub from_tag:   String,
    pub to_tag:     Option<String>,   // 收到 200 OK 後才有
    pub branch:     String,
    pub cseq:       u32,
    pub state:      DialogState,

    // 計時
    pub invite_sent_at:  Instant,
    pub ringing_at:      Option<Instant>,  // 收到 180
    pub answered_at:     Option<Instant>,  // 收到 200 OK
    pub bye_sent_at:     Option<Instant>,
    pub ended_at:        Option<Instant>,

    // 目標號碼（記錄用）
    pub callee: String,

    // 本機 RTP port（INVITE 前預分配，用於 SDP 與 RTP session）
    pub local_rtp_port: u16,

    // 對端 Contact URI（從 200 OK 解析，用於 BYE/ACK Request-URI）
    pub remote_contact: Option<String>,

    // UDP 重傳（INVITE：Timer A；BYE：Timer E）
    /// 等待回應中的請求原文（重傳時原樣送出）
    retx_msg:      Option<String>,
    retx_interval: Duration,
    next_retx_at:  Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DialogState {
    /// INVITE 已送出，等待回應
    Calling,
    /// 收到 100 Trying
    Trying,
    /// 收到 180 Ringing，PDD 計時結束
    Ringing,
    /// 收到 200 OK，ACK 已送出
    Connected,
    /// BYE 已送出，等待 200 OK
    Terminating,
    /// 通話正常結束
    Completed,
    /// 被拒絕（4xx/5xx/6xx）
    Failed(u16),
    /// 逾時（未收到回應）
    TimedOut,
    /// 主動 CANCEL
    Cancelled,
}

impl Dialog {
    pub fn new(
        call_id: String,
        from_tag: String,
        branch: String,
        callee: String,
        local_rtp_port: u16,
    ) -> Self {
        Self {
            call_id,
            from_tag,
            to_tag:         None,
            branch,
            cseq:           1,
            state:          DialogState::Calling,
            invite_sent_at: Instant::now(),
            ringing_at:     None,
            answered_at:    None,
            bye_sent_at:    None,
            ended_at:       None,
            callee,
            local_rtp_port,
            remote_contact: None,
            retx_msg:       None,
            retx_interval:  T1,
            next_retx_at:   None,
        }
    }

    /// 送出 INVITE / BYE 後呼叫：記住請求原文並在 T1 後開始重傳
    pub fn arm_retransmit(&mut self, msg: String, now: Instant) {
        self.retx_msg      = Some(msg);
        self.retx_interval = T1;
        self.next_retx_at  = Some(now + T1);
    }

    /// 若重傳時間已到，回傳要重送的請求並排定下一次重傳
    ///
    /// - Calling（尚未收到任何 1xx）：重傳 INVITE，間隔每次加倍（Timer A）
    /// - Terminating：重傳 BYE，間隔加倍但不超過 T2（Timer E）
    /// - 其他狀態（已收到 1xx / 最終回應）：停止重傳
    pub fn due_retransmit(&mut self, now: Instant) -> Option<String> {
        let max_interval = match self.state {
            DialogState::Calling     => None,
            DialogState::Terminating => Some(T2),
            _ => {
                self.retx_msg     = None;
                self.next_retx_at = None;
                return None;
            }
        };
        if now < self.next_retx_at? {
            return None;
        }
        let doubled = self.retx_interval * 2;
        self.retx_interval = max_interval.map_or(doubled, |m| doubled.min(m));
        self.next_retx_at  = Some(now + self.retx_interval);
        self.retx_msg.clone()
    }

    /// 收到 100 Trying
    pub fn on_trying(&mut self) {
        if self.state == DialogState::Calling {
            self.state = DialogState::Trying;
        }
    }

    /// 收到 180 Ringing → 記錄 PDD
    pub fn on_ringing(&mut self) {
        if matches!(self.state, DialogState::Calling | DialogState::Trying) {
            self.state     = DialogState::Ringing;
            self.ringing_at = Some(Instant::now());
        }
    }

    /// 收到 200 OK → 記錄通話建立時間，準備送 ACK
    pub fn on_ok(&mut self, to_tag: String) {
        if matches!(self.state, DialogState::Calling | DialogState::Trying | DialogState::Ringing) {
            self.state       = DialogState::Connected;
            self.to_tag      = Some(to_tag);
            self.answered_at = Some(Instant::now());
        }
    }

    /// 準備送 BYE
    pub fn on_bye_sent(&mut self) {
        self.state       = DialogState::Terminating;
        self.bye_sent_at = Some(Instant::now());
        self.cseq       += 1;
    }

    /// 收到 BYE 的 200 OK
    pub fn on_bye_ok(&mut self) {
        self.state    = DialogState::Completed;
        self.ended_at = Some(Instant::now());
    }

    /// 收到錯誤回應
    pub fn on_error(&mut self, code: u16) {
        self.state    = DialogState::Failed(code);
        self.ended_at = Some(Instant::now());
    }

    /// 逾時
    pub fn on_timeout(&mut self) {
        self.state    = DialogState::TimedOut;
        self.ended_at = Some(Instant::now());
    }

    // ── 指標計算 ──

    /// PDD（Post Dial Delay）：INVITE → 180 Ringing（毫秒）
    pub fn pdd_ms(&self) -> Option<f64> {
        self.ringing_at.map(|r| {
            r.duration_since(self.invite_sent_at).as_secs_f64() * 1000.0
        })
    }

    /// 通話建立時間：INVITE → 200 OK（毫秒）
    pub fn setup_time_ms(&self) -> Option<f64> {
        self.answered_at.map(|a| {
            a.duration_since(self.invite_sent_at).as_secs_f64() * 1000.0
        })
    }

    /// 通話持續時間：200 OK → BYE 200 OK（秒）
    pub fn call_duration_secs(&self) -> Option<f64> {
        match (self.answered_at, self.ended_at) {
            (Some(a), Some(e)) if self.state == DialogState::Completed => {
                Some(e.duration_since(a).as_secs_f64())
            }
            _ => None,
        }
    }

    /// 是否成功接通
    pub fn is_answered(&self) -> bool {
        self.answered_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_dialog() -> Dialog {
        Dialog::new("call-1".into(), "tag-1".into(), "branch-1".into(), "2001".into(), 16000)
    }

    #[test]
    fn initial_state_is_calling() {
        let d = new_dialog();
        assert_eq!(d.state, DialogState::Calling);
        assert!(!d.is_answered());
    }

    #[test]
    fn normal_call_flow() {
        let mut d = new_dialog();

        d.on_trying();
        assert_eq!(d.state, DialogState::Trying);

        d.on_ringing();
        assert_eq!(d.state, DialogState::Ringing);
        assert!(d.pdd_ms().is_some());

        d.on_ok("to-tag".into());
        assert_eq!(d.state, DialogState::Connected);
        assert!(d.is_answered());
        assert!(d.setup_time_ms().is_some());
        assert_eq!(d.to_tag, Some("to-tag".to_string()));

        d.on_bye_sent();
        assert_eq!(d.state, DialogState::Terminating);
        assert!(d.bye_sent_at.is_some());

        d.on_bye_ok();
        assert_eq!(d.state, DialogState::Completed);
        assert!(d.ended_at.is_some());
        assert!(d.call_duration_secs().is_some());
    }

    #[test]
    fn skip_trying_directly_to_ringing() {
        let mut d = new_dialog();
        d.on_ringing();
        assert_eq!(d.state, DialogState::Ringing);
    }

    #[test]
    fn skip_to_200ok_from_calling() {
        let mut d = new_dialog();
        d.on_ok("tag".into());
        assert_eq!(d.state, DialogState::Connected);
    }

    #[test]
    fn error_response_records_code() {
        let mut d = new_dialog();
        d.on_trying();
        d.on_error(486);
        assert_eq!(d.state, DialogState::Failed(486));
        assert!(d.ended_at.is_some());
    }

    #[test]
    fn timeout_sets_state() {
        let mut d = new_dialog();
        d.on_timeout();
        assert_eq!(d.state, DialogState::TimedOut);
    }

    #[test]
    fn ringing_only_from_early_states() {
        let mut d = new_dialog();
        d.on_ok("tag".into());
        let prev = d.state.clone();
        d.on_ringing();
        assert_eq!(d.state, prev, "ringing should not change Connected state");
    }

    #[test]
    fn ok_only_from_early_states() {
        let mut d = new_dialog();
        d.on_ok("tag1".into());
        d.on_bye_sent();
        let prev = d.state.clone();
        d.on_ok("tag2".into());
        assert_eq!(d.state, prev, "OK should not change Terminating state");
    }

    #[test]
    fn invite_retransmits_with_doubling_interval_until_provisional() {
        let mut d = new_dialog();
        let t0 = d.invite_sent_at;
        d.arm_retransmit("INVITE".into(), t0);

        assert_eq!(d.due_retransmit(t0 + Duration::from_millis(499)), None);
        // Timer A：0.5s → 1s → 2s → 4s（不設上限）
        let mut t = t0;
        for gap_ms in [500, 1000, 2000, 4000, 8000] {
            t += Duration::from_millis(gap_ms);
            assert_eq!(d.due_retransmit(t).as_deref(), Some("INVITE"), "t+{}ms", gap_ms);
            assert_eq!(d.due_retransmit(t), None, "同一時間點不可重複重傳");
        }

        d.on_trying();
        assert_eq!(d.due_retransmit(t + Duration::from_secs(60)), None, "收到 1xx 後停止重傳");
    }

    #[test]
    fn bye_retransmit_interval_is_capped_at_t2() {
        let mut d = new_dialog();
        d.on_ok("tag".into());
        d.on_bye_sent();
        let t0 = Instant::now();
        d.arm_retransmit("BYE".into(), t0);

        let mut t = t0;
        for gap_ms in [500, 1000, 2000, 4000, 4000, 4000] {
            t += Duration::from_millis(gap_ms);
            assert_eq!(d.due_retransmit(t).as_deref(), Some("BYE"), "t+{}ms", gap_ms);
        }

        d.on_bye_ok();
        assert_eq!(d.due_retransmit(t + Duration::from_secs(60)), None, "收到最終回應後停止重傳");
    }

    #[test]
    fn no_retransmit_without_arming() {
        let mut d = new_dialog();
        assert_eq!(d.due_retransmit(Instant::now() + Duration::from_secs(10)), None);
    }

    #[test]
    fn duration_requires_completed_state() {
        let mut d = new_dialog();
        d.on_ok("tag".into());
        assert!(d.call_duration_secs().is_none(), "duration requires Completed state");
    }
}
