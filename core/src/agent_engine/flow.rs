//! 座席流程：依序執行的撥碼 / 等待步驟（座席登入、就緒、登出等特殊碼），以及撥碼的 SIP 訊息組裝
//!
//! 狀態由帳號的 runner 持有；收到回應、計時器到期時由 runner 推進（見 `runner.rs` 的「座席流程」段落）。
use crate::config::FlowStep;
use crate::sip::{SipMessage, SipParser};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// UDP 時 INVITE 重傳（RFC 3261 Timer A：T1 起每次加倍，上限 T2）
const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);

/// 一個帳號待執行的流程步驟
#[derive(Default)]
pub(super) struct FlowQueue {
    steps: VecDeque<FlowStep>,
    /// 目前的等待步驟結束時間
    wait_until: Option<Instant>,
    /// 進行中的撥碼
    pub dial: Option<OutDial>,
}

/// 下一個要執行的動作
pub(super) enum Next {
    /// 還在等待或撥碼中
    Busy,
    Dial {
        code: String,
        label: String,
    },
    Idle,
}

impl FlowQueue {
    pub fn push(&mut self, steps: impl IntoIterator<Item = FlowStep>) {
        self.steps.extend(steps);
    }

    /// 丟掉還沒執行的步驟（進行中的撥碼照常結束）
    pub fn clear_pending(&mut self) {
        self.steps.clear();
        self.wait_until = None;
    }

    /// 沒有待執行的步驟、也沒有進行中的撥碼或等待
    pub fn is_idle(&self) -> bool {
        self.steps.is_empty() && self.dial.is_none() && self.wait_until.is_none()
    }

    pub fn is_dial(&self, call_id: &str) -> bool {
        self.dial.as_ref().is_some_and(|d| d.call_id == call_id)
    }

    /// 取出下一個撥碼；等待步驟在這裡展開成計時
    pub fn next(&mut self, now: Instant) -> Next {
        if self.dial.is_some() {
            return Next::Busy;
        }
        loop {
            if let Some(t) = self.wait_until {
                if now < t {
                    return Next::Busy;
                }
                self.wait_until = None;
            }
            match self.steps.pop_front() {
                None => return Next::Idle,
                Some(FlowStep::Wait { secs }) => {
                    self.wait_until = Some(now + Duration::from_secs_f64(secs.max(0.0)));
                }
                Some(FlowStep::Dial { code, label }) => return Next::Dial { code, label },
            }
        }
    }

    /// 最近的計時：等待結束、撥碼重傳 / 逾時 / 掛斷
    pub fn deadline(&self) -> Option<Instant> {
        let dial = self.dial.as_ref().map(OutDial::deadline);
        match (self.wait_until, dial) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum DialState {
    /// 已送 INVITE，等最終回應
    Calling,
    /// 已接通，`hangup_at` 時由座席掛斷
    Answered { hangup_at: Instant },
    /// 已送 BYE，等回應
    Terminating,
}

/// 撥碼用到的本端資訊
pub(super) struct Endpoint<'a> {
    pub username: &'a str,
    /// SIP server（ip:port）；From / To / Request-URI 都用它，與 REGISTER 相同
    pub server: &'a str,
    pub local_addr: &'a str,
    /// "UDP" / "TCP"
    pub transport: &'a str,
    /// SDP offer 的偏好 codec
    pub codec_pt: u8,
}

/// 進行中的一個撥碼（UAC 的 INVITE dialog）
pub(super) struct OutDial {
    /// 設定中的撥碼範本（統計依此彙總，log 也只印範本，避免印出密碼）
    pub template: String,
    pub label: String,
    /// 代入變數後實際撥出的號碼
    pub code: String,
    pub call_id: String,
    from_tag: String,
    branch: String,
    pub cseq: u32,
    to_tag: String,
    /// 對方 200 OK 的 Contact（ACK / BYE 的 Request-URI）
    remote_uri: Option<String>,
    rtp_port: u16,
    pub state: DialState,
    /// 收到過 1xx（逾時要送 CANCEL）
    pub provisional: bool,
    /// 已帶認證重送過（再收到 401 / 407 就算失敗）
    pub auth_tried: bool,
    /// Calling：放棄的時間；Terminating：不再等 BYE 回應的時間
    pub give_up_at: Instant,
    /// UDP INVITE 重傳（收到任何回應後停止）
    retx: Option<(String, Duration, Instant)>,
}

impl OutDial {
    pub fn new(template: String, label: String, code: String, rtp_port: u16, now: Instant) -> Self {
        Self {
            template,
            label,
            code,
            call_id: SipMessage::new_call_id("sipress"),
            from_tag: SipMessage::new_tag(),
            branch: SipMessage::new_branch(),
            cseq: 1,
            to_tag: String::new(),
            remote_uri: None,
            rtp_port,
            state: DialState::Calling,
            provisional: false,
            auth_tried: false,
            give_up_at: now + Duration::from_secs(crate::config::AgentFlow::DIAL_TIMEOUT_SECS),
            retx: None,
        }
    }

    /// 顯示用名稱：有 label 用 label，否則用範本
    pub fn name(&self) -> &str {
        if self.label.is_empty() {
            &self.template
        } else {
            &self.label
        }
    }

    pub fn deadline(&self) -> Instant {
        let state = match self.state {
            DialState::Answered { hangup_at } => hangup_at,
            _ => self.give_up_at,
        };
        match &self.retx {
            Some((_, _, at)) => state.min(*at),
            None => state,
        }
    }

