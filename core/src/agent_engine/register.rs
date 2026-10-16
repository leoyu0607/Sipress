//! 單一帳號的註冊狀態：REGISTER 的 CSeq / Digest challenge / 重傳，以及已註冊計數
use crate::sip::register::DigestChallenge;
use crate::stats::LiveStats;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 第一次 REGISTER 的 Expires（秒）
pub(super) const INITIAL_EXPIRES: u32 = 240;

/// 同一輪 REGISTER 收到 401/407 後最多帶認證重送幾次（避免密碼錯誤時無限重送）
pub(super) const MAX_AUTH_RETRIES: u32 = 2;

const REG_T1: Duration = Duration::from_millis(500);
const REG_T2: Duration = Duration::from_secs(4);

/// 一個帳號的註冊 dialog
pub(super) struct Registration {
    /// 整個測試期間不變的 From tag / Call-ID（同一個註冊 dialog）
    pub from_tag: String,
    pub call_id: String,
    pub cseq: u32,
    /// 最近一次收到的 401 / 407 challenge；之後的 REGISTER 都帶它的認證
    pub challenge: Option<DigestChallenge>,
    /// 自上次 200 OK 以來連續帶認證重送的次數
    pub auth_retries: u32,
    /// 伺服器同意的有效期（秒）
    pub expires: u32,
    /// 最近一次送出或成功的時間（刷新排程由此起算）
    pub last_register_at: Instant,
    /// UDP 時尚未收到最終回應的 REGISTER（重傳用）
    retx: Option<RegRetx>,
    pub gauge: RegisteredGauge,
}

/// REGISTER 重傳（RFC 3261 Timer E / F：T1 起每次加倍、上限 T2，64×T1 後放棄）
struct RegRetx {
    msg: String,
    cseq: u32,
    interval: Duration,
    next_at: Instant,
    give_up_at: Instant,
}

impl Registration {
    pub fn new(from_tag: String, call_id: String, live: Arc<LiveStats>) -> Self {
        Self {
            from_tag,
            call_id,
            cseq: 0,
            challenge: None,
            auth_retries: 0,
            expires: INITIAL_EXPIRES,
            last_register_at: Instant::now(),
            retx: None,
            gauge: RegisteredGauge::new(live),
        }
    }

    /// 下一次刷新的時間：Expires 的一半，最少 60 秒
    pub fn refresh_at(&self) -> Instant {
        self.last_register_at + Duration::from_secs((self.expires as u64 / 2).max(60))
    }

    /// 送出 REGISTER 後呼叫：UDP 時排定重傳（TCP 由傳輸層保證送達）
    pub fn arm(&mut self, msg: &str, reliable: bool) {
        let now = Instant::now();
        self.retx = (!reliable).then(|| RegRetx {
            msg: msg.to_string(),
            cseq: self.cseq,
            interval: REG_T1,
            next_at: now + REG_T1,
            give_up_at: now + REG_T1 * 64,
        });
    }

    /// 收到 REGISTER 的最終回應：停止該 CSeq 的重傳
    pub fn on_final(&mut self, cseq: Option<u32>) {
        if self.retx.as_ref().is_some_and(|r| Some(r.cseq) == cseq) {
            self.retx = None;
        }
    }

    pub fn retx_at(&self) -> Option<Instant> {
        self.retx.as_ref().map(|r| r.next_at.min(r.give_up_at))
    }

    /// 重傳時間到：回傳要重送的訊息；超過放棄時間時回傳 Err（停止重傳）
    pub fn due(&mut self, now: Instant) -> Result<Option<String>, ()> {
        let Some(r) = self.retx.as_mut() else {
            return Ok(None);
        };
        if now >= r.give_up_at {
            self.retx = None;
            return Err(());
        }
        if now < r.next_at {
            return Ok(None);
        }
        r.interval = (r.interval * 2).min(REG_T2);
        r.next_at = now + r.interval;
        Ok(Some(r.msg.clone()))
    }
}

/// 單一帳號是否已註冊；狀態改變時才增減 `LiveStats::agents_registered`，drop 時自動取消
pub(super) struct RegisteredGauge {
    live: Arc<LiveStats>,
    on: bool,
    /// 最近一次 REGISTER 200 OK 的時間
    last_ok: Instant,
}

impl RegisteredGauge {
    pub fn new(live: Arc<LiveStats>) -> Self {
        Self {
            live,
            on: false,
            last_ok: Instant::now(),
        }
    }

    pub fn set(&mut self, on: bool) {
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

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// 收到 REGISTER 200 OK（Expires=0 代表伺服器已移除註冊）
    pub fn on_ok(&mut self, expires: Option<u32>) {
        self.last_ok = Instant::now();
        self.set(expires != Some(0));
    }

    /// 已註冊但超過 expires 秒都沒有再收到 200 OK
    pub fn expired(&self, expires: u32) -> bool {
        self.on && self.last_ok.elapsed() > Duration::from_secs(expires as u64)
    }
}

impl Drop for RegisteredGauge {
    fn drop(&mut self) {
        self.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn registered(live: &LiveStats) -> u64 {
        live.agents_registered.load(Ordering::Relaxed)
    }

    fn reg() -> Registration {
        Registration::new("t".into(), "c".into(), Arc::default())
    }

    #[test]
    fn register_retransmission_timer() {
        let mut st = reg();
        st.cseq = 3;
        st.arm("REGISTER x", true);
        assert!(st.retx_at().is_none(), "TCP 不重傳");

        st.arm("REGISTER x", false);
        let t0 = Instant::now();
        assert_eq!(st.due(t0), Ok(None), "還沒到 T1");
        let mut t = t0 + REG_T1;
        let mut gaps = Vec::new();
        for _ in 0..5 {
            assert_eq!(st.due(t), Ok(Some("REGISTER x".into())));
            let next = st.retx_at().unwrap();
            gaps.push(next.duration_since(t).as_millis());
            t = next;
        }
        assert_eq!(gaps, vec![1000, 2000, 4000, 4000, 4000], "加倍、上限 T2");

        // 舊 CSeq 的最終回應不停止重傳；同 CSeq 的才停止
        st.on_final(Some(2));
        assert!(st.retx_at().is_some());
        st.on_final(Some(3));
        assert!(st.retx_at().is_none());

        // 64 × T1 後放棄
        st.arm("REGISTER y", false);
        assert_eq!(st.due(Instant::now() + REG_T1 * 64), Err(()));
        assert!(st.retx_at().is_none());
    }

    #[test]
    fn refresh_at_half_of_expires_but_at_least_60s() {
        let mut r = reg();
        let t = r.last_register_at;
        assert_eq!(r.refresh_at(), t + Duration::from_secs(120));
        r.expires = 60;
        assert_eq!(r.refresh_at(), t + Duration::from_secs(60));
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
