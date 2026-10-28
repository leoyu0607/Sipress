//! 分時段統計：長時間耐久測試每 N 秒一段的 ASR / PDD / MOS / 失敗碼
//!
//! 引擎每秒呼叫一次 `TimelineRecorder::tick`，跨過時段邊界時以「累計值的差」結算一段：
//! 計數器與失敗碼直接相減，PDD 以直方圖相減後取分位數，MOS 為該段每秒即時 MOS 的平均。
//!
//! 引擎另外以固定 10 秒收集一份（`FINE_INTERVAL_SECS`，與主機監控取樣相同），
//! 每段附上 `RtpBucketStats`：每通電話只算本段新增的封包（`RtpStats::take_interval`）。

use crate::rtp::stats::RtpInterval;
use crate::stats::{DetailedStats, LiveStats};
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;

/// 一個時段的統計（時間為測試開始後的秒數）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct TimelineBucket {
    pub start_secs: f64,
    pub end_secs: f64,
    /// 本段發起（座席端為收到）的通話數
    pub initiated: u64,
    pub answered: u64,
    pub completed: u64,
    pub failed: u64,
    pub timeout: u64,
    /// 本段 ASR（%）：本段有結果的通話（接通 + 失敗 + 逾時）中接通的比例；本段沒有結果時為 None
    pub asr: Option<f64>,
    pub pdd_p50_ms: Option<f64>,
    pub pdd_p95_ms: Option<f64>,
    /// 本段每秒即時 MOS 的平均（沒有 RTP 時為 None）
    pub mos: Option<f64>,
    /// 本段的失敗回應碼（狀態碼 → 次數）
    pub fail_codes: BTreeMap<u16, u64>,
    /// 本段結束時進行中的通話數
    #[serde(default)]
    pub active: u64,
    /// 本段的 RTP 品質（只算本段新增的封包；只有 10 秒時間軸有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtp: Option<RtpBucketStats>,
}

/// 10 秒時間軸的固定時段長度
pub const FINE_INTERVAL_SECS: u64 = 10;

/// 判定 MOS 不達標的門檻（之後改由門檻設定提供）
pub const MOS_THRESHOLD: f64 = 4.0;

/// 一個時段的 RTP 品質：每通電話只算本段新增的封包，再彙整
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RtpBucketStats {
    /// 本段有 RTP 的通話數
    pub calls: u64,
    /// 本段的應收、掉包封包數（合併時段時用來重算掉包率）
    #[serde(default)]
    pub expected_packets: u64,
    #[serde(default)]
    pub lost_packets: u64,
    /// 掉包率（%）：本段所有通話的掉包數加總 ÷ 應收封包數加總
    pub loss_pct: f64,
    /// 本段掉包最嚴重的單通電話（%）
    pub loss_max_pct: f64,
    /// Jitter（ms）：依應收封包數加權平均，以及每通電話的 P95
    pub jitter_avg_ms: f64,
    pub jitter_p95_ms: f64,
    /// MOS（E-Model）：每通電話一票的平均、最低值、P5
    pub mos_avg: f64,
    pub mos_min: f64,
    pub mos_p5: f64,
    /// MOS（jitter buffer 50 ms 模擬）平均
    pub mos_jb50_avg: f64,
    /// MOS（jitter buffer 200 ms 模擬）：判定用
    pub mos_jb200_avg: f64,
    pub mos_jb200_min: f64,
    pub mos_jb200_p5: f64,
    /// MOS（jitter buffer 200 ms）低於門檻的通話數
    pub below_threshold: u64,
}

