/// RTP 品質統計
/// 實作 RFC 3550 §A.8 jitter 計算 + E-Model MOS 估算

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

// ── 即時統計（原子，無鎖）────────────────────────────────────────

#[derive(Default)]
pub struct RtpStats {
    /// 已傳送 RTP 封包數
    pub sent_packets:  AtomicU64,
    /// 已傳送 bytes
    pub sent_bytes:    AtomicU64,
    /// 已接收 RTP 封包數
    pub recv_packets:  AtomicU64,
    /// 累計 jitter（µs × 16，RFC 3550 格式）
    jitter_x16_us:    AtomicU64,
    /// 上一個封包的 RTP 時間戳記
    last_rtp_ts:      Mutex<Option<u32>>,
    /// 上一個封包的到達時間（µs）
    last_recv_us:     Mutex<Option<u64>>,
    /// 上一個封包的序號（用於 jitter / 亂序偵測）
    last_seq:         Mutex<Option<u16>>,
    /// 接收到的第一個序號（用於計算 expected，RFC 3550 §A.3）
    first_seq:        Mutex<Option<u16>>,
    /// 接收到的最高序號
    max_seq:          Mutex<Option<u16>>,
    /// 序號 wrap-around 次數（每次 u16 回繞 +1）
    seq_cycles:       AtomicU64,
    /// 累計亂序封包數
    pub out_of_order: AtomicU64,
    /// 累計重複封包數
    pub duplicates:   AtomicU64,
    /// 目前追蹤的 SSRC（RE-INVITE 後對端可能換 SSRC）
    current_ssrc:     Mutex<Option<u32>>,
    /// SSRC 切換前累積的 (expected, received) — 避免跨 SSRC 序號空間造成假掉包
    prior_expected:   AtomicU64,
    prior_received:   AtomicU64,
}

