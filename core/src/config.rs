/// 壓測設定結構
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// CPS 負載模式
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CpsPattern {
    #[default]
    Flat,
    Ramp {
        start: f64,
        end: f64,
    },
    Step {
        levels: Vec<CpsStep>,
    },
    Spike {
        base: f64,
        peak: f64,
        spike_at_pct: f64,
        spike_duration_secs: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpsStep {
    pub cps: f64,
    pub duration_secs: u64,
}

impl CpsPattern {
    pub fn current_cps(&self, elapsed: Duration, base_cps: f64, total_duration: Duration) -> f64 {
        match self {
            Self::Flat => base_cps,
            Self::Ramp { start, end } => {
                let total = total_duration.as_secs_f64();
                if total <= 0.0 {
                    return *end;
                }
                let t = (elapsed.as_secs_f64() / total).clamp(0.0, 1.0);
                start + (end - start) * t
            }
            Self::Step { levels } => {
                let mut acc = 0u64;
                for step in levels {
                    acc += step.duration_secs;
                    if elapsed.as_secs() < acc {
                        return step.cps;
                    }
                }
                levels.last().map_or(base_cps, |s| s.cps)
            }
            Self::Spike {
                base,
                peak,
                spike_at_pct,
                spike_duration_secs,
            } => {
                let total = total_duration.as_secs_f64();
                if total <= 0.0 {
                    return *base;
                }
                let spike_start = total * spike_at_pct / 100.0;
                let spike_end = spike_start + *spike_duration_secs as f64;
                let t = elapsed.as_secs_f64();
                if t >= spike_start && t < spike_end {
                    *peak
                } else {
                    *base
                }
            }
        }
    }
}

/// 音訊 Codec
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Pcma,
    Pcmu,
}

impl Codec {
    pub fn payload_type(&self) -> u8 {
        match self {
            Self::Pcma => 8,
            Self::Pcmu => 0,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Pcma => "PCMA",
            Self::Pcmu => "PCMU",
        }
    }

    pub fn sdp_rtpmap(&self) -> &'static str {
        match self {
            Self::Pcma => "a=rtpmap:8 PCMA/8000",
            Self::Pcmu => "a=rtpmap:0 PCMU/8000",
        }
    }
}

/// 壓測模式：民眾端（主動撥出）vs 座席端（接聽）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 民眾端：壓測程式主動發 INVITE
    #[default]
    Caller,
    /// 座席端：模擬 SIP 話機，REGISTER 後等待來電並自動接聽
    Agent,
}

/// 座席帳號
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentAccount {
    pub extension: String,
    pub username: String,
    pub password: String,
    pub domain: String,
}

impl AgentAccount {
    /// 解析帳號清單文字，格式與 GUI 匯入相同（一行一個帳號）：
    ///   - `extension,username,password[,domain]`（CSV，第一行可為標題列）
    ///   - `extension:password[:domain]`（username = extension）
    ///   - `extension username password [domain]`（空白分隔）
    ///
    /// 空行與 `#` 開頭的行略過；未填 domain 時用 `default_domain`
    /// （空字串 = 由引擎改用 SIP server 的 IP）。分機重複或格式錯誤時回傳錯誤。
    pub fn parse_list(text: &str, default_domain: &str) -> anyhow::Result<Vec<AgentAccount>> {
        let mut out: Vec<AgentAccount> = Vec::new();
        let mut errors = Vec::new();
        let mut first_content = true;
        let make = |ext: &str, user: &str, pw: &str, domain: Option<&str>| AgentAccount {
            extension: ext.to_string(),
            username: user.to_string(),
            password: pw.to_string(),
            domain: domain
                .filter(|d| !d.is_empty())
                .unwrap_or(default_domain)
                .to_string(),
        };

        for (idx, raw) in text.lines().enumerate() {
            let line = raw.trim();
            let line_no = idx + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if std::mem::take(&mut first_content) {
                let head = line
                    .split([',', ':', ' ', '\t'])
                    .next()
                    .unwrap_or("")
                    .to_lowercase();
                if matches!(head.as_str(), "extension" | "ext" | "username" | "user") {
                    continue;
                }
            }

            let acc = if line.contains(',') {
                let p: Vec<&str> = line.split(',').map(str::trim).collect();
                if p.len() < 3 {
                    errors.push(format!(
                        "第 {} 行：需為 extension,username,password",
                        line_no
                    ));
                    continue;
                }
                make(p[0], p[1], p[2], p.get(3).copied())
            } else if line.contains(':') {
                let p: Vec<&str> = line.split(':').map(str::trim).collect();
                make(
                    p[0],
                    p[0],
                    p.get(1).copied().unwrap_or(""),
                    p.get(2).copied(),
                )
            } else {
                let p: Vec<&str> = line.split_whitespace().collect();
                if p.len() < 3 {
                    errors.push(format!("第 {} 行：無法辨識格式", line_no));
                    continue;
                }
                make(p[0], p[1], p[2], p.get(3).copied())
            };
            if acc.extension.is_empty() {
                errors.push(format!("第 {} 行：分機不可空白", line_no));
            } else if out.iter().any(|a| a.extension == acc.extension) {
                errors.push(format!("第 {} 行：分機 {} 重複", line_no, acc.extension));
            } else {
                out.push(acc);
            }
        }

        if !errors.is_empty() {
            anyhow::bail!("帳號清單格式錯誤：{}", errors.join("；"));
        }
        Ok(out)
    }
}

