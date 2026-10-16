//! 通話與響鈴中來電的狀態、座席行為的隨機決定，以及座席端送出的 SIP 訊息組裝
use crate::config::{AgentAccount, AgentBehavior};
use crate::rtp::session::RtpSession;
use crate::sip::{sdp, SipMessage, SipParser};
use std::time::{Duration, Instant};

/// 已接聽的通話
pub(super) struct DialogCtx {
    pub invite_raw: String,
    pub local_to_tag: String,
    pub answered_at: Instant,
    pub remote_from_tag: String,
    pub rtp_session: Option<RtpSession>,
    pub ack_received: bool,
    pub ack_timeout_secs: u64,
    pub bye_timeout_secs: u64,
    /// 座席主動掛斷的時間（行為模擬；None = 等對方 BYE）
    pub hangup_at: Option<Instant>,
    /// 協商出的 codec（RTP 與 RE-INVITE 的 answer 都用它）
    pub codec_pt: u8,
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
    pub fn agent_hangup_due(&self, now: Instant) -> bool {
        self.ack_received && self.hangup_at.is_some_and(|t| now >= t)
    }

    /// 座席要主動 BYE 的時間（ACK / BYE 逾時或行為模擬的掛斷，取較早者）
    pub fn deadline(&self) -> Instant {
        let timeout = self.timeout_at();
        match self.hangup_at {
            Some(t) if self.ack_received => timeout.min(t),
            _ => timeout,
        }
    }

    /// 座席主動 BYE 的原因（`deadline` 到期時呼叫）
    pub fn hangup_reason(&self, now: Instant) -> &'static str {
        if self.agent_hangup_due(now) {
            "座席掛斷"
        } else if !self.ack_received {
            "ACK timeout"
        } else {
            "BYE timeout"
        }
    }
}

/// 響鈴結束後要做的事
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum RingOutcome {
    Answer,
    /// 486 Busy Here
    Reject,
    /// 不接：等 CANCEL，逾時回 480
    NoAnswer,
}

impl RingOutcome {
    /// `roll` 為 [0, 100) 的隨機數
    pub fn decide(b: &AgentBehavior, roll: f64) -> Self {
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
pub(super) fn random_secs(min: f64, max: f64) -> Option<Duration> {
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
pub(super) struct PendingInvite {
    pub invite_raw: String,
    pub local_to_tag: String,
    pub remote_from_tag: String,
    pub outcome: RingOutcome,
    /// 響鈴結束（送出最終回應）的時間
    pub due: Instant,
}

/// 帶 SDP 的回應（接聽的 200 OK）
///
/// `contact_user` 要與 REGISTER 的 Contact 相同（座席帳號）：交換機以此對應到已註冊的座席，
/// 通話結束時才能把正確的座席恢復為可派話
pub(super) fn build_response_with_sdp(
    raw_request: &str,
    status_line: &str,
    to_tag: &str,
    contact_user: &str,
    local_addr: &str,
    rtp_port: u16,
    payloads: &[u8],
) -> String {
    let (via, from, to, call_id, cseq) = SipMessage::extract_request_headers(raw_request);
    let to_with_tag = crate::sip::message::inject_to_tag_if_missing(&to, to_tag);
    let sdp = sdp::body(&crate::net::host(local_addr), rtp_port, payloads);
    let sdp_len = sdp.len();
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
        user = contact_user,
        local = local_addr,
        sdp_len = sdp_len,
        sdp = sdp,
    )
}

/// 從 INVITE 原始訊息建構座席端主動 BYE
pub(super) fn build_bye_from_dialog(
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

/// 沒有 RTP 時 SDP 裡填的 port（偶數）
pub(super) fn pick_dummy_rtp_port() -> u16 {
    use rand::Rng;
    let p = rand::thread_rng().gen_range(16000..32000);
    if p % 2 == 0 {
        p
    } else {
        p + 1
    }
}

/// log 用的短 Call-ID
pub(super) fn short(call_id: &str) -> String {
    call_id.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            ctx.hangup_reason(now + Duration::from_secs(10)),
            "ACK timeout"
        );
        ctx.ack_received = true;
        assert_eq!(ctx.deadline(), now + Duration::from_secs(3));
        assert!(ctx.agent_hangup_due(now + Duration::from_secs(3)));
        assert_eq!(ctx.hangup_reason(now + Duration::from_secs(3)), "座席掛斷");
        ctx.hangup_at = Some(now + Duration::from_secs(500));
        assert_eq!(
            ctx.deadline(),
            now + Duration::from_secs(150),
            "BYE 逾時較早時以逾時為準"
        );
        assert_eq!(
            ctx.hangup_reason(now + Duration::from_secs(150)),
            "BYE timeout"
        );
    }

    #[test]
    fn dummy_rtp_port_is_even() {
        for _ in 0..100 {
            let p = pick_dummy_rtp_port();
            assert!(p.is_multiple_of(2) && (16000..=32000).contains(&p));
        }
    }
}