impl RtpStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// 記錄傳送成功事件（應在 socket.send() 成功後呼叫）
    pub fn on_send(&self, payload_size: usize) {
        self.sent_packets.fetch_add(1, Ordering::Relaxed);
        self.sent_bytes.fetch_add(payload_size as u64, Ordering::Relaxed);
    }

    /// 記錄接收事件（ssrc = 同步源，seq = 序號，rtp_ts = 封包內時間戳記，recv_us = 系統時間 µs）
    pub fn on_recv(&self, ssrc: u32, seq: u16, rtp_ts: u32, recv_us: u64) {
        self.recv_packets.fetch_add(1, Ordering::Relaxed);

        // ── SSRC 切換偵測：RE-INVITE 後對端可能換 SSRC ──
        // 將舊 SSRC 的序號空間統計累積到 prior_expected/prior_received，
        // 重置 seq 追蹤，避免跨 SSRC 序號空間造成假掉包
        {
            let mut cur_ssrc  = self.current_ssrc.lock().unwrap();
            match *cur_ssrc {
                None => { *cur_ssrc = Some(ssrc); }
                Some(prev) if prev != ssrc => {
                    let first = self.first_seq.lock().unwrap().take();
                    let max   = self.max_seq.lock().unwrap().take();
                    if let (Some(f), Some(m)) = (first, max) {
                        let cycles = self.seq_cycles.load(Ordering::Relaxed);
                        let expected = cycles * 65536 + m.wrapping_sub(f) as u64 + 1;
                        let recv_so_far = self.recv_packets.load(Ordering::Relaxed)
                            - 1  // 不含本次
                            - self.prior_received.load(Ordering::Relaxed);
                        self.prior_expected.fetch_add(expected, Ordering::Relaxed);
                        self.prior_received.fetch_add(recv_so_far, Ordering::Relaxed);
                    }
                    self.seq_cycles.store(0, Ordering::Relaxed);
                    self.jitter_x16_us.store(0, Ordering::Relaxed);
                    *self.last_rtp_ts.lock().unwrap() = None;
                    *self.last_recv_us.lock().unwrap() = None;
                    *self.last_seq.lock().unwrap() = None;
                    *cur_ssrc = Some(ssrc);
                }
                _ => {}
            }
        }

        // ── Jitter 計算（RFC 3550 §A.8）──
        let mut last_ts  = self.last_rtp_ts.lock().unwrap();
        let mut last_us  = self.last_recv_us.lock().unwrap();
        let mut last_seq = self.last_seq.lock().unwrap();

        if let (Some(prev_ts), Some(prev_us), Some(prev_seq)) =
            (*last_ts, *last_us, *last_seq)
        {
            let seq_diff = seq.wrapping_sub(prev_seq);
            if seq_diff == 0 {
                self.duplicates.fetch_add(1, Ordering::Relaxed);
            } else if seq_diff > 0x7FFF {
                self.out_of_order.fetch_add(1, Ordering::Relaxed);
            } else {
                let recv_diff_8k = ((recv_us.wrapping_sub(prev_us)) as f64
                    / 1_000_000.0 * 8000.0) as i64;
                let rtp_diff = rtp_ts.wrapping_sub(prev_ts) as i64;
                let d = (recv_diff_8k - rtp_diff).unsigned_abs() as u64;
                let j = self.jitter_x16_us.load(Ordering::Relaxed);
                let new_j = j.saturating_add(d).saturating_sub(j / 16);
                self.jitter_x16_us.store(new_j, Ordering::Relaxed);
            }
        }

        *last_ts  = Some(rtp_ts);
        *last_us  = Some(recv_us);
        *last_seq = Some(seq);
        drop(last_ts);
        drop(last_us);
        drop(last_seq);

        // ── 序號追蹤（RFC 3550 §A.3 掉包率基礎）──
        let mut first_seq = self.first_seq.lock().unwrap();
        let mut max_seq   = self.max_seq.lock().unwrap();

        if first_seq.is_none() {
            *first_seq = Some(seq);
            *max_seq   = Some(seq);
        } else if let Some(prev_max) = *max_seq {
            let diff = seq.wrapping_sub(prev_max);
            if diff > 0 && diff <= 0x7FFF {
                if seq < prev_max && diff < 0x8000 {
                    self.seq_cycles.fetch_add(1, Ordering::Relaxed);
                }
                *max_seq = Some(seq);
            }
        }
    }

    /// Jitter（ms）
    pub fn jitter_ms(&self) -> f64 {
        let j_8k = self.jitter_x16_us.load(Ordering::Relaxed) as f64 / 16.0;
        j_8k / 8.0  // 8kHz 單位 → ms
    }

    /// 掉包率（0.0 ~ 1.0）
    /// 依接收端序號空間計算：(expected - received) / expected（RFC 3550 §A.3）
    /// 包含 SSRC 切換前累積的統計，避免跨 SSRC 序號空間造成假掉包
    pub fn packet_loss_rate(&self) -> f64 {
        self.loss_details().1
    }

    /// 快照
    pub fn snapshot(&self) -> RtpStatsSnapshot {
        let sent = self.sent_packets.load(Ordering::Relaxed);
        let recv = self.recv_packets.load(Ordering::Relaxed);
        let (lost, loss_rate) = self.loss_details();
        let jitter_ms = self.jitter_ms();
        let mos       = estimate_mos(loss_rate, jitter_ms);

        RtpStatsSnapshot {
            sent_packets:  sent,
            recv_packets:  recv,
            lost_packets:  lost,
            loss_rate_pct: loss_rate * 100.0,
            jitter_ms,
            mos,
            out_of_order:  self.out_of_order.load(Ordering::Relaxed),
            duplicates:    self.duplicates.load(Ordering::Relaxed),
        }
    }

    fn loss_details(&self) -> (u64, f64) {
        let sent    = self.sent_packets.load(Ordering::Relaxed);
        let recv    = self.recv_packets.load(Ordering::Relaxed);
        let first   = *self.first_seq.lock().unwrap();
        let max     = *self.max_seq.lock().unwrap();
        let cycles  = self.seq_cycles.load(Ordering::Relaxed);
        let prior_e = self.prior_expected.load(Ordering::Relaxed);
        let prior_r = self.prior_received.load(Ordering::Relaxed);

        let cur_expected = match (first, max) {
            (Some(f), Some(m)) => cycles * 65536 + m.wrapping_sub(f) as u64 + 1,
            _ => 0,
        };
        let cur_recv = recv.saturating_sub(prior_r);
        let total_expected = prior_e + cur_expected;
        let total_recv     = prior_r + cur_recv;

        if total_expected > 0 {
            let lost = total_expected.saturating_sub(total_recv);
            (lost, lost as f64 / total_expected as f64)
        } else if sent > 0 {
            (sent, 1.0)
        } else {
            (0, 0.0)
        }
    }
}

// ── 快照（可序列化）────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RtpStatsSnapshot {
    pub sent_packets:  u64,
    pub recv_packets:  u64,
    pub lost_packets:  u64,
    /// 掉包率（%）
    pub loss_rate_pct: f64,
    /// Jitter（ms）
    pub jitter_ms:     f64,
    /// MOS 估算值（1.0 ~ 5.0）
    pub mos:           f64,
    pub out_of_order:  u64,
    pub duplicates:    u64,
}

impl RtpStatsSnapshot {
    pub fn mos_label(&self) -> &'static str {
        mos_label(self.mos)
    }
}