impl RtpBucketStats {
    /// 彙整本段每通電話的增量；沒有任何通話收發 RTP 時為 None
    pub fn from_intervals(samples: &[RtpInterval]) -> Option<Self> {
        let calls: Vec<&RtpInterval> = samples.iter().filter(|s| s.expected > 0).collect();
        if calls.is_empty() {
            return None;
        }
        let n = calls.len() as f64;
        let expected: u64 = calls.iter().map(|s| s.expected).sum();
        let lost: u64 = calls.iter().map(|s| s.lost).sum();
        let mos: Vec<f64> = calls.iter().map(|s| s.mos()).collect();
        let jb200: Vec<f64> = calls.iter().map(|s| s.mos_jb_large()).collect();
        let jitter: Vec<f64> = calls.iter().map(|s| s.jitter_ms).collect();
        let avg = |v: &[f64]| v.iter().sum::<f64>() / n;
        let min = |v: &[f64]| v.iter().cloned().fold(f64::INFINITY, f64::min);
        Some(Self {
            calls: calls.len() as u64,
            expected_packets: expected,
            lost_packets: lost,
            loss_pct: lost as f64 / expected as f64 * 100.0,
            loss_max_pct: calls
                .iter()
                .map(|s| s.loss_rate() * 100.0)
                .fold(0.0, f64::max),
            jitter_avg_ms: calls
                .iter()
                .map(|s| s.jitter_ms * s.expected as f64)
                .sum::<f64>()
                / expected as f64,
            jitter_p95_ms: quantile(&jitter, 0.95),
            mos_avg: avg(&mos),
            mos_min: min(&mos),
            mos_p5: quantile(&mos, 0.05),
            mos_jb50_avg: calls.iter().map(|s| s.mos_jb_small()).sum::<f64>() / n,
            mos_jb200_avg: avg(&jb200),
            mos_jb200_min: min(&jb200),
            mos_jb200_p5: quantile(&jb200, 0.05),
            below_threshold: jb200.iter().filter(|&&m| m < MOS_THRESHOLD).count() as u64,
        })
    }
}

/// 最近排名法的分位數（`q` = 0.0 ~ 1.0）
fn quantile(values: &[f64], q: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len());
    v[rank - 1]
}

impl TimelineBucket {
    pub fn duration_secs(&self) -> f64 {
        self.end_secs - self.start_secs
    }

    /// 本段實際 CPS
    pub fn cps(&self) -> f64 {
        self.initiated as f64 / self.duration_secs().max(0.001)
    }

    /// CSV 標頭（與 `csv_row` 對應）
    pub const CSV_HEADER: &'static str =
        "start_secs,end_secs,initiated,answered,completed,failed,timeout,cps,asr,pdd_p50_ms,pdd_p95_ms,mos,fail_codes";

    pub fn csv_row(&self) -> String {
        let opt =
            |v: Option<f64>, prec: usize| v.map_or(String::new(), |x| format!("{:.*}", prec, x));
        let codes: Vec<String> = self
            .fail_codes
            .iter()
            .map(|(c, n)| format!("{}:{}", c, n))
            .collect();
        format!(
            "{:.0},{:.0},{},{},{},{},{},{:.2},{},{},{},{},{}",
            self.start_secs,
            self.end_secs,
            self.initiated,
            self.answered,
            self.completed,
            self.failed,
            self.timeout,
            self.cps(),
            opt(self.asr, 2),
            opt(self.pdd_p50_ms, 1),
            opt(self.pdd_p95_ms, 1),
            opt(self.mos, 2),
            codes.join(" "),
        )
    }
}

#[derive(Default, Clone, Copy, PartialEq)]
struct Counters {
    initiated: u64,
    answered: u64,
    completed: u64,
    failed: u64,
    timeout: u64,
}

impl Counters {
    fn read(live: &LiveStats) -> Self {
        Self {
            initiated: live.calls_initiated.load(Ordering::Relaxed),
            answered: live.calls_answered.load(Ordering::Relaxed),
            completed: live.calls_completed.load(Ordering::Relaxed),
            failed: live.calls_failed.load(Ordering::Relaxed),
            timeout: live.calls_timeout.load(Ordering::Relaxed),
        }
    }
}

/// 分時段統計的收集器（`interval_secs` = 0 時不收集）
pub struct TimelineRecorder {
    interval_secs: f64,
    bucket_start: f64,
    prev: Counters,
    prev_pdd: Option<Histogram<u64>>,
    prev_fail: HashMap<u16, u64>,
    mos_sum: f64,
    mos_n: u32,
    /// 本段已收集的 RTP 增量（見 `add_rtp`）
    rtp: Vec<RtpInterval>,
    buckets: Vec<TimelineBucket>,
}

impl TimelineRecorder {
    pub fn new(interval_secs: u64) -> Self {
        Self {
            interval_secs: interval_secs as f64,
            bucket_start: 0.0,
            prev: Counters::default(),
            prev_pdd: None,
            prev_fail: HashMap::new(),
            mos_sum: 0.0,
            mos_n: 0,
            rtp: Vec::new(),
            buckets: Vec::new(),
        }
    }

