//! 每通通話的追蹤紀錄（端到端對照用）
//!
//! 民眾端與座席端各自記錄每通電話的 SIPX 全域 ID（`P-CIN-CallId`）、主叫號碼與時間點，
//! 由 [`crate::correlate`] 合併配對。時間使用系統時鐘（Unix epoch 毫秒），
//! 兩端在不同程序（甚至不同機器）執行時也能比較；不同機器需先校時（NTP）。
use crate::config::Mode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// 單次壓測最多記錄的通話數（超過的不記錄，避免長時間高 CPS 壓測耗盡記憶體）
pub const MAX_TRACES: usize = 500_000;

/// 目前系統時間（Unix epoch 毫秒）
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// 通話結果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    /// 尚未有最終結果（壓測結束時仍在響鈴 / 等待中）
    #[default]
    Pending,
    /// 已接通（200 OK）
    Answered,
    /// 收到 / 送出 3xx–6xx
    Failed,
    /// 民眾端 INVITE 逾時（送 CANCEL）
    Timeout,
}

/// 一通電話在單一端的追蹤紀錄
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CallTrace {
    /// 這一段的 SIP Call-ID（SIPX 每段都不同，只用於除錯）
    pub sip_call_id: String,
    /// SIPX 全域通話 ID（`P-CIN-CallId`）；民眾端取自 18x / 200，座席端取自 INVITE
    pub cin_call_id: Option<String>,
    /// 主叫號碼（民眾端：送出的 From；座席端：收到的 From）
    pub caller_number: String,
    /// 座席分機（只有座席端有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// 民眾端送出 / 座席端收到 INVITE 的時間（epoch ms）
    pub invite_at_ms: u64,
    /// 接通時間（民眾端收到 / 座席端送出 200 OK）
    pub answered_at_ms: Option<u64>,
    /// 通話結束時間
    pub ended_at_ms: Option<u64>,
    pub outcome: CallOutcome,
    /// 最終回應碼（200 或失敗碼）
    pub response_code: Option<u16>,
    /// 這一端量到的接收方向音質（只在啟用 RTP 且有通話時有值）
    pub mos: Option<f64>,
    pub loss_pct: Option<f64>,
    pub jitter_ms: Option<f64>,
}

impl CallTrace {
    pub fn set_rtp(&mut self, snap: &crate::rtp::stats::RtpStatsSnapshot) {
        self.mos = Some(snap.mos);
        self.loss_pct = Some(snap.loss_rate_pct);
        self.jitter_ms = Some(snap.jitter_ms);
    }
}

/// 一端的完整追蹤紀錄（CLI `--trace-file` 的檔案格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFile {
    pub side: Mode,
    /// SIP server（報告顯示用）
    #[serde(default)]
    pub server: String,
    /// 超過 [`MAX_TRACES`] 而未記錄的通話數
    #[serde(default)]
    pub dropped: u64,
    pub traces: Vec<CallTrace>,
}

#[derive(Default)]
struct Inner {
    order: Vec<String>,
    map: HashMap<String, CallTrace>,
    dropped: u64,
}

/// 引擎執行中收集追蹤紀錄（以每段的唯一鍵索引，保留建立順序）
#[derive(Default)]
pub struct TraceLog {
    inner: Mutex<Inner>,
}

impl TraceLog {
    /// 新增一通；同一個鍵已存在時忽略
    pub fn start(&self, key: &str, trace: CallTrace) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.map.contains_key(key) {
            return;
        }
        if g.map.len() >= MAX_TRACES {
            g.dropped += 1;
            return;
        }
        g.order.push(key.to_string());
        g.map.insert(key.to_string(), trace);
    }

    /// 更新既有紀錄（不存在時不做事）
    pub fn update(&self, key: &str, f: impl FnOnce(&mut CallTrace)) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(t) = g.map.get_mut(key) {
                f(t);
            }
        }
    }

    /// 依建立順序取出所有紀錄
    pub fn snapshot(&self) -> Vec<CallTrace> {
        let Ok(g) = self.inner.lock() else {
            return Vec::new();
        };
        g.order
            .iter()
            .filter_map(|k| g.map.get(k).cloned())
            .collect()
    }

    pub fn dropped(&self) -> u64 {
        self.inner.lock().map(|g| g.dropped).unwrap_or_default()
    }

    pub fn to_file(&self, side: Mode, server: &str) -> TraceFile {
        TraceFile {
            side,
            server: server.to_string(),
            dropped: self.dropped(),
            traces: self.snapshot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_keeps_order_and_ignores_duplicates() {
        let log = TraceLog::default();
        log.start(
            "b",
            CallTrace {
                sip_call_id: "b".into(),
                ..Default::default()
            },
        );
        log.start(
            "a",
            CallTrace {
                sip_call_id: "a".into(),
                ..Default::default()
            },
        );
        log.start(
            "b",
            CallTrace {
                sip_call_id: "dup".into(),
                ..Default::default()
            },
        );
        log.update("a", |t| t.outcome = CallOutcome::Answered);
        log.update("missing", |t| t.outcome = CallOutcome::Failed);
        let all = log.snapshot();
        assert_eq!(
            all.iter()
                .map(|t| t.sip_call_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(all[1].outcome, CallOutcome::Answered);
        assert_eq!(log.dropped(), 0);
    }

    #[test]
    fn trace_file_round_trips() {
        let log = TraceLog::default();
        log.start(
            "x",
            CallTrace {
                sip_call_id: "x".into(),
                cin_call_id: Some("C1".into()),
                caller_number: "0912".into(),
                invite_at_ms: 1000,
                ..Default::default()
            },
        );
        let file = log.to_file(Mode::Caller, "10.0.0.1:5060");
        let json = serde_json::to_string(&file).unwrap();
        assert!(!json.contains("\"agent\""), "民眾端不輸出 agent 欄位");
        let back: TraceFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.side, Mode::Caller);
        assert_eq!(back.traces, file.traces);
    }
}