/// 聚合多通電話的 RTP 統計
///
/// - 掉包率：依封包數加權（Σ掉包 / Σ(收到 + 掉包)），與 `lost_packets` 加總一致
/// - Jitter：依收到的封包數加權，未收到封包的通話不會拉低平均
/// - MOS：每通電話平均（每通通話的體驗各占一票）
pub fn aggregate_snapshots(snapshots: &[RtpStatsSnapshot]) -> Option<RtpStatsSnapshot> {
    if snapshots.is_empty() { return None; }
    let n = snapshots.len() as f64;
    let recv: u64 = snapshots.iter().map(|s| s.recv_packets).sum();
    let lost: u64 = snapshots.iter().map(|s| s.lost_packets).sum();
    let expected  = recv + lost;
    let loss_rate_pct = if expected > 0 { lost as f64 / expected as f64 * 100.0 } else { 0.0 };
    let jitter_ms = if recv > 0 {
        snapshots.iter().map(|s| s.jitter_ms * s.recv_packets as f64).sum::<f64>() / recv as f64
    } else {
        0.0
    };
    Some(RtpStatsSnapshot {
        sent_packets:  snapshots.iter().map(|s| s.sent_packets).sum(),
        recv_packets:  recv,
        lost_packets:  lost,
        loss_rate_pct,
        jitter_ms,
        mos:           snapshots.iter().map(|s| s.mos).sum::<f64>() / n,
        out_of_order:  snapshots.iter().map(|s| s.out_of_order).sum(),
        duplicates:    snapshots.iter().map(|s| s.duplicates).sum(),
    })
}

// ── MOS 估算（ITU-T E-Model 簡化版）────────────────────────────

/// 由掉包率（0.0~1.0）和 jitter（ms）估算 MOS（1.0~5.0）
pub fn estimate_mos(loss_rate: f64, jitter_ms: f64) -> f64 {
    let r0  = 93.2_f64;
    let ppl = (loss_rate * 100.0).min(100.0);
    let ie  = 0.0_f64;
    let bpl = 4.3_f64;
    let ie_eff = ie + (95.0 - ie) * ppl / (ppl + bpl);
    let id = if jitter_ms < 150.0 { 0.0 } else { (jitter_ms - 150.0) * 0.1 };
    let r  = (r0 - ie_eff - id).max(0.0).min(100.0);
    r_to_mos(r)
}

/// R 值 → MOS（ITU-T G.107 §B.4）
fn r_to_mos(r: f64) -> f64 {
    if r <= 0.0 { return 1.0; }
    if r >= 100.0 { return 4.5; }
    let mos = 1.0 + 0.035 * r + r * (r - 60.0) * (100.0 - r) * 7e-6;
    mos.max(1.0).min(5.0)
}

