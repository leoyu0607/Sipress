/// 壓測設定結構
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 壓測模式：民眾端（主動撥出）vs 座席端（接聽）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 民眾端：壓測程式主動發 INVITE
    Caller,
    /// 座席端：模擬 SIP 話機，REGISTER 後等待來電並自動接聽
    Agent,
}

impl Default for Mode {
    fn default() -> Self {
        Self::Caller
    }
}

/// 座席帳號
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentAccount {
    pub extension: String,
    pub username:  String,
    pub password:  String,
    pub domain:    String,
}

/// 傳輸層協定
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Udp,
    Tcp,
}

impl Default for Transport {
    fn default() -> Self {
        Self::Udp
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Udp => write!(f, "UDP"),
            Self::Tcp => write!(f, "TCP"),
        }
    }
}

/// 完整壓測設定
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 目標 SIP 伺服器（ip:port）
    pub server_addr: String,

    /// 本機綁定地址（ip:port），None 表示自動
    pub local_addr: Option<String>,

    /// 本機 SIP domain（用於 From header），None 則取 local_addr 的 IP
    pub local_domain: Option<String>,

    /// 主叫號碼
    pub caller_number: String,

    /// 被叫號碼前綴（若 callee_fixed 為 Some 則忽略）
    pub callee_prefix: String,

    /// 被叫號碼尾數範圍（0..=callee_range 隨機；若 callee_fixed 為 Some 則忽略）
    pub callee_range: u64,

    /// 固定被叫號碼（Some = 所有通話打給同一個號碼，覆蓋 prefix/range 隨機產生）
    pub callee_fixed: Option<String>,

    /// 每秒通話數（CPS）
    pub cps: f64,

    /// 最大並發通話數
    pub max_concurrent_calls: usize,

    /// 壓測持續秒數
    pub duration_secs: u64,

    /// 單通通話持續秒數（0 = 不主動 BYE）
    pub call_duration_secs: u64,

    /// INVITE 逾時秒數（未收到 180/200 視為失敗）
    pub invite_timeout_secs: u64,

    /// 傳輸層
    pub transport: Transport,

    /// SIP log 輸出目錄
    pub logs_dir: String,

    /// RTP 起始 port（偶數，預設 10000）
    pub rtp_base_port: u16,

    /// 音檔路徑（None = 靜音）
    pub audio_file: Option<std::path::PathBuf>,

    /// 是否啟用 RTP 傳送（false = 只做 SIP signaling）
    pub enable_rtp: bool,

    /// 總通話上限（None = 不限，依 duration_secs 決定結束）
    pub max_total_calls: Option<u64>,

    // ── Agent 模式 ─────────────────────────────────────────────────
    /// 壓測模式（預設 Caller）
    #[serde(default)]
    pub mode: Mode,

    /// 座席帳號列表（Agent 模式必填，Caller 模式忽略）
    #[serde(default)]
    pub agent_accounts: Vec<AgentAccount>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_addr:          "127.0.0.1:5060".to_string(),
            local_addr:           None,
            local_domain:         None,
            caller_number:        "1000".to_string(),
            callee_prefix:        "2".to_string(),
            callee_range:         9999,
            callee_fixed:         None,
            cps:                  10.0,
            max_concurrent_calls: 100,
            duration_secs:        60,
            call_duration_secs:   30,
            invite_timeout_secs:  8,
            transport:            Transport::Udp,
            logs_dir:             "logs".to_string(),
            rtp_base_port:        16000,
            audio_file:           None,
            enable_rtp:           false,
            max_total_calls:      None,
            mode:                 Mode::Caller,
            agent_accounts:       Vec::new(),
        }
    }
}

impl Config {
    /// 壓測總時長
    pub fn duration(&self) -> Duration {
        Duration::from_secs(self.duration_secs)
    }

    /// INVITE 逾時
    pub fn invite_timeout(&self) -> Duration {
        Duration::from_secs(self.invite_timeout_secs)
    }

    /// 傳輸層名稱字串（給 SIP message 用）
    pub fn transport_str(&self) -> &str {
        match self.transport {
            Transport::Udp => "UDP",
            Transport::Tcp => "TCP",
        }
    }

    /// 檢查設定值是否可執行（避免 CPS=0 等導致引擎 panic）
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.mode == Mode::Caller {
            if !(self.cps.is_finite() && self.cps > 0.0) {
                anyhow::bail!("CPS 必須大於 0（目前為 {}）", self.cps);
            }
            if self.max_concurrent_calls == 0 {
                anyhow::bail!("最大並發通話數必須大於 0");
            }
        }
        if self.invite_timeout_secs == 0 {
            anyhow::bail!("INVITE 逾時秒數必須大於 0");
        }
        Ok(())
    }

    /// 從 CLI 參數快速建構
    pub fn from_args(
        server_addr:          impl Into<String>,
        cps:                  f64,
        max_concurrent_calls: usize,
        duration_secs:        u64,
        call_duration_secs:   u64,
    ) -> Self {
        Self {
            server_addr: server_addr.into(),
            cps,
            max_concurrent_calls,
            duration_secs,
            call_duration_secs,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn zero_or_invalid_cps_is_rejected() {
        for cps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let cfg = Config { cps, ..Default::default() };
            assert!(cfg.validate().is_err(), "cps={} 應被拒絕", cps);
        }
    }

    #[test]
    fn agent_mode_ignores_cps() {
        let cfg = Config { mode: Mode::Agent, cps: 0.0, ..Default::default() };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn zero_concurrency_and_timeout_are_rejected() {
        assert!(Config { max_concurrent_calls: 0, ..Default::default() }.validate().is_err());
        assert!(Config { invite_timeout_secs: 0, ..Default::default() }.validate().is_err());
    }
}