    /// 下一次 `tick(elapsed)` 是否會結算時段；引擎據此在結算前收集各通話的 RTP 增量
    pub fn due(&self, elapsed: f64) -> bool {
        self.interval_secs > 0.0 && elapsed >= self.bucket_start + self.interval_secs
    }

    /// 加入 RTP 增量（進行中通話在結算前取一次；通話結束時取最後一段），併入下一個結算的時段
    pub fn add_rtp(&mut self, samples: impl IntoIterator<Item = RtpInterval>) {
        self.rtp.extend(samples);
    }

    /// 最近結算的一段
    pub fn last(&self) -> Option<&TimelineBucket> {
        self.buckets.last()
    }

    /// 每秒呼叫：`elapsed` 為測試開始後的秒數，`mos` 為此刻的即時 MOS
    pub fn tick(
        &mut self,
        elapsed: f64,
        live: &LiveStats,
        detail: &DetailedStats,
        mos: Option<f64>,
    ) {
        if self.interval_secs <= 0.0 {
            return;
        }
        if let Some(m) = mos {
            self.mos_sum += m;
            self.mos_n += 1;
        }
        while elapsed >= self.bucket_start + self.interval_secs {
            let end = self.bucket_start + self.interval_secs;
            self.close(end, live, detail);
        }
    }

    /// 測試結束：結算最後一段（不足一個時段也算），回傳所有時段
    pub fn finish(
        mut self,
        elapsed: f64,
        live: &LiveStats,
        detail: &DetailedStats,
    ) -> Vec<TimelineBucket> {
        if self.interval_secs <= 0.0 {
            return Vec::new();
        }
        self.tick(elapsed, live, detail, None);
        let tail = elapsed - self.bucket_start;
        if tail >= 1.0 || self.buckets.is_empty() {
            self.close(elapsed.max(self.bucket_start), live, detail);
        } else if Counters::read(live) != self.prev {
            // 不到 1 秒的尾段（通常是時間到後的收尾）併入上一段，避免圖表最後掉到 0
            self.close(elapsed, live, detail);
            let t = self.buckets.pop().unwrap();
            let last = self.buckets.last_mut().unwrap();
            last.end_secs = t.end_secs;
            last.initiated += t.initiated;
            last.answered += t.answered;
            last.completed += t.completed;
            last.failed += t.failed;
            last.timeout += t.timeout;
            let decided = last.answered + last.failed + last.timeout;
            last.asr = (decided > 0).then(|| last.answered as f64 / decided as f64 * 100.0);
            for (code, n) in t.fail_codes {
                *last.fail_codes.entry(code).or_default() += n;
            }
            last.active = t.active;
            if last.rtp.is_none() {
                last.rtp = t.rtp;
            }
        }
        self.buckets
    }

    fn close(&mut self, end: f64, live: &LiveStats, detail: &DetailedStats) {
        let now = Counters::read(live);
        let d = |a: u64, b: u64| a.saturating_sub(b);
        let (answered, failed, timeout) = (
            d(now.answered, self.prev.answered),
            d(now.failed, self.prev.failed),
            d(now.timeout, self.prev.timeout),
        );
        let decided = answered + failed + timeout;

        // PDD：累計直方圖 − 上一段結束時的累計直方圖
        let (pdd_p50_ms, pdd_p95_ms) = match detail.pdd_hist.lock() {
            Ok(cur) => {
                let mut seg = cur.clone();
                if let Some(prev) = &self.prev_pdd {
                    let _ = seg.subtract(prev);
                }
                self.prev_pdd = Some(cur.clone());
                if seg.is_empty() {
                    (None, None)
                } else {
                    let q = |p: f64| Some(seg.value_at_quantile(p) as f64 / 1000.0);
                    (q(0.5), q(0.95))
                }
            }
            Err(_) => (None, None),
        };

        let cur_fail = detail
            .fail_codes
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default();
        let fail_codes = cur_fail
            .iter()
            .filter_map(|(&code, &n)| {
                let delta = n.saturating_sub(*self.prev_fail.get(&code).unwrap_or(&0));
                (delta > 0).then_some((code, delta))
            })
            .collect();

        self.buckets.push(TimelineBucket {
            start_secs: self.bucket_start,
            end_secs: end,
            initiated: d(now.initiated, self.prev.initiated),
            answered,
            completed: d(now.completed, self.prev.completed),
            failed,
            timeout,
            asr: (decided > 0).then(|| answered as f64 / decided as f64 * 100.0),
            pdd_p50_ms,
            pdd_p95_ms,
            mos: (self.mos_n > 0).then(|| self.mos_sum / self.mos_n as f64),
            fail_codes,
            active: live.calls_active.load(Ordering::Relaxed).max(0) as u64,
            rtp: RtpBucketStats::from_intervals(&std::mem::take(&mut self.rtp)),
        });
        self.prev = now;
        self.prev_fail = cur_fail;
        self.mos_sum = 0.0;
        self.mos_n = 0;
        self.bucket_start = end;
    }
}