/// MOS 文字等級
pub fn mos_label(mos: f64) -> &'static str {
    match mos as u32 {
        5 => "優秀 (Excellent)",
        4 => "良好 (Good)",
        3 => "普通 (Fair)",
        2 => "差  (Poor)",
        _ => "劣  (Bad)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mos_perfect() {
        let mos = estimate_mos(0.0, 0.0);
        assert!(mos > 4.0, "MOS={}", mos);
    }

    #[test]
    fn test_mos_high_loss() {
        let mos = estimate_mos(0.20, 20.0);
        assert!(mos < 2.5, "MOS={}", mos);
    }

    #[test]
    fn test_mos_range() {
        let mos = estimate_mos(0.05, 80.0);
        assert!(mos >= 1.0 && mos <= 5.0, "MOS={} out of range", mos);
    }

    #[test]
    fn test_loss_rate_seq_based() {
        let stats = RtpStats::new();
        let ssrc = 0x12345678;
        // 模擬收到 seq 100, 101, 103（跳過 102 = 1 個掉包）
        let t = 0u64;
        stats.on_recv(ssrc, 100, 0, t);
        stats.on_recv(ssrc, 101, 160, t + 20_000);
        stats.on_recv(ssrc, 103, 480, t + 60_000);
        // expected = 103 - 100 + 1 = 4, recv = 3, lost = 1
        let loss = stats.packet_loss_rate();
        assert!((loss - 0.25).abs() < 0.01, "loss={}", loss);
    }

    #[test]
    fn test_ssrc_change_no_phantom_loss() {
        let stats = RtpStats::new();
        let ssrc_a = 0xAAAAAAAA;
        let ssrc_b = 0xBBBBBBBB;
        let t = 0u64;
        // SSRC A: seq 100..104 (5 packets, 0 loss)
        for i in 0..5u16 {
            stats.on_recv(ssrc_a, 100 + i, i as u32 * 160, t + i as u64 * 20_000);
        }
        // SSRC B (after RE-INVITE): seq 50000..50004 (5 packets, 0 loss)
        for i in 0..5u16 {
            stats.on_recv(ssrc_b, 50000 + i, i as u32 * 160, t + (5 + i) as u64 * 20_000);
        }
        let snap = stats.snapshot();
        // total expected = 5 + 5 = 10, total recv = 10, loss = 0%
        assert!(snap.loss_rate_pct < 1.0,
            "SSRC change should not cause phantom loss, got {}%", snap.loss_rate_pct);
        assert_eq!(snap.recv_packets, 10);
    }

    #[test]
    fn test_aggregate_snapshots_empty() {
        assert!(aggregate_snapshots(&[]).is_none());
    }

    #[test]
    fn test_aggregate_snapshots_single() {
        let snap = RtpStatsSnapshot {
            sent_packets: 100, recv_packets: 95, lost_packets: 5,
            loss_rate_pct: 5.0, jitter_ms: 10.0, mos: 4.0,
            out_of_order: 2, duplicates: 1,
        };
        let agg = aggregate_snapshots(&[snap.clone()]).unwrap();
        assert_eq!(agg.sent_packets, 100);
        assert_eq!(agg.recv_packets, 95);
        assert!((agg.mos - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_aggregate_snapshots_weights_by_packets() {
        let a = RtpStatsSnapshot {
            sent_packets: 100, recv_packets: 90, lost_packets: 10,
            loss_rate_pct: 10.0, jitter_ms: 20.0, mos: 3.0,
            out_of_order: 2, duplicates: 0,
        };
        let b = RtpStatsSnapshot {
            sent_packets: 200, recv_packets: 198, lost_packets: 2,
            loss_rate_pct: 1.0, jitter_ms: 5.0, mos: 4.5,
            out_of_order: 0, duplicates: 1,
        };
        let agg = aggregate_snapshots(&[a, b]).unwrap();
        assert_eq!(agg.sent_packets, 300);
        assert_eq!(agg.recv_packets, 288);
        // 掉包率依封包數加權：12 / 300（而非兩通電話 10% 與 1% 的平均 5.5%）
        assert!((agg.loss_rate_pct - 4.0).abs() < 0.001, "loss={}", agg.loss_rate_pct);
        assert_eq!(agg.lost_packets, 12);
        // jitter 依收到封包數加權：(20×90 + 5×198) / 288
        assert!((agg.jitter_ms - 9.6875).abs() < 0.001, "jitter={}", agg.jitter_ms);
        // MOS 為每通電話平均
        assert!((agg.mos - 3.75).abs() < 0.001);
        assert_eq!(agg.out_of_order, 2);
        assert_eq!(agg.duplicates, 1);
    }

    #[test]
    fn test_aggregate_ignores_silent_calls_for_jitter() {
        let talking = RtpStatsSnapshot {
            sent_packets: 100, recv_packets: 100, lost_packets: 0,
            loss_rate_pct: 0.0, jitter_ms: 30.0, mos: 4.0,
            out_of_order: 0, duplicates: 0,
        };
        // 送出但完全沒收到（單向）：掉包 100%，沒有 jitter 樣本
        let one_way = RtpStatsSnapshot {
            sent_packets: 100, recv_packets: 0, lost_packets: 100,
            loss_rate_pct: 100.0, jitter_ms: 0.0, mos: 1.0,
            out_of_order: 0, duplicates: 0,
        };
        let agg = aggregate_snapshots(&[talking, one_way]).unwrap();
        assert!((agg.jitter_ms - 30.0).abs() < 0.001, "沒收到封包的通話不應拉低 jitter");
        assert!((agg.loss_rate_pct - 50.0).abs() < 0.001);
    }

    #[test]
    fn test_snapshot_and_loss_rate_consistent() {
        let stats = RtpStats::new();
        let ssrc = 0x11111111;
        for i in 0..10u16 {
            stats.on_send(160);
            stats.on_recv(ssrc, 100 + i, i as u32 * 160, i as u64 * 20_000);
        }
        let snap = stats.snapshot();
        let rate = stats.packet_loss_rate();
        assert!((snap.loss_rate_pct / 100.0 - rate).abs() < 0.001,
            "snapshot and packet_loss_rate should agree: snap={} rate={}", snap.loss_rate_pct, rate);
    }

    #[test]
    fn test_send_only_shows_total_loss() {
        let stats = RtpStats::new();
        for _ in 0..5 {
            stats.on_send(160);
        }
        let rate = stats.packet_loss_rate();
        assert!((rate - 1.0).abs() < 0.001, "send-only should show 100% loss");
    }

    #[test]
    fn test_jitter_increases_with_variation() {
        let stats = RtpStats::new();
        let ssrc = 0x12345678;
        stats.on_recv(ssrc, 100, 0, 0);
        stats.on_recv(ssrc, 101, 160, 20_000);
        let j1 = stats.jitter_ms();
        stats.on_recv(ssrc, 102, 320, 100_000);
        let j2 = stats.jitter_ms();
        assert!(j2 > j1, "jitter should increase with timing variation: j1={} j2={}", j1, j2);
    }
}
