/// 指標收集：ASR / ACD / PDD / 延遲直方圖
use crate::rtp::stats::RtpStatsSnapshot;
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

// ─── 即時原子計數器 ─────────────────────────────────────────────

/// 執行期間持續更新的輕量計數（原子操作，無鎖）
#[derive(Default)]
pub struct LiveStats {
    pub calls_initiated: AtomicU64,
    pub calls_answered:  AtomicU64,
    pub calls_completed: AtomicU64,
    pub calls_failed:    AtomicU64,
    pub calls_timeout:   AtomicU64,
    /// 目前進行中的通話（INVITE sent 到 completed/failed/timeout）
    pub calls_active:    std::sync::atomic::AtomicI64,
    /// 目前活躍的 RTP session 數（用於前端顯示 RTP 狀態）
    pub rtp_sessions:    AtomicU64,
}

impl LiveStats {
    pub fn on_invite(&self) {
        self.calls_initiated.fetch_add(1, Ordering::Relaxed);
        self.calls_active.fetch_add(1, Ordering::Relaxed);
    }
    pub fn on_answered(&self) {
        self.calls_answered.fetch_add(1, Ordering::Relaxed);
    }
    pub fn on_completed(&self) {
        self.calls_completed.fetch_add(1, Ordering::Relaxed);
        self.calls_active.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn on_failed(&self) {
        self.calls_failed.fetch_add(1, Ordering::Relaxed);
        self.calls_active.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn on_timeout(&self) {
        self.calls_timeout.fetch_add(1, Ordering::Relaxed);
        self.calls_active.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn on_rtp_start(&self) { self.rtp_sessions.fetch_add(1, Ordering::Relaxed); }
    pub fn on_rtp_stop(&self)  {
        // 防止下溢
        let _ = self.rtp_sessions.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            if v > 0 { Some(v - 1) } else { Some(0) }
        });
    }

    /// 快照（用於進度回報 / TUI 更新）
    pub fn snapshot(&self) -> StatsSnapshot {
        let initiated  = self.calls_initiated.load(Ordering::Relaxed);
        let answered   = self.calls_answered.load(Ordering::Relaxed);
        let completed  = self.calls_completed.load(Ordering::Relaxed);
        let failed     = self.calls_failed.load(Ordering::Relaxed);
        let timeout    = self.calls_timeout.load(Ordering::Relaxed);
        let active     = self.calls_active.load(Ordering::Relaxed).max(0) as u64;
        let rtp        = self.rtp_sessions.load(Ordering::Relaxed);

        let asr = if initiated > 0 {
            answered as f64 / initiated as f64 * 100.0
        } else {
            0.0
        };
        let error_rate = if initiated > 0 {
            (failed + timeout) as f64 / initiated as f64 * 100.0
        } else {
            0.0
        };

        StatsSnapshot {
            calls_initiated:  initiated,
            calls_answered:   answered,
            calls_completed:  completed,
            calls_failed:     failed,
            calls_timeout:    timeout,
            calls_concurrent: active,
            rtp_sessions:     rtp,
            asr,
            error_rate,
            rtp_mos:          None,
            rtp_loss_pct:     None,
            rtp_jitter_ms:    None,
            rtp_sent_packets: None,
            rtp_recv_packets: None,
            pdd_avg_ms:       None,
            finished:         false,
        }
    }
}

// ─── 快照（可序列化，用於 JSON / TUI） ──────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatsSnapshot {
    pub calls_initiated:  u64,
    pub calls_answered:   u64,
    pub calls_completed:  u64,
    pub calls_failed:     u64,
    pub calls_timeout:    u64,
    /// 目前進行中通話數
    pub calls_concurrent: u64,
    /// 目前活躍的 RTP session 數（> 0 表示音訊傳送中）
    pub rtp_sessions:     u64,
    /// Answer Seizure Ratio（%）
    pub asr: f64,
    /// Error Rate = (failed + timeout) / initiated × 100（%）
    pub error_rate: f64,
    /// 即時 RTP 品質（從所有活躍 session 聚合；None = RTP 未啟用或無活躍 session）
    pub rtp_mos:           Option<f64>,
    pub rtp_loss_pct:      Option<f64>,
    pub rtp_jitter_ms:     Option<f64>,
    pub rtp_sent_packets:  Option<u64>,
    pub rtp_recv_packets:  Option<u64>,
    /// 目前為止的平均 PDD（ms；None = 尚無樣本）
    #[serde(default)]
    pub pdd_avg_ms:        Option<f64>,
    /// 引擎是否已結束（用於前端偵測 max_total_calls 完成）
    pub finished: bool,
}

// ─── 詳細統計（直方圖，需要 Mutex） ────────────────────────────

