//! 壓測原始數據檔（`<時間>_result.json`）
//!
//! 壓測結束時和 HTML 報告一起寫出，內容為完整的 `FinalReport`（含 10 秒時間軸與測試開始時間）
//! 與壓測設定；之後的整合報告讀這份檔案，不必重跑壓測。設定中的密碼與帳號清單不寫入。

use crate::config::Config;
use crate::stats::FinalReport;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 格式版本：只新增欄位時不變；刪除、改名或改變單位時遞增
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultFile {
    pub format_version: u32,
    /// 產生這份檔案的 sipress 版本
    pub sipress_version: String,
    /// 壓測設定（已移除密碼；座席帳號清單只留數量）
    pub config: Value,
    pub report: FinalReport,
}

impl ResultFile {
    pub fn new(cfg: &Config, report: &FinalReport) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            sipress_version: env!("CARGO_PKG_VERSION").to_string(),
            config: redacted_config(cfg),
            report: report.clone(),
        }
    }

    /// 寫到 `<dir>/<timestamp>_result.json`
    pub fn save(&self, output_dir: &str, timestamp: &str) -> Result<PathBuf> {
        std::fs::create_dir_all(output_dir)?;
        let path = PathBuf::from(output_dir).join(format!("{}_result.json", timestamp));
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("無法讀取 {}", path.display()))?;
        let file: Self = serde_json::from_str(&text)
            .with_context(|| format!("{} 不是 sipress 壓測數據檔", path.display()))?;
        if file.format_version > FORMAT_VERSION {
            anyhow::bail!(
                "{} 的格式版本 {} 比這個 sipress 支援的 {} 新，請更新 sipress",
                path.display(),
                file.format_version,
                FORMAT_VERSION
            );
        }
        Ok(file)
    }
}

fn redacted_config(cfg: &Config) -> Value {
    let mut v = serde_json::to_value(cfg).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        if let Some(accounts) = obj.remove("agent_accounts") {
            let n = accounts.as_array().map_or(0, |a| a.len());
            obj.insert("agent_account_count".into(), n.into());
        }
    }
    strip_passwords(&mut v);
    v
}

fn strip_passwords(v: &mut Value) {
    match v {
        Value::Object(obj) => {
            obj.retain(|k, _| !k.contains("password"));
            obj.values_mut().for_each(strip_passwords);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_passwords),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AgentAccount;
    use crate::stats::{DetailedStats, LiveStats};

    #[test]
    fn round_trip_without_secrets() {
        let mut cfg = Config {
            caller_auth_password: Some("caller-secret".into()),
            ..Config::default()
        };
        cfg.agent_accounts = vec![AgentAccount {
            extension: "1001".into(),
            username: "1001".into(),
            password: "agent-secret".into(),
            domain: String::new(),
        }];
        cfg.agent_flow.agent_password = "flow-secret".into();
        let mut report = FinalReport::build(
            &LiveStats::default().snapshot(),
            &DetailedStats::default(),
            1.0,
            None,
        );
        report.started_at = "2026-10-28T05:00:00Z".into();

        let dir = std::env::temp_dir().join(format!("sipress-result-test-{}", std::process::id()));
        let path = ResultFile::new(&cfg, &report)
            .save(&dir.to_string_lossy(), "20261028_050000")
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret"), "密碼不應寫入：{}", text);
        assert!(!text.contains("\"1001\""), "帳號清單不應寫入");

        let back = ResultFile::load(&path).unwrap();
        assert_eq!(back.format_version, FORMAT_VERSION);
        assert_eq!(back.report.started_at, "2026-10-28T05:00:00Z");
        assert_eq!(back.config["agent_account_count"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