    /// INVITE（`auth` 為完整的 Authorization / Proxy-Authorization 行）
    pub fn invite(&self, ep: &Endpoint, auth: Option<&str>) -> String {
        let invite = SipMessage::invite(
            &self.call_id,
            ep.username,
            ep.server,
            &self.code,
            ep.server,
            ep.local_addr,
            self.cseq,
            &self.branch,
            &self.from_tag,
            ep.transport,
            self.rtp_port,
            ep.codec_pt,
        );
        match auth {
            Some(line) => invite.replacen(
                "Content-Type: application/sdp\r\n",
                &format!("{}\r\nContent-Type: application/sdp\r\n", line),
                1,
            ),
            None => invite,
        }
    }

    /// 帶認證重送：新的 transaction（CSeq + 1、新 branch）
    pub fn restart(&mut self, now: Instant) {
        self.cseq += 1;
        self.branch = SipMessage::new_branch();
        self.provisional = false;
        self.auth_tried = true;
        self.retx = None;
        self.give_up_at = now + Duration::from_secs(crate::config::AgentFlow::DIAL_TIMEOUT_SECS);
    }

    /// 排定 UDP 重傳；TCP 不重傳
    pub fn arm_retx(&mut self, msg: String, reliable: bool, now: Instant) {
        self.retx = (!reliable).then(|| (msg, T1, now + T1));
    }

    pub fn stop_retx(&mut self) {
        self.retx = None;
    }

    /// 重傳時間到：回傳要重送的 INVITE
    pub fn due_retx(&mut self, now: Instant) -> Option<String> {
        let (msg, interval, at) = self.retx.as_mut()?;
        if now < *at {
            return None;
        }
        *interval = (*interval * 2).min(T2);
        *at = now + *interval;
        Some(msg.clone())
    }

    /// 收到 2xx：記下 dialog 資訊
    pub fn on_answered(&mut self, raw: &str, hold: Duration, now: Instant) {
        self.to_tag = SipParser::to_tag(raw).unwrap_or_default();
        self.remote_uri = SipParser::contact_uri(raw);
        self.retx = None;
        self.state = DialState::Answered {
            hangup_at: now + hold,
        };
    }

    pub fn ack(&self, ep: &Endpoint) -> String {
        SipMessage::ack(
            &self.call_id,
            ep.username,
            ep.server,
            &self.code,
            &self.to_tag,
            ep.server,
            ep.local_addr,
            self.cseq,
            &SipMessage::new_branch(),
            &self.from_tag,
            ep.transport,
            self.remote_uri.as_deref(),
        )
    }

    /// 座席掛斷：送 BYE 並等回應
    pub fn bye(&mut self, ep: &Endpoint, now: Instant) -> String {
        self.state = DialState::Terminating;
        self.give_up_at = now + Duration::from_secs(crate::config::AgentFlow::BYE_TIMEOUT_SECS);
        SipMessage::bye(
            &self.call_id,
            ep.username,
            ep.server,
            &self.code,
            &self.to_tag,
            ep.server,
            ep.local_addr,
            self.cseq + 1,
            &SipMessage::new_branch(),
            &self.from_tag,
            ep.transport,
            self.remote_uri.as_deref(),
        )
    }

    /// 逾時放棄：CANCEL 與 INVITE 同 branch / CSeq
    pub fn cancel(&self, ep: &Endpoint) -> String {
        SipMessage::cancel(
            &self.call_id,
            ep.username,
            ep.server,
            &self.code,
            ep.server,
            ep.local_addr,
            self.cseq,
            &self.branch,
            &self.from_tag,
            ep.transport,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_runs_waits_then_dials_in_order() {
        let mut q = FlowQueue::default();
        q.push([
            FlowStep::Dial {
                code: "*94".into(),
                label: "登入".into(),
            },
            FlowStep::Wait { secs: 2.0 },
            FlowStep::Dial {
                code: "*96".into(),
                label: String::new(),
            },
        ]);
        let t0 = Instant::now();
        assert!(matches!(q.next(t0), Next::Dial { ref code, .. } if code == "*94"));
        q.dial = Some(OutDial::new(
            "*94".into(),
            String::new(),
            "*94".into(),
            9,
            t0,
        ));
        assert!(matches!(q.next(t0), Next::Busy), "撥碼中");
        q.dial = None;
        assert!(matches!(q.next(t0), Next::Busy), "開始等待");
        assert_eq!(q.deadline(), Some(t0 + Duration::from_secs(2)));
        assert!(
            matches!(q.next(t0 + Duration::from_secs(2)), Next::Dial { ref code, .. } if code == "*96")
        );
        assert!(matches!(q.next(t0 + Duration::from_secs(2)), Next::Idle));
        assert!(q.is_idle());
    }

    #[test]
    fn invite_retransmission_doubles_up_to_t2() {
        let t0 = Instant::now();
        let mut d = OutDial::new("*96".into(), String::new(), "*96".into(), 9, t0);
        d.arm_retx("INVITE".into(), true, t0);
        assert!(d.due_retx(t0 + T2).is_none(), "TCP 不重傳");
        d.arm_retx("INVITE".into(), false, t0);
        d.give_up_at = t0 + Duration::from_secs(100); // 只看重傳間隔
        let mut t = t0 + T1;
        let mut gaps = Vec::new();
        for _ in 0..5 {
            assert!(d.due_retx(t).is_some());
            let next = d.deadline().min(t + T2);
            gaps.push(next.duration_since(t).as_millis());
            t = next;
        }
        assert_eq!(gaps, vec![1000, 2000, 4000, 4000, 4000]);
    }
}