/// 座席行為模擬（預設：立即接聽、不拒接、不主動掛斷、無休息時間）
///
/// 秒數區間在 `[min, max]` 內均勻隨機；`max` 為 0 表示停用該項。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentBehavior {
    /// 響鈴幾秒後才接聽（180 Ringing 到 200 OK 之間）
    pub ring_min_secs: f64,
    pub ring_max_secs: f64,
    /// 拒接比例（%）：響鈴後回 486 Busy Here
    pub reject_pct: f64,
    /// 不接比例（%）：一直響鈴，等 SIPX 逾時 CANCEL（回 487）；
    /// 超過 `AgentBehavior::NO_ANSWER_CAP_SECS` 仍未取消則回 480
    pub no_answer_pct: f64,
    /// 通話幾秒後由座席主動掛斷（從接聽起算；max = 0 → 等對方 BYE）
    pub talk_min_secs: f64,
    pub talk_max_secs: f64,
    /// 兩通之間的休息時間（wrap-up）：通話結束後這段時間內的來電直接回 486
    pub wrap_up_secs: f64,
}

impl AgentBehavior {
    /// 「不接」的來電最多響這麼久，之後回 480 Temporarily Unavailable
    pub const NO_ANSWER_CAP_SECS: u64 = 120;

    pub fn validate(&self) -> anyhow::Result<()> {
        let secs = [
            ("響鈴秒數", self.ring_min_secs, self.ring_max_secs),
            ("通話秒數", self.talk_min_secs, self.talk_max_secs),
            ("休息時間", self.wrap_up_secs, self.wrap_up_secs),
        ];
        for (name, min, max) in secs {
            if !(min.is_finite() && max.is_finite() && min >= 0.0 && max >= 0.0) {
                anyhow::bail!("座席{}需為 ≥ 0 的數字", name);
            }
            if min > max {
                anyhow::bail!("座席{}的最小值（{}）不可大於最大值（{}）", name, min, max);
            }
        }
        for (name, pct) in [
            ("拒接比例", self.reject_pct),
            ("不接比例", self.no_answer_pct),
        ] {
            if !(pct.is_finite() && (0.0..=100.0).contains(&pct)) {
                anyhow::bail!("座席{}需介於 0–100%（目前為 {}）", name, pct);
            }
        }
        if self.reject_pct + self.no_answer_pct > 100.0 {
            anyhow::bail!("座席拒接 + 不接比例合計不可超過 100%");
        }
        Ok(())
    }
}

/// 傳輸層協定
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Udp,
    Tcp,
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

    /// 每秒通話數（CPS），作為 Flat 模式的基準值
    pub cps: f64,

    /// CPS 負載模式（Flat / Ramp / Step / Spike）
    #[serde(default)]
    pub cps_pattern: CpsPattern,

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

    /// 音訊 Codec（預設 PCMA）
    #[serde(default)]
    pub codec: Codec,

    /// 是否啟用 RTP 傳送（false = 只做 SIP signaling）
    pub enable_rtp: bool,

    /// DTMF 序列（接通後自動送出，格式："1,w2000,2,w500,#"）
    #[serde(default)]
    pub dtmf_sequence: Option<String>,

    /// 總通話上限（None = 不限，依 duration_secs 決定結束）
    pub max_total_calls: Option<u64>,

    // ── Agent 模式 ─────────────────────────────────────────────────
    /// 壓測模式（預設 Caller）
    #[serde(default)]
    pub mode: Mode,

    /// 座席帳號列表（Agent 模式必填，Caller 模式忽略）
    #[serde(default)]
    pub agent_accounts: Vec<AgentAccount>,

    /// Agent 模式：等待 ACK 的逾時秒數（預設 10）
    #[serde(default = "default_ack_timeout")]
    pub agent_ack_timeout_secs: u64,

    /// Agent 模式：等待 BYE 的逾時秒數（預設 150）
    #[serde(default = "default_bye_timeout")]
    pub agent_bye_timeout_secs: u64,

    /// Agent 模式：座席行為模擬（響鈴延遲、拒接 / 不接、主動掛斷、休息時間）
    #[serde(default)]
    pub agent_behavior: AgentBehavior,
}