/// 精確時延統計，用 HDR Histogram（解析度 1µs；PDD / 建立時間最大 60s，通話時長最大 24h）
pub struct DetailedStats {
    /// PDD 分佈（µs）
    pub pdd_hist: Mutex<Histogram<u64>>,
    /// 通話建立時間分佈（µs）
    pub setup_hist: Mutex<Histogram<u64>>,
    /// 通話持續時間分佈（µs）
    pub dur_hist: Mutex<Histogram<u64>>,

    // 錯誤碼分類（粗分）
    pub fail_4xx: AtomicU64,
    pub fail_5xx: AtomicU64,
    pub fail_6xx: AtomicU64,
    // 每個具體錯誤碼的計數（細分）
    pub fail_codes: Mutex<HashMap<u16, u64>>,
}

impl Default for DetailedStats {
    fn default() -> Self {
        // PDD / 建立時間受 INVITE timeout 限制，60s 足夠；通話時長可能很長（不限時長 / 座席端），上限 24h
        let make = |max_us: u64| Histogram::<u64>::new_with_bounds(1, max_us, 3).unwrap();
        Self {
            pdd_hist:   Mutex::new(make(60_000_000)),
            setup_hist: Mutex::new(make(60_000_000)),
            dur_hist:   Mutex::new(make(86_400_000_000)),
            fail_4xx:   AtomicU64::new(0),
            fail_5xx:   AtomicU64::new(0),
            fail_6xx:   AtomicU64::new(0),
            fail_codes: Mutex::new(HashMap::new()),
        }
    }
}

impl DetailedStats {
    /// 記錄 PDD（ms → µs）
    pub fn record_pdd(&self, ms: f64) {
        let us = (ms * 1000.0) as u64;
        if let Ok(mut h) = self.pdd_hist.lock() {
            let _ = h.record(us.max(1));
        }
    }

    /// 目前為止的平均 PDD（ms）；尚無樣本時回傳 None
    pub fn pdd_mean_ms(&self) -> Option<f64> {
        let h = self.pdd_hist.lock().ok()?;
        (!h.is_empty()).then(|| h.mean() / 1000.0)
    }

    /// 記錄通話建立時間（ms → µs）
    pub fn record_setup(&self, ms: f64) {
        let us = (ms * 1000.0) as u64;
        if let Ok(mut h) = self.setup_hist.lock() {
            let _ = h.record(us.max(1));
        }
    }

    /// 記錄通話持續時間（s → µs）
    pub fn record_duration(&self, secs: f64) {
        let us = (secs * 1_000_000.0) as u64;
        if let Ok(mut h) = self.dur_hist.lock() {
            let _ = h.record(us.max(1));
        }
    }

    /// 依狀態碼分類記錄失敗（粗分 + 細分）
    pub fn record_fail_code(&self, code: u16) {
        match code {
            400..=499 => { self.fail_4xx.fetch_add(1, Ordering::Relaxed); }
            500..=599 => { self.fail_5xx.fetch_add(1, Ordering::Relaxed); }
            600..=699 => { self.fail_6xx.fetch_add(1, Ordering::Relaxed); }
            _ => {}
        }
        if let Ok(mut map) = self.fail_codes.lock() {
            *map.entry(code).or_insert(0) += 1;
        }
    }
}

// ─── 最終報告 ────────────────────────────────────────────────────

/// 壓測結束後產生的完整報告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalReport {
    // 通話數量
    pub calls_initiated: u64,
    pub calls_answered:  u64,
    pub calls_completed: u64,
    pub calls_failed:    u64,
    pub calls_timeout:   u64,

    // 持續時間
    pub duration_secs: f64,

    // 關鍵指標
    /// Answer Seizure Ratio（%）
    pub asr: f64,
    /// Call Completion Rate（%）
    pub ccr: f64,
    /// 實際 CPS
    pub actual_cps: f64,
    /// Average Call Duration（秒）
    pub acd_secs: f64,

    // PDD 分位數（ms）
    pub pdd_p50_ms:  f64,
    pub pdd_p95_ms:  f64,
    pub pdd_p99_ms:  f64,
    pub pdd_max_ms:  f64,

    // 建立時間分位數（ms）
    pub setup_p50_ms: f64,
    pub setup_p95_ms: f64,
    pub setup_p99_ms: f64,
    pub setup_max_ms: f64,

    // 錯誤碼分類
    pub fail_4xx: u64,
    pub fail_5xx: u64,
    pub fail_6xx: u64,

    // ── RTP 聲音品質 ──
    /// MOS 估算（1.0 ~ 5.0，None = 未啟用 RTP）
    pub mos:              Option<f64>,
    /// 掉包率（%）
    pub loss_rate_pct:    Option<f64>,
    /// Jitter（ms）
    pub jitter_ms:        Option<f64>,
    /// 傳送 RTP 封包數
    pub rtp_sent:         Option<u64>,
    /// 接收 RTP 封包數
    pub rtp_recv:         Option<u64>,
    /// 亂序封包數
    pub rtp_out_of_order: Option<u64>,

    // ── 具體失敗回應碼（key = SIP 狀態碼，value = 次數）──
    pub fail_codes: HashMap<u16, u64>,
}

