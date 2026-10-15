//! 端到端對照：把民眾端每通電話和座席端收到的來電配對
//!
//! 配對順序：
//!   1. `P-CIN-CallId`：SIPX 在同一通電話的所有段（回給主叫的 18x / 200、送到座席的 INVITE）
//!      都帶相同的值，兩端都有時直接配對。ACD 改派時同一通會有多個座席收到 INVITE。
//!   2. 主叫號碼 + 時間窗：沒有 `P-CIN-CallId` 的來電，找主叫號碼相同、在時間窗內最近一通
//!      民眾端電話。同一號碼在時間窗內有多通候選時標記為「不確定」——民眾端用
//!      `caller_number_seq`（每通不同主叫號碼）就能避免。
use crate::trace::{CallOutcome, CallTrace};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 配對參數
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CorrelateOptions {
    /// 主叫號碼配對的時間窗：座席收到 INVITE 最晚可在民眾端 INVITE 後幾毫秒（含排隊時間）
    pub window_ms: u64,
    /// 兩端時鐘誤差容許值：座席端時間可比民眾端早幾毫秒
    pub clock_skew_ms: u64,
}

impl Default for CorrelateOptions {
    fn default() -> Self {
        Self {
            window_ms: 120_000,
            clock_skew_ms: 2_000,
        }
    }
}

/// 配對依據
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    CinCallId,
    CallerNumber,
}

/// 一通民眾端電話與座席端收到的所有 INVITE（ACD 改派時不只一個）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallPair {
    pub caller: CallTrace,
    /// 依座席收到 INVITE 的時間排序
    pub offers: Vec<CallTrace>,
    pub matched_by: Option<MatchKind>,
    /// 主叫號碼配對時時間窗內有多通候選（配對可能錯誤）
    pub ambiguous: bool,
}

impl CallPair {
    /// 接起這通的座席端紀錄
    pub fn answered_offer(&self) -> Option<&CallTrace> {
        self.offers
            .iter()
            .find(|o| o.outcome == CallOutcome::Answered)
    }

    /// 分配延遲：民眾端送出 INVITE → 第一個座席收到 INVITE（ms，時鐘誤差可能讓它略小於 0）
    pub fn distribution_ms(&self) -> Option<i64> {
        self.offers
            .first()
            .map(|o| o.invite_at_ms as i64 - self.caller.invite_at_ms as i64)
    }

    /// 民眾端送出 INVITE → 座席接起（ms）
    pub fn agent_answer_ms(&self) -> Option<i64> {
        let at = self.answered_offer()?.answered_at_ms?;
        Some(at as i64 - self.caller.invite_at_ms as i64)
    }
}

/// 延遲分布（ms）
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct DelayStats {
    pub count: u64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl DelayStats {
    fn from_values(mut v: Vec<f64>) -> Self {
        if v.is_empty() {
            return Self::default();
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let pick = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        Self {
            count: v.len() as u64,
            mean_ms: v.iter().sum::<f64>() / v.len() as f64,
            p50_ms: pick(0.50),
            p95_ms: pick(0.95),
            max_ms: *v.last().unwrap(),
        }
    }
}

/// 雙向音質（只計兩端都有 MOS 的通話）
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct MosCompare {
    pub pairs: u64,
    /// 民眾端收到的音質（座席 → 民眾方向）
    pub caller_avg: Option<f64>,
    pub caller_min: Option<f64>,
    /// 座席端收到的音質（民眾 → 座席方向）
    pub agent_avg: Option<f64>,
    pub agent_min: Option<f64>,
}

/// 端到端對照結果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CorrelationReport {
    pub options: CorrelateOptions,
    /// 民眾端發起 / 接通（SIPX 回 200，可能是 IVR 先接）
    pub caller_calls: u64,
    pub caller_answered: u64,
    /// 至少一個座席收到 INVITE 的通數
    pub delivered: u64,
    /// 有座席接起的通數
    pub agent_answered: u64,
    /// 漏接：民眾端接通、但沒有座席接起
    pub missed: u64,
    /// 漏接率 = missed / caller_answered（%）
    pub miss_rate_pct: f64,
    /// 漏接中完全沒有座席收到 INVITE 的通數
    pub not_delivered: u64,
    /// 改派：同一通有多個座席收到 INVITE 的通數，以及多出來的 INVITE 數
    pub rerouted: u64,
    pub extra_offers: u64,
    pub matched_by_cin: u64,
    pub matched_by_number: u64,
    /// 主叫號碼配對中不確定的通數
    pub ambiguous: u64,
    /// 帶 `P-CIN-CallId` 的比例（%）——民眾端偏低代表 SIPX 沒有回給主叫，只能靠號碼配對
    pub caller_cin_pct: f64,
    pub agent_cin_pct: f64,
    /// 民眾端 INVITE → 第一個座席收到 INVITE
    pub distribution: DelayStats,
    /// 民眾端 INVITE → 座席接起
    pub agent_answer: DelayStats,
    pub mos: MosCompare,
    pub pairs: Vec<CallPair>,
    /// 配不到民眾端電話的座席來電（非本次壓測的來電，或時間窗太小）
    pub orphans: Vec<CallTrace>,
}

impl CorrelationReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// 只留數字、去掉開頭的 0 再比較；SIPX 可能加前綴（國碼 / 撥出碼），
/// 尾碼相同且夠長也視為同一號碼（`+886912345678` ≈ `0912345678`）
fn numbers_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| -> String {
        let d: String = s.chars().filter(char::is_ascii_digit).collect();
        d.trim_start_matches('0').to_string()
    };
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || (a.len().min(b.len()) >= 6 && (a.ends_with(&b) || b.ends_with(&a)))
}

