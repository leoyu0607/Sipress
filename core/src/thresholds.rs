//! 整合報告的判定門檻（可由 JSON 檔覆寫，未寫的欄位用預設值）

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// CPU 使用率上限（%）
    pub cpu_pct: f64,
    /// 記憶體使用率上限（%）
    pub mem_pct: f64,
    /// 磁碟 I/O 佇列長度上限
    pub disk_queue: f64,
    /// Jitter 上限（ms，看每通電話的 P95）
    pub jitter_ms: f64,
    /// 掉包率上限（%，整體）
    pub loss_pct: f64,
    /// MOS 下限（jitter buffer 200 ms 模擬，看每通電話的 P5）
    pub mos: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            cpu_pct: 70.0,
            mem_pct: 80.0,
            disk_queue: 2.0,
            jitter_ms: 30.0,
            loss_pct: 0.5,
            mos: 4.0,
        }
    }
}

impl Thresholds {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("無法讀取門檻設定 {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("門檻設定格式錯誤：{}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let t: Thresholds = serde_json::from_str(r#"{"cpu_pct": 60, "mos": 3.8}"#).unwrap();
        assert_eq!(t.cpu_pct, 60.0);
        assert_eq!(t.mos, 3.8);
        assert_eq!(t.loss_pct, Thresholds::default().loss_pct);
    }
}