impl FinalReport {
    /// 由即時計數、延遲直方圖與 RTP 聚合結果組出最終報告（Caller / Agent 共用）
    pub fn build(
        snap:         &StatsSnapshot,
        detail:       &DetailedStats,
        elapsed_secs: f64,
        rtp:          Option<&RtpStatsSnapshot>,
    ) -> Self {
        // 直方圖單位為 µs，報告為 ms
        let us_to_ms = |h: &Histogram<u64>, q: f64| h.value_at_quantile(q) as f64 / 1000.0;
        let pdd_h   = detail.pdd_hist.lock().unwrap();
        let setup_h = detail.setup_hist.lock().unwrap();
        let dur_h   = detail.dur_hist.lock().unwrap();

        Self {
            calls_initiated: snap.calls_initiated,
            calls_answered:  snap.calls_answered,
            calls_completed: snap.calls_completed,
            calls_failed:    snap.calls_failed,
            calls_timeout:   snap.calls_timeout,
            duration_secs:   elapsed_secs,
            asr:             snap.asr,
            ccr: if snap.calls_initiated > 0 {
                snap.calls_completed as f64 / snap.calls_initiated as f64 * 100.0
            } else { 0.0 },
            actual_cps:   snap.calls_initiated as f64 / elapsed_secs.max(0.001),
            // 通話時長直方圖單位為 µs → 秒
            acd_secs:     if snap.calls_completed > 0 { dur_h.mean() / 1_000_000.0 } else { 0.0 },
            pdd_p50_ms:   us_to_ms(&pdd_h, 0.50),
            pdd_p95_ms:   us_to_ms(&pdd_h, 0.95),
            pdd_p99_ms:   us_to_ms(&pdd_h, 0.99),
            pdd_max_ms:   pdd_h.max() as f64 / 1000.0,
            setup_p50_ms: us_to_ms(&setup_h, 0.50),
            setup_p95_ms: us_to_ms(&setup_h, 0.95),
            setup_p99_ms: us_to_ms(&setup_h, 0.99),
            setup_max_ms: setup_h.max() as f64 / 1000.0,
            fail_4xx:     detail.fail_4xx.load(Ordering::Relaxed),
            fail_5xx:     detail.fail_5xx.load(Ordering::Relaxed),
            fail_6xx:     detail.fail_6xx.load(Ordering::Relaxed),
            fail_codes:   detail.fail_codes.lock().unwrap().clone(),
            mos:              rtp.map(|r| r.mos),
            loss_rate_pct:    rtp.map(|r| r.loss_rate_pct),
            jitter_ms:        rtp.map(|r| r.jitter_ms),
            rtp_sent:         rtp.map(|r| r.sent_packets),
            rtp_recv:         rtp.map(|r| r.recv_packets),
            rtp_out_of_order: rtp.map(|r| r.out_of_order),
        }
    }

    /// 輸出為 JSON 字串
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ─── 每通通話明細（CDR）────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallDetailRecord {
    pub call_id:       String,
    pub callee:        String,
    pub result:        String,
    pub response_code: u16,
    pub pdd_ms:        Option<f64>,
    pub setup_ms:      Option<f64>,
    pub duration_secs: Option<f64>,
    pub mos:           Option<f64>,
}

impl CallDetailRecord {
    pub fn csv_header() -> &'static str {
        "call_id,callee,result,response_code,pdd_ms,setup_ms,duration_secs,mos"
    }

    pub fn to_csv_row(&self) -> String {
        format!("{},{},{},{},{},{},{},{}",
            self.call_id,
            self.callee,
            self.result,
            self.response_code,
            self.pdd_ms.map_or("-".into(), |v| format!("{:.1}", v)),
            self.setup_ms.map_or("-".into(), |v| format!("{:.1}", v)),
            self.duration_secs.map_or("-".into(), |v| format!("{:.2}", v)),
            self.mos.map_or("-".into(), |v| format!("{:.2}", v)),
        )
    }
}

#[derive(Default)]
pub struct CdrCollector {
    records: Mutex<Vec<CallDetailRecord>>,
}

impl CdrCollector {
    pub fn push(&self, cdr: CallDetailRecord) {
        if let Ok(mut v) = self.records.lock() {
            v.push(cdr);
        }
    }

    pub fn take(&self) -> Vec<CallDetailRecord> {
        self.records.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acd_includes_calls_longer_than_60s() {
        let detail = DetailedStats::default();
        detail.record_duration(30.0);
        detail.record_duration(150.0); // 座席端 BYE timeout 預設 150s
        let snap = StatsSnapshot { calls_initiated: 2, calls_completed: 2, ..Default::default() };
        let r = FinalReport::build(&snap, &detail, 200.0, None);
        assert!((r.acd_secs - 90.0).abs() < 0.5, "超過 60s 的通話也應計入 ACD（實際 {}）", r.acd_secs);
    }
}