fn default_ack_timeout() -> u64 {
    10
}
fn default_bye_timeout() -> u64 {
    150
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:5060".to_string(),
            local_addr: None,
            local_domain: None,
            caller_number: "1000".to_string(),
            callee_prefix: "2".to_string(),
            callee_range: 9999,
            callee_fixed: None,
            cps: 10.0,
            cps_pattern: CpsPattern::default(),
            max_concurrent_calls: 100,
            duration_secs: 60,
            call_duration_secs: 30,
            invite_timeout_secs: 8,
            transport: Transport::Udp,
            logs_dir: "logs".to_string(),
            rtp_base_port: 16000,
            audio_file: None,
            codec: Codec::default(),
            enable_rtp: false,
            dtmf_sequence: None,
            max_total_calls: None,
            mode: Mode::Caller,
            agent_accounts: Vec::new(),
            agent_ack_timeout_secs: default_ack_timeout(),
            agent_bye_timeout_secs: default_bye_timeout(),
            agent_behavior: AgentBehavior::default(),
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
        if self.mode == Mode::Agent {
            self.agent_behavior.validate()?;
        }
        if self.invite_timeout_secs == 0 {
            anyhow::bail!("INVITE 逾時秒數必須大於 0");
        }
        if let Some(local) = self.local_addr.as_deref() {
            let valid = local.parse::<std::net::SocketAddr>().is_ok()
                || local.parse::<std::net::IpAddr>().is_ok();
            if !valid {
                anyhow::bail!("本機綁定位址格式錯誤（需為 IP 或 IP:port）：{}", local);
            }
        }
        Ok(())
    }

    /// 從 CLI 參數快速建構
    pub fn from_args(
        server_addr: impl Into<String>,
        cps: f64,
        max_concurrent_calls: usize,
        duration_secs: u64,
        call_duration_secs: u64,
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
    fn flat_pattern_returns_base_cps() {
        let p = CpsPattern::Flat;
        let cps = p.current_cps(Duration::from_secs(30), 10.0, Duration::from_secs(60));
        assert!((cps - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ramp_interpolates_linearly() {
        let p = CpsPattern::Ramp {
            start: 10.0,
            end: 50.0,
        };
        let total = Duration::from_secs(100);

        let at_0 = p.current_cps(Duration::from_secs(0), 0.0, total);
        assert!((at_0 - 10.0).abs() < 0.01);

        let at_50 = p.current_cps(Duration::from_secs(50), 0.0, total);
        assert!((at_50 - 30.0).abs() < 0.01);

        let at_100 = p.current_cps(Duration::from_secs(100), 0.0, total);
        assert!((at_100 - 50.0).abs() < 0.01);
    }

    #[test]
    fn step_selects_correct_level() {
        let p = CpsPattern::Step {
            levels: vec![
                CpsStep {
                    cps: 5.0,
                    duration_secs: 10,
                },
                CpsStep {
                    cps: 20.0,
                    duration_secs: 10,
                },
                CpsStep {
                    cps: 50.0,
                    duration_secs: 10,
                },
            ],
        };
        let total = Duration::from_secs(30);
        assert!((p.current_cps(Duration::from_secs(5), 1.0, total) - 5.0).abs() < 0.01);
        assert!((p.current_cps(Duration::from_secs(15), 1.0, total) - 20.0).abs() < 0.01);
        assert!((p.current_cps(Duration::from_secs(25), 1.0, total) - 50.0).abs() < 0.01);
        assert!((p.current_cps(Duration::from_secs(35), 1.0, total) - 50.0).abs() < 0.01);
    }

    #[test]
    fn spike_activates_at_correct_time() {
        let p = CpsPattern::Spike {
            base: 10.0,
            peak: 100.0,
            spike_at_pct: 50.0,
            spike_duration_secs: 10,
        };
        let total = Duration::from_secs(100);
        assert!((p.current_cps(Duration::from_secs(40), 0.0, total) - 10.0).abs() < 0.01);
        assert!((p.current_cps(Duration::from_secs(55), 0.0, total) - 100.0).abs() < 0.01);
        assert!((p.current_cps(Duration::from_secs(65), 0.0, total) - 10.0).abs() < 0.01);
    }

    #[test]
    fn default_config_is_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn zero_or_invalid_cps_is_rejected() {
        for cps in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let cfg = Config {
                cps,
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "cps={} 應被拒絕", cps);
        }
    }

    #[test]
    fn agent_mode_ignores_cps() {
        let cfg = Config {
            mode: Mode::Agent,
            cps: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn zero_concurrency_and_timeout_are_rejected() {
        assert!(Config {
            max_concurrent_calls: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(Config {
            invite_timeout_secs: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn local_addr_accepts_ip_or_ip_port() {
        for ok in ["127.0.0.1", "0.0.0.0:5070", "192.168.1.10:15060"] {
            let cfg = Config {
                local_addr: Some(ok.into()),
                ..Default::default()
            };
            assert!(cfg.validate().is_ok(), "{} 應為合法的本機位址", ok);
        }
        for bad in ["localhost", "0.0.0.0:", "0.0.0.0:99999", "1.2.3"] {
            let cfg = Config {
                local_addr: Some(bad.into()),
                ..Default::default()
            };
            assert!(cfg.validate().is_err(), "{} 應被拒絕", bad);
        }
    }

    #[test]
    fn agent_behavior_validation() {
        let ok = AgentBehavior {
            ring_min_secs: 1.0,
            ring_max_secs: 3.0,
            reject_pct: 20.0,
            no_answer_pct: 80.0,
            talk_min_secs: 5.0,
            talk_max_secs: 5.0,
            wrap_up_secs: 2.0,
        };
        assert!(ok.validate().is_ok());
        assert!(AgentBehavior::default().validate().is_ok());
        let bad = [
            AgentBehavior {
                ring_min_secs: 3.0,
                ring_max_secs: 1.0,
                ..Default::default()
            },
            AgentBehavior {
                talk_max_secs: -1.0,
                ..Default::default()
            },
            AgentBehavior {
                wrap_up_secs: f64::NAN,
                ..Default::default()
            },
            AgentBehavior {
                reject_pct: 101.0,
                ..Default::default()
            },
            AgentBehavior {
                reject_pct: 60.0,
                no_answer_pct: 50.0,
                ..Default::default()
            },
        ];
        for b in bad {
            assert!(b.validate().is_err(), "{:?} 應被拒絕", b);
            // 只在座席模式檢查
            assert!(Config {
                agent_behavior: b.clone(),
                ..Default::default()
            }
            .validate()
            .is_ok());
            assert!(Config {
                mode: Mode::Agent,
                agent_behavior: b,
                ..Default::default()
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn account_list_accepts_gui_import_formats() {
        let text = "extension,username,password,domain\n\
                    # 註解\n\
                    \n\
                    1001,u1001,pw1,sip.example\n\
                    1002,u1002,pw2\n\
                    1003:pw3\n\
                    1004:pw4:other.example\n\
                    1005 u1005 pw5\r\n";
        let list = AgentAccount::parse_list(text, "10.0.0.1").unwrap();
        let got: Vec<_> = list
            .iter()
            .map(|a| {
                (
                    a.extension.as_str(),
                    a.username.as_str(),
                    a.password.as_str(),
                    a.domain.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("1001", "u1001", "pw1", "sip.example"),
                ("1002", "u1002", "pw2", "10.0.0.1"),
                ("1003", "1003", "pw3", "10.0.0.1"),
                ("1004", "1004", "pw4", "other.example"),
                ("1005", "u1005", "pw5", "10.0.0.1"),
            ]
        );
    }

    #[test]
    fn account_list_header_only_skipped_on_first_line() {
        // 第一行不是標題列時照常解析；標題字樣出現在後面的行也是一般帳號
        let list = AgentAccount::parse_list("1001,u,p\nuser,u2,p2", "").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].extension, "user");
        assert_eq!(list[0].domain, "");
    }

    #[test]
    fn account_list_rejects_bad_lines_and_duplicates() {
        let err = AgentAccount::parse_list("1001,u\n1002\n1003,u,p\n1003:p", "")
            .unwrap_err()
            .to_string();
        assert!(err.contains("第 1 行"), "{}", err);
        assert!(err.contains("第 2 行"), "{}", err);
        assert!(err.contains("第 4 行") && err.contains("重複"), "{}", err);
        assert!(!err.contains("第 3 行"), "{}", err);
    }
}