/// 兩端引擎共用：依 `--interval` 的時間軸，加上固定 10 秒、含 RTP 品質的時間軸
pub struct Timelines {
    coarse: TimelineRecorder,
    fine: TimelineRecorder,
    started_at: String,
}

impl Timelines {
    /// `started` = 測試開始的時刻
    pub fn new(interval_secs: u64, started: std::time::Instant) -> Self {
        let wall = std::time::SystemTime::now() - started.elapsed();
        let secs = wall
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            coarse: TimelineRecorder::new(interval_secs),
            fine: TimelineRecorder::new(FINE_INTERVAL_SECS),
            started_at: crate::sip_logger::SipLogger::rfc3339_utc(secs),
        }
    }

    /// 下一次 `tick(elapsed)` 是否會結算 10 秒時段：是的話先收集進行中通話的 RTP 增量
    pub fn fine_due(&self, elapsed: f64) -> bool {
        self.fine.due(elapsed)
    }

    /// 通話結束時呼叫：最後一段 RTP 增量
    pub fn add_rtp(&mut self, sample: RtpInterval) {
        self.fine.add_rtp([sample]);
    }

    /// 每秒呼叫：`mos` 為此刻的即時 MOS，`rtp` 為進行中通話的增量（`fine_due` 時才需要）
    pub fn tick(
        &mut self,
        elapsed: f64,
        live: &LiveStats,
        detail: &DetailedStats,
        mos: Option<f64>,
        rtp: Vec<RtpInterval>,
    ) {
        self.fine.add_rtp(rtp);
        self.fine.tick(elapsed, live, detail, None);
        self.coarse.tick(elapsed, live, detail, mos);
    }

    /// 最近一個 10 秒時段的 RTP 品質
    pub fn latest_rtp(&self) -> Option<&RtpBucketStats> {
        self.fine.last().and_then(|b| b.rtp.as_ref())
    }

    /// 測試結束：`rtp` 為收尾時仍在進行的通話的最後增量，結果寫入報告
    pub fn finish(
        mut self,
        elapsed: f64,
        live: &LiveStats,
        detail: &DetailedStats,
        rtp: Vec<RtpInterval>,
        report: &mut crate::stats::FinalReport,
    ) {
        self.fine.add_rtp(rtp);
        report.timeline = self.coarse.finish(elapsed, live, detail);
        report.fine_timeline = self.fine.finish(elapsed, live, detail);
        report.started_at = self.started_at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::FinalReport;

    fn calls(live: &LiveStats, n: u64, f: impl Fn(&LiveStats)) {
        for _ in 0..n {
            live.on_invite();
            f(live);
        }
    }

    #[test]
    fn buckets_hold_per_interval_deltas() {
        let live = LiveStats::default();
        let detail = DetailedStats::default();
        let mut tl = TimelineRecorder::new(60);

        // 第 1 段：10 通全部接通，PDD 100 ms，MOS 4.4
        calls(&live, 10, |l| l.on_answered());
        for _ in 0..10 {
            detail.record_pdd(100.0);
        }
        tl.tick(30.0, &live, &detail, Some(4.4));
        tl.tick(60.0, &live, &detail, Some(4.2)); // 跨過 60 秒 → 結算第 1 段
                                                  // 第 2 段：10 通中 6 通接通、2 通 503、2 通逾時，PDD 變慢，MOS 變差
        calls(&live, 6, |l| l.on_answered());
        calls(&live, 2, |l| l.on_failed());
        detail.record_fail_code(503);
        detail.record_fail_code(503);
        calls(&live, 2, |l| l.on_timeout());
        for _ in 0..6 {
            detail.record_pdd(900.0);
        }
        tl.tick(90.0, &live, &detail, Some(3.0));
        tl.tick(120.0, &live, &detail, Some(2.0));
        // 第 3 段只有 5 秒：沒有任何通話
        let buckets = tl.finish(125.0, &live, &detail);

        assert_eq!(buckets.len(), 3);
        let (a, b, c) = (&buckets[0], &buckets[1], &buckets[2]);
        assert_eq!((a.start_secs, a.end_secs), (0.0, 60.0));
        assert_eq!((a.initiated, a.answered), (10, 10));
        assert_eq!(a.asr, Some(100.0));
        assert!((a.pdd_p50_ms.unwrap() - 100.0).abs() < 1.0);
        assert!((a.mos.unwrap() - 4.3).abs() < 1e-9);
        assert!(a.fail_codes.is_empty());

        assert_eq!((b.start_secs, b.end_secs), (60.0, 120.0));
        assert_eq!(
            (b.initiated, b.answered, b.failed, b.timeout),
            (10, 6, 2, 2)
        );
        assert_eq!(b.asr, Some(60.0));
        assert!(
            (b.pdd_p50_ms.unwrap() - 900.0).abs() < 2.0,
            "只看本段的 PDD 樣本"
        );
        assert!((b.mos.unwrap() - 2.5).abs() < 1e-9);
        assert_eq!(b.fail_codes, BTreeMap::from([(503, 2)]));
        assert!((b.cps() - 10.0 / 60.0).abs() < 1e-9);

        assert_eq!((c.start_secs, c.end_secs), (120.0, 125.0));
        assert_eq!(c.initiated, 0);
        assert_eq!(c.asr, None);
        assert_eq!(c.pdd_p50_ms, None);
    }

    #[test]
    fn skipped_ticks_still_produce_contiguous_buckets() {
        let live = LiveStats::default();
        let detail = DetailedStats::default();
        let mut tl = TimelineRecorder::new(10);
        calls(&live, 3, |l| l.on_answered());
        tl.tick(35.0, &live, &detail, None);
        let b = tl.finish(35.0, &live, &detail);
        let spans: Vec<_> = b.iter().map(|x| (x.start_secs, x.end_secs)).collect();
        assert_eq!(
            spans,
            vec![(0.0, 10.0), (10.0, 20.0), (20.0, 30.0), (30.0, 35.0)]
        );
        assert_eq!(b.iter().map(|x| x.initiated).sum::<u64>(), 3);
    }

    #[test]
    fn calls_ending_during_shutdown_are_kept() {
        let live = LiveStats::default();
        let detail = DetailedStats::default();
        let mut tl = TimelineRecorder::new(10);
        calls(&live, 4, |l| l.on_answered());
        tl.tick(10.0, &live, &detail, None);
        // 時間到後 0.3 秒內收尾：4 通都掛斷
        for _ in 0..4 {
            live.on_completed();
        }
        live.on_invite();
        live.on_timeout();
        detail.record_fail_code(408);
        let b = tl.finish(10.3, &live, &detail);
        assert_eq!(b.len(), 1, "不到 1 秒的尾段併入上一段");
        assert_eq!(
            (b[0].end_secs, b[0].initiated, b[0].completed, b[0].timeout),
            (10.3, 5, 4, 1)
        );
        assert_eq!(b[0].asr, Some(80.0));
        assert_eq!(b[0].fail_codes, BTreeMap::from([(408, 1)]));
        // 尾段超過 1 秒則另列
        let live = LiveStats::default();
        let mut tl = TimelineRecorder::new(10);
        tl.tick(10.0, &live, &detail, None);
        live.on_invite();
        assert_eq!(tl.finish(12.0, &live, &detail).len(), 2);
        // 沒有任何變化的極短尾段不列出
        let mut tl = TimelineRecorder::new(10);
        tl.tick(10.0, &live, &detail, None);
        assert_eq!(tl.finish(10.3, &live, &detail).len(), 1);
    }

    #[test]
    fn disabled_and_csv() {
        let live = LiveStats::default();
        let detail = DetailedStats::default();
        let mut tl = TimelineRecorder::new(0);
        tl.tick(100.0, &live, &detail, Some(4.0));
        assert!(tl.finish(100.0, &live, &detail).is_empty());

        let b = TimelineBucket {
            start_secs: 0.0,
            end_secs: 60.0,
            initiated: 120,
            answered: 90,
            asr: Some(90.0),
            fail_codes: BTreeMap::from([(486, 3), (503, 7)]),
            ..Default::default()
        };
        assert_eq!(
            TimelineBucket::CSV_HEADER.split(',').count(),
            b.csv_row().split(',').count()
        );
        assert_eq!(b.csv_row(), "0,60,120,90,0,0,0,2.00,90.00,,,,486:3 503:7");
    }

    fn interval(expected: u64, lost: u64, late_large: u64, jitter_ms: f64) -> RtpInterval {
        RtpInterval {
            expected,
            lost,
            late_small: late_large,
            late_large,
            jitter_ms,
        }
    }

    #[test]
    fn rtp_bucket_stats_per_call() {
        assert_eq!(RtpBucketStats::from_intervals(&[]), None);
        // 沒有收發封包的通話不列入
        assert_eq!(
            RtpBucketStats::from_intervals(&[interval(0, 0, 0, 0.0)]),
            None
        );

        // 19 通完全正常 + 1 通本段掉包 20%
        let mut v = vec![interval(500, 0, 0, 5.0); 19];
        v.push(interval(500, 100, 0, 25.0));
        let r = RtpBucketStats::from_intervals(&v).unwrap();
        assert_eq!(r.calls, 20);
        assert!((r.loss_pct - 1.0).abs() < 1e-9, "100 / 10000");
        assert!((r.loss_max_pct - 20.0).abs() < 1e-9);
        assert!((r.jitter_avg_ms - 6.0).abs() < 1e-9);
        assert_eq!(r.jitter_p95_ms, 5.0);
        // 最差那通拉低最低值，P5（20 通的第 1 名）就是它
        assert!(r.mos_min < 2.5 && r.mos_p5 == r.mos_min);
        assert!(r.mos_avg > 4.0);
        assert_eq!(r.below_threshold, 1);

        // jitter buffer：晚到的封包只影響含 jitter buffer 的 MOS
        let r = RtpBucketStats::from_intervals(&[interval(500, 0, 25, 40.0)]).unwrap();
        assert!(r.mos_avg > 4.4 && r.mos_jb200_avg < 3.5, "{:?}", r);
    }

    #[test]
    fn timelines_attach_rtp_to_fine_buckets() {
        let live = LiveStats::default();
        let detail = DetailedStats::default();
        let mut t = Timelines::new(60, std::time::Instant::now());
        assert!(!t.fine_due(9.0) && t.fine_due(10.0));
        t.tick(5.0, &live, &detail, Some(4.4), Vec::new());
        // 中途結束的通話：最後一段在結束時送入
        t.add_rtp(interval(250, 0, 0, 5.0));
        t.tick(
            10.0,
            &live,
            &detail,
            Some(4.4),
            vec![interval(500, 50, 0, 5.0)],
        );
        let first = t.latest_rtp().unwrap().clone();
        assert_eq!(first.calls, 2);
        assert!((first.loss_pct - 50.0 / 750.0 * 100.0).abs() < 1e-9);
        // 第二段沒有 RTP
        t.tick(20.0, &live, &detail, None, Vec::new());
        assert_eq!(t.latest_rtp(), None);

        let mut report = FinalReport::build(&live.snapshot(), &detail, 25.0, None);
        t.finish(
            25.0,
            &live,
            &detail,
            vec![interval(100, 0, 0, 1.0)],
            &mut report,
        );
        let spans: Vec<_> = report
            .fine_timeline
            .iter()
            .map(|b| (b.start_secs, b.end_secs, b.rtp.as_ref().map(|r| r.calls)))
            .collect();
        assert_eq!(
            spans,
            vec![
                (0.0, 10.0, Some(2)),
                (10.0, 20.0, None),
                (20.0, 25.0, Some(1))
            ]
        );
        assert_eq!(report.timeline.len(), 1, "依 --interval 的時間軸照舊");
        assert!(report.timeline[0].rtp.is_none());
        assert!(report.started_at.ends_with('Z') && report.started_at.len() == 20);
    }
}