pub fn correlate(
    caller: &[CallTrace],
    agent: &[CallTrace],
    opt: CorrelateOptions,
) -> CorrelationReport {
    let mut pairs: Vec<CallPair> = caller
        .iter()
        .map(|c| CallPair {
            caller: c.clone(),
            offers: Vec::new(),
            matched_by: None,
            ambiguous: false,
        })
        .collect();

    // ── 1. P-CIN-CallId ──
    let mut by_cin: HashMap<&str, usize> = HashMap::new();
    for (i, p) in pairs.iter().enumerate() {
        if let Some(cin) = p.caller.cin_call_id.as_deref() {
            by_cin.entry(cin).or_insert(i);
        }
    }
    let mut rest: Vec<&CallTrace> = Vec::new();
    let mut cin_hits: Vec<(usize, &CallTrace)> = Vec::new();
    for o in agent {
        match o.cin_call_id.as_deref().and_then(|c| by_cin.get(c)) {
            Some(&i) => cin_hits.push((i, o)),
            None => rest.push(o),
        }
    }
    for (i, o) in cin_hits {
        pairs[i].offers.push(o.clone());
        pairs[i].matched_by = Some(MatchKind::CinCallId);
    }

    // ── 2. 主叫號碼 + 時間窗（只配給還沒有 P-CIN-CallId 配對的民眾端電話）──
    rest.sort_by_key(|o| o.invite_at_ms);
    let mut orphans = Vec::new();
    for o in rest {
        let candidates: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.matched_by != Some(MatchKind::CinCallId))
            // 兩端都有 P-CIN-CallId 但不同 → 一定不是同一通
            .filter(|(_, p)| !matches!((&p.caller.cin_call_id, &o.cin_call_id), (Some(a), Some(b)) if a != b))
            .filter(|(_, p)| numbers_match(&p.caller.caller_number, &o.caller_number))
            .filter(|(_, p)| {
                let c = p.caller.invite_at_ms;
                o.invite_at_ms + opt.clock_skew_ms >= c && o.invite_at_ms <= c + opt.window_ms
            })
            .map(|(i, _)| i)
            .collect();
        // 已有座席接起的通話通常不會再被分配，優先排除
        let open: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&i| pairs[i].answered_offer().is_none())
            .collect();
        let pool = if open.is_empty() { &candidates } else { &open };
        // 時間上最接近（最晚送出）的那一通
        match pool
            .iter()
            .copied()
            .max_by_key(|&i| pairs[i].caller.invite_at_ms)
        {
            Some(i) => {
                let p = &mut pairs[i];
                p.offers.push(o.clone());
                p.matched_by = Some(MatchKind::CallerNumber);
                p.ambiguous |= pool.len() > 1;
            }
            None => orphans.push(o.clone()),
        }
    }
    for p in &mut pairs {
        p.offers.sort_by_key(|o| o.invite_at_ms);
    }

    // ── 統計 ──
    let pct = |n: usize, d: usize| {
        if d == 0 {
            0.0
        } else {
            n as f64 * 100.0 / d as f64
        }
    };
    let caller_answered = pairs
        .iter()
        .filter(|p| p.caller.outcome == CallOutcome::Answered)
        .count();
    let missed: Vec<&CallPair> = pairs
        .iter()
        .filter(|p| p.caller.outcome == CallOutcome::Answered && p.answered_offer().is_none())
        .collect();

    let mut caller_mos = Vec::new();
    let mut agent_mos = Vec::new();
    for p in &pairs {
        if let (Some(c), Some(a)) = (p.caller.mos, p.answered_offer().and_then(|o| o.mos)) {
            caller_mos.push(c);
            agent_mos.push(a);
        }
    }
    let avg = |v: &[f64]| (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64);
    let min = |v: &[f64]| v.iter().copied().reduce(f64::min);

    CorrelationReport {
        options: opt,
        caller_calls: pairs.len() as u64,
        caller_answered: caller_answered as u64,
        delivered: pairs.iter().filter(|p| !p.offers.is_empty()).count() as u64,
        agent_answered: pairs
            .iter()
            .filter(|p| p.answered_offer().is_some())
            .count() as u64,
        missed: missed.len() as u64,
        miss_rate_pct: pct(missed.len(), caller_answered),
        not_delivered: missed.iter().filter(|p| p.offers.is_empty()).count() as u64,
        rerouted: pairs.iter().filter(|p| p.offers.len() > 1).count() as u64,
        extra_offers: pairs
            .iter()
            .map(|p| p.offers.len().saturating_sub(1) as u64)
            .sum(),
        matched_by_cin: pairs
            .iter()
            .filter(|p| p.matched_by == Some(MatchKind::CinCallId))
            .count() as u64,
        matched_by_number: pairs
            .iter()
            .filter(|p| p.matched_by == Some(MatchKind::CallerNumber))
            .count() as u64,
        ambiguous: pairs.iter().filter(|p| p.ambiguous).count() as u64,
        caller_cin_pct: pct(
            caller.iter().filter(|t| t.cin_call_id.is_some()).count(),
            caller.len(),
        ),
        agent_cin_pct: pct(
            agent.iter().filter(|t| t.cin_call_id.is_some()).count(),
            agent.len(),
        ),
        distribution: DelayStats::from_values(
            pairs
                .iter()
                .filter_map(|p| p.distribution_ms())
                .map(|v| v as f64)
                .collect(),
        ),
        agent_answer: DelayStats::from_values(
            pairs
                .iter()
                .filter_map(|p| p.agent_answer_ms())
                .map(|v| v as f64)
                .collect(),
        ),
        mos: MosCompare {
            pairs: caller_mos.len() as u64,
            caller_avg: avg(&caller_mos),
            caller_min: min(&caller_mos),
            agent_avg: avg(&agent_mos),
            agent_min: min(&agent_mos),
        },
        pairs,
        orphans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(id: &str, cin: Option<&str>, num: &str, at: u64, outcome: CallOutcome) -> CallTrace {
        CallTrace {
            sip_call_id: id.into(),
            cin_call_id: cin.map(Into::into),
            caller_number: num.into(),
            invite_at_ms: at,
            answered_at_ms: (outcome == CallOutcome::Answered).then_some(at + 100),
            outcome,
            ..Default::default()
        }
    }

    fn offer(id: &str, cin: Option<&str>, num: &str, at: u64, answered: bool) -> CallTrace {
        CallTrace {
            sip_call_id: id.into(),
            cin_call_id: cin.map(Into::into),
            caller_number: num.into(),
            agent: Some("1001".into()),
            invite_at_ms: at,
            answered_at_ms: answered.then_some(at + 500),
            outcome: if answered {
                CallOutcome::Answered
            } else {
                CallOutcome::Failed
            },
            ..Default::default()
        }
    }

    const A: CallOutcome = CallOutcome::Answered;

    #[test]
    fn pairs_by_cin_call_id_including_reroutes() {
        let callers = [
            caller("c1", Some("C1"), "0900", 1_000, A),
            caller("c2", Some("C2"), "0900", 1_100, A),
        ];
        // C1 先派給座席 A 沒接、改派座席 B 接起；C2 沒有座席收到
        let agents = [
            offer("a2", Some("C1"), "0900", 4_000, true),
            offer("a1", Some("C1"), "0900", 1_300, false),
        ];
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        assert_eq!(r.matched_by_cin, 1);
        assert_eq!(
            r.pairs[0]
                .offers
                .iter()
                .map(|o| o.sip_call_id.as_str())
                .collect::<Vec<_>>(),
            ["a1", "a2"]
        );
        assert_eq!(r.pairs[0].distribution_ms(), Some(300));
        assert_eq!(r.pairs[0].agent_answer_ms(), Some(3_500));
        assert_eq!((r.rerouted, r.extra_offers), (1, 1));
        assert_eq!(
            (
                r.caller_answered,
                r.agent_answered,
                r.missed,
                r.not_delivered
            ),
            (2, 1, 1, 1)
        );
        assert_eq!(r.miss_rate_pct, 50.0);
        assert!(r.orphans.is_empty());
    }

    #[test]
    fn falls_back_to_caller_number_within_window() {
        let callers = [
            caller("c1", None, "0912000001", 10_000, A),
            caller("c2", None, "0912000002", 10_050, A),
            caller("c3", None, "0912000003", 10_100, CallOutcome::Timeout),
        ];
        let agents = [
            // 主叫號碼加了國碼前綴，時鐘比民眾端慢 1 秒
            offer("a1", None, "+886912000002", 9_200, true),
            offer("a2", None, "0912000001", 11_000, true),
            // 超出時間窗
            offer("a3", None, "0912000003", 10_100 + 120_001, true),
            // 不認識的號碼
            offer("a4", None, "0999", 10_000, true),
        ];
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        assert_eq!(r.pairs[0].offers[0].sip_call_id, "a2");
        assert_eq!(r.pairs[1].offers[0].sip_call_id, "a1");
        assert!(r.pairs[2].offers.is_empty());
        assert_eq!(r.matched_by_number, 2);
        assert_eq!(r.ambiguous, 0);
        assert_eq!(
            r.orphans
                .iter()
                .map(|o| o.sip_call_id.as_str())
                .collect::<Vec<_>>(),
            ["a4", "a3"]
        );
        assert_eq!(
            r.pairs[1].distribution_ms(),
            Some(-850),
            "時鐘誤差可能讓延遲為負"
        );
        assert_eq!(r.missed, 0, "逾時的通話不算接通，也不算漏接");
    }

    #[test]
    fn same_number_picks_latest_open_call_and_flags_ambiguity() {
        let callers = [
            caller("c1", None, "1000", 1_000, A),
            caller("c2", None, "1000", 2_000, A),
            caller("c3", None, "1000", 9_000, A),
        ];
        let agents = [
            offer("a1", None, "1000", 2_500, true), // c1、c2 都在窗內 → 選較晚的 c2
            offer("a2", None, "1000", 3_000, true), // c2 已有座席接起 → 改配 c1
        ];
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        assert_eq!(r.pairs[1].offers[0].sip_call_id, "a1");
        assert_eq!(r.pairs[0].offers[0].sip_call_id, "a2");
        assert!(r.pairs[1].ambiguous);
        assert!(!r.pairs[0].ambiguous, "c2 已被接起，剩 c1 一個候選");
        assert_eq!(r.ambiguous, 1);
        assert_eq!(r.missed, 1);
    }

    #[test]
    fn different_cin_ids_never_match_by_number() {
        let callers = [caller("c1", Some("C1"), "1000", 1_000, A)];
        let agents = [offer("a1", Some("C9"), "1000", 1_200, true)];
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        assert!(r.pairs[0].offers.is_empty());
        assert_eq!(r.orphans.len(), 1);
    }

    #[test]
    fn delay_and_mos_statistics() {
        let mut callers = Vec::new();
        let mut agents = Vec::new();
        for i in 0..10u64 {
            let cin = format!("C{}", i);
            let mut c = caller(&format!("c{}", i), Some(&cin), "1", 1_000 * i, A);
            c.mos = Some(4.0 + i as f64 * 0.01);
            let mut o = offer(
                &format!("a{}", i),
                Some(&cin),
                "1",
                1_000 * i + (i + 1) * 10,
                true,
            );
            o.mos = Some(3.0);
            callers.push(c);
            agents.push(o);
        }
        // 只有一端有 MOS 的不列入
        callers.push(caller("cx", Some("CX"), "1", 50_000, A));
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        assert_eq!(r.distribution.count, 10);
        assert_eq!(r.distribution.p50_ms, 60.0);
        assert_eq!(r.distribution.max_ms, 100.0);
        assert_eq!(r.distribution.mean_ms, 55.0);
        assert_eq!(r.mos.pairs, 10);
        assert_eq!(r.mos.agent_avg, Some(3.0));
        assert!((r.mos.caller_avg.unwrap() - 4.045).abs() < 1e-9);
        assert_eq!(r.mos.caller_min, Some(4.0));
        assert!((r.caller_cin_pct - 100.0).abs() < 1e-9);
    }

    #[test]
    fn number_normalization() {
        assert!(numbers_match("0912-345-678", "0912345678"));
        assert!(numbers_match("+886912345678", "0912345678"));
        assert!(
            numbers_match("00966551552", "0966551552"),
            "SIPX 外撥時加的 0"
        );
        assert!(!numbers_match("0912345678", "0912345679"));
        assert!(!numbers_match("12345", "2345"), "太短不做尾碼比對");
        assert!(!numbers_match("", ""));
    }
}
