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
    /// 批次：每 `every_secs` 秒一次發出 `count` 通（測試開始時發第一批）；
    /// 發出時併發已滿，就只補空出的線路，這一批剩下的略過、不累積
    Batch {
        count: u32,
        every_secs: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpsStep {
    pub cps: f64,
    pub duration_secs: u64,
}

impl CpsPattern {
    /// 檢查參數：數值需為 ≥ 0 的有限數；漸增 / 尖峰依總時長計算，需設定持續時間
    pub fn validate(&self, duration_secs: u64) -> anyhow::Result<()> {
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        match self {
            Self::Flat => {}
            Self::Ramp { start, end } => {
                if !(ok(*start) && ok(*end)) || start.max(*end) <= 0.0 {
                    anyhow::bail!("CPS 漸增的起訖值需 ≥ 0，且至少一個 > 0");
                }
                if duration_secs == 0 {
                    anyhow::bail!("CPS 漸增模式需設定持續時間（> 0）");
                }
            }
            Self::Step { levels } => {
                if levels.is_empty() {
                    anyhow::bail!("CPS 階梯模式至少需要一個階層");
                }
                if levels.iter().any(|l| !(l.cps.is_finite() && l.cps > 0.0)) {
                    anyhow::bail!("CPS 階梯的每一階 CPS 需 > 0");
                }
            }
            Self::Spike {
                base,
                peak,
                spike_at_pct,
                ..
            } => {
                if !(ok(*base) && ok(*peak)) || base.max(*peak) <= 0.0 {
                    anyhow::bail!("CPS 尖峰的基準 / 尖峰值需 ≥ 0，且至少一個 > 0");
                }
                if !(spike_at_pct.is_finite() && (0.0..=100.0).contains(spike_at_pct)) {
                    anyhow::bail!("CPS 尖峰開始位置需介於 0–100%");
                }
                if duration_secs == 0 {
                    anyhow::bail!("CPS 尖峰模式需設定持續時間（> 0）");
                }
            }
            Self::Batch { count, every_secs } => {
                if *count == 0 {
                    anyhow::bail!("批次模式每批至少 1 通");
                }
                if !(every_secs.is_finite() && *every_secs > 0.0) {
                    anyhow::bail!("批次模式的間隔秒數需 > 0");
                }
            }
        }
        Ok(())
    }

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
            // 平均速率（引擎依批次排程發話，不使用這個值排程）
            Self::Batch { count, every_secs } => *count as f64 / every_secs.max(0.001),
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
    /// 通話中忙線：已有通話（含響鈴中）時，新來電直接回 486，讓 ACD 改派給其他座席。
    /// 關閉時同一帳號可同時接多通
    pub busy_in_call: bool,
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

/// 座席流程的一個步驟
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FlowStep {
    /// 撥特殊碼（例如 `*96`、`*94{agent}*{pwd}`）
    Dial {
        code: String,
        /// 顯示用的名稱（例如「就緒」）
        #[serde(default)]
        label: String,
    },
    /// 等待秒數
    Wait { secs: f64 },
}

impl FlowStep {
    /// 撥碼可用的字元：數字、`*`、`#`、英文字母，以及變數的大括號
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Dial { code, .. } => {
                if code.trim().is_empty() {
                    anyhow::bail!("座席流程的撥碼不可空白");
                }
                if let Some(c) = code
                    .chars()
                    .find(|c| !(c.is_ascii_alphanumeric() || "*#{}".contains(*c)))
                {
                    anyhow::bail!("座席流程的撥碼「{}」含不支援的字元「{}」", code, c);
                }
            }
            Self::Wait { secs } => {
                if !(secs.is_finite() && *secs >= 0.0) {
                    anyhow::bail!("座席流程的等待秒數需為 ≥ 0 的數字（目前為 {}）", secs);
                }
            }
        }
        Ok(())
    }
}

/// 座席流程：在三個時間點依序執行的撥碼 / 等待步驟（例如註冊後撥 `*94…` 登入、`*96` 就緒）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentFlow {
    /// 註冊成功後（每個帳號一次）
    pub on_start: Vec<FlowStep>,
    /// 每通通話結束後
    pub after_call: Vec<FlowStep>,
    /// 測試結束、解除註冊前
    pub on_stop: Vec<FlowStep>,
    /// `{agent}` 取分機號碼的末幾碼（0 = 整個分機號碼）
    pub agent_digits: u32,
    /// `{pwd}` 的值（座席密碼）
    pub agent_password: String,
    /// 撥碼接通（200 OK）或開始播提示音（18x）後保持幾秒再掛斷；交換機先掛斷就提早結束。
    /// 提示音期間分機仍被佔用（SIPX 派來的電話會失敗），所以預設很短
    pub hold_secs: f64,
}

impl Default for AgentFlow {
    fn default() -> Self {
        Self {
            on_start: Vec::new(),
            after_call: Vec::new(),
            on_stop: Vec::new(),
            agent_digits: 4,
            agent_password: String::new(),
            hold_secs: 1.0,
        }
    }
}

impl AgentFlow {
    /// 撥碼沒有最終回應時放棄的秒數
    pub const DIAL_TIMEOUT_SECS: u64 = 15;
    /// 送出 BYE 後等待回應的秒數
    pub const BYE_TIMEOUT_SECS: u64 = 4;

    pub fn is_empty(&self) -> bool {
        self.on_start.is_empty() && self.after_call.is_empty() && self.on_stop.is_empty()
    }

    /// 代入 `{agent}`（分機末 `agent_digits` 碼）與 `{pwd}`
    pub fn expand(&self, code: &str, extension: &str) -> String {
        let digits = self.agent_digits as usize;
        let agent = if digits == 0 || digits >= extension.len() {
            extension
        } else {
            &extension[extension.len() - digits..]
        };
        code.replace("{agent}", agent)
            .replace("{pwd}", &self.agent_password)
    }

    /// 結束流程最多需要多久（每個撥碼以逾時 + 保持時間計），用來延長引擎的收尾時間
    pub fn stop_budget(&self) -> Duration {
        let secs: f64 = self
            .on_stop
            .iter()
            .map(|s| match s {
                FlowStep::Dial { .. } => {
                    (Self::DIAL_TIMEOUT_SECS + Self::BYE_TIMEOUT_SECS) as f64 + self.hold_secs
                }
                FlowStep::Wait { secs } => *secs,
            })
            .sum();
        Duration::from_secs_f64(secs)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for step in self
            .on_start
            .iter()
            .chain(&self.after_call)
            .chain(&self.on_stop)
        {
            step.validate()?;
        }
        if !(self.hold_secs.is_finite() && (0.0..=60.0).contains(&self.hold_secs)) {
            anyhow::bail!(
                "座席流程的撥通後保持秒數需介於 0–60（目前為 {}）",
                self.hold_secs
            );
        }
        Ok(())
    }
}

/// 傳輸層協定
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
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

    /// 每通使用不同主叫號碼：第 n 通 = `caller_number` + n（保留位數，例如 0912000000 → 0912000001）。
    /// 端到端對照在拿不到 `P-CIN-CallId` 時，靠主叫號碼才能一對一配對
    #[serde(default)]
    pub caller_number_seq: bool,

    /// 民眾端撥號認證：SIPX 對 INVITE 回 401 / 407 時，帶 Digest 認證重送一次。
    /// 密碼為 None 時不認證（直接算失敗）；帳號為空時用該通的主叫號碼
    #[serde(default)]
    pub caller_auth_username: Option<String>,
    #[serde(default)]
    pub caller_auth_password: Option<String>,

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

    /// 分時段統計的時段長度（秒，0 = 不分段）；長時間耐久測試看第幾段開始變差
    #[serde(default = "default_stats_interval")]
    pub stats_interval_secs: u64,

    /// Agent 模式：每通結束後立即重新 REGISTER（預設關閉，只在接近 Expires 時刷新）。
    /// 只有交換機要求通話後重新註冊才會繼續派話時才需要開啟；高話務下會對交換機多送大量 REGISTER
    #[serde(default)]
    pub agent_reregister_after_call: bool,

    /// Agent 模式：各帳號開始註冊的間隔（毫秒，0 = 同時註冊）。
    /// 大量座席同時 REGISTER 會有部分被交換機丟掉或拒絕時，拉開間隔分批註冊
    #[serde(default)]
    pub agent_register_interval_ms: u64,

    /// Agent 模式：註冊後 / 每通結束後 / 結束前要撥的特殊碼（座席登入、就緒、登出等）
    #[serde(default)]
    pub agent_flow: AgentFlow,

    /// Caller 模式：線路釋放後幾秒才補新的通話（0 = 立即補）。
    /// 避免新來電剛好在座席掛線的瞬間進線（真實話務不會每通都緊接著前一通）
    #[serde(default)]
    pub refill_delay_secs: f64,
}

fn default_ack_timeout() -> u64 {
    10
}
fn default_stats_interval() -> u64 {
    300
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
            caller_number_seq: false,
            caller_auth_username: None,
            caller_auth_password: None,
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
            stats_interval_secs: default_stats_interval(),
            agent_reregister_after_call: false,
            agent_register_interval_ms: 0,
            agent_flow: AgentFlow::default(),
            refill_delay_secs: 0.0,
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
    /// 撥號認證的帳號與密碼（未設定密碼時為 None）；帳號未填時用該通的主叫號碼
    pub fn caller_auth<'a>(&'a self, caller_number: &'a str) -> Option<(&'a str, &'a str)> {
        let password = self.caller_auth_password.as_deref()?;
        let user = self
            .caller_auth_username
            .as_deref()
            .filter(|u| !u.is_empty())
            .unwrap_or(caller_number);
        Some((user, password))
    }

    /// 第 `n` 通（從 0 起算）的主叫號碼
    pub fn caller_number_for(&self, n: u64) -> String {
        if !self.caller_number_seq {
            return self.caller_number.clone();
        }
        let base = &self.caller_number;
        if base.is_empty() || !base.bytes().all(|b| b.is_ascii_digit()) || base.len() > 30 {
            return format!("{}{}", base, n);
        }
        // 純數字：在原位數內遞增（超過位數就從頭繞回），保留開頭的 0
        let width = base.len() as u32;
        let modulus = 10u128.pow(width);
        let value = (base.parse::<u128>().unwrap_or(0) + n as u128) % modulus;
        format!("{:0width$}", value, width = width as usize)
    }

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
            self.cps_pattern.validate(self.duration_secs)?;
            if !(self.refill_delay_secs.is_finite() && self.refill_delay_secs >= 0.0) {
                anyhow::bail!("補話延遲秒數需 ≥ 0（目前為 {}）", self.refill_delay_secs);
            }
            if let Some(seq) = self.dtmf_sequence.as_deref() {
                crate::rtp::dtmf::validate_dtmf_sequence(seq).map_err(anyhow::Error::msg)?;
            }
        }
        if self.mode == Mode::Agent {
            self.agent_behavior.validate()?;
            self.agent_flow.validate()?;
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
    fn batch_pattern_validation_and_rate() {
        let b = |count, every_secs| CpsPattern::Batch { count, every_secs };
        assert!(b(3, 20.0).validate(0).is_ok());
        assert!(b(0, 20.0).validate(0).is_err());
        assert!(b(3, 0.0).validate(0).is_err());
        assert!(b(3, f64::NAN).validate(0).is_err());
        let rate = b(3, 20.0).current_cps(Duration::ZERO, 10.0, Duration::from_secs(60));
        assert!((rate - 0.15).abs() < 1e-9);
        let json = r#"{"type":"batch","count":3,"every_secs":20}"#;
        assert!(matches!(
            serde_json::from_str::<CpsPattern>(json).unwrap(),
            CpsPattern::Batch { count: 3, .. }
        ));
        assert!(Config {
            refill_delay_secs: -1.0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn cps_pattern_validation() {
        assert!(CpsPattern::Flat.validate(0).is_ok());
        let ramp = CpsPattern::Ramp {
            start: 1.0,
            end: 50.0,
        };
        assert!(ramp.validate(60).is_ok());
        assert!(ramp.validate(0).is_err(), "漸增需要持續時間");
        assert!(CpsPattern::Ramp {
            start: 0.0,
            end: 0.0
        }
        .validate(60)
        .is_err());
        assert!(CpsPattern::Ramp {
            start: -1.0,
            end: 5.0
        }
        .validate(60)
        .is_err());
        assert!(CpsPattern::Step { levels: vec![] }.validate(0).is_err());
        let step = |cps| CpsPattern::Step {
            levels: vec![CpsStep {
                cps,
                duration_secs: 10,
            }],
        };
        assert!(step(5.0).validate(0).is_ok(), "階梯不需要總時長");
        assert!(step(0.0).validate(0).is_err());
        let spike = |at| CpsPattern::Spike {
            base: 10.0,
            peak: 100.0,
            spike_at_pct: at,
            spike_duration_secs: 5,
        };
        assert!(spike(50.0).validate(60).is_ok());
        assert!(spike(150.0).validate(60).is_err());
        assert!(spike(50.0).validate(0).is_err());
    }

    #[test]
    fn caller_config_rejects_bad_dtmf_and_pattern() {
        let base = Config {
            duration_secs: 60,
            ..Default::default()
        };
        assert!(Config {
            dtmf_sequence: Some("1,w500,#".into()),
            ..base.clone()
        }
        .validate()
        .is_ok());
        assert!(Config {
            dtmf_sequence: Some("12".into()),
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(Config {
            cps_pattern: CpsPattern::Ramp {
                start: 1.0,
                end: 5.0
            },
            duration_secs: 0,
            ..base
        }
        .validate()
        .is_err());
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
    fn agent_flow_expands_agent_and_password() {
        let flow = AgentFlow {
            agent_password: "000000".into(),
            ..Default::default()
        };
        assert_eq!(
            flow.expand("*94{agent}*{pwd}", "0000021113333001"),
            "*943001*000000"
        );
        assert_eq!(flow.expand("*96", "0000021113333001"), "*96");
        let whole = AgentFlow {
            agent_digits: 0,
            ..Default::default()
        };
        assert_eq!(whole.expand("*94{agent}", "1001"), "*941001");
        let longer = AgentFlow {
            agent_digits: 8,
            ..Default::default()
        };
        assert_eq!(
            longer.expand("{agent}", "1001"),
            "1001",
            "分機比位數短時用整個分機"
        );
    }

    #[test]
    fn agent_flow_validation_and_stop_budget() {
        let dial = |code: &str| FlowStep::Dial {
            code: code.into(),
            label: String::new(),
        };
        let ok = AgentFlow {
            on_start: vec![dial("*94{agent}*{pwd}"), FlowStep::Wait { secs: 1.5 }],
            on_stop: vec![dial("*97"), FlowStep::Wait { secs: 2.0 }, dial("#95")],
            hold_secs: 3.0,
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
        // 兩個撥碼各 (15 + 4 + 3) 秒，加上等待 2 秒
        assert_eq!(ok.stop_budget(), Duration::from_secs(46));

        for bad in [
            AgentFlow {
                after_call: vec![dial(" ")],
                ..Default::default()
            },
            AgentFlow {
                on_start: vec![dial("*96;x")],
                ..Default::default()
            },
            AgentFlow {
                on_stop: vec![FlowStep::Wait { secs: -1.0 }],
                ..Default::default()
            },
            AgentFlow {
                hold_secs: f64::NAN,
                ..Default::default()
            },
        ] {
            assert!(bad.validate().is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn agent_flow_json_round_trips() {
        let json = r#"{"on_start":[{"type":"dial","code":"*96","label":"就緒"},{"type":"wait","secs":2}],
                       "agent_password":"000000"}"#;
        let flow: AgentFlow = serde_json::from_str(json).unwrap();
        assert_eq!(flow.on_start.len(), 2);
        assert_eq!(flow.agent_digits, 4, "未填欄位取預設值");
        assert_eq!(flow.hold_secs, 1.0);
        let back: AgentFlow = serde_json::from_str(&serde_json::to_string(&flow).unwrap()).unwrap();
        assert_eq!(back, flow);
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
    fn caller_auth_defaults_username_to_caller_number() {
        let none = Config::default();
        assert_eq!(none.caller_auth("0912"), None, "沒有密碼就不認證");
        let pw_only = Config {
            caller_auth_password: Some("pw".into()),
            ..Default::default()
        };
        assert_eq!(pw_only.caller_auth("0912"), Some(("0912", "pw")));
        let empty_user = Config {
            caller_auth_username: Some(String::new()),
            ..pw_only.clone()
        };
        assert_eq!(empty_user.caller_auth("0912"), Some(("0912", "pw")));
        let both = Config {
            caller_auth_username: Some("trunk".into()),
            ..pw_only
        };
        assert_eq!(both.caller_auth("0912"), Some(("trunk", "pw")));
    }

    #[test]
    fn caller_number_sequence() {
        let fixed = Config {
            caller_number: "0912000000".into(),
            ..Default::default()
        };
        assert_eq!(fixed.caller_number_for(5), "0912000000");

        let seq = Config {
            caller_number_seq: true,
            ..fixed
        };
        assert_eq!(seq.caller_number_for(0), "0912000000");
        assert_eq!(seq.caller_number_for(1), "0912000001");
        assert_eq!(seq.caller_number_for(12345), "0912012345");

        let wrap = Config {
            caller_number: "98".into(),
            caller_number_seq: true,
            ..Default::default()
        };
        assert_eq!(wrap.caller_number_for(3), "01", "超過位數繞回，位數不變");

        let text = Config {
            caller_number: "anon".into(),
            caller_number_seq: true,
            ..Default::default()
        };
        assert_eq!(text.caller_number_for(7), "anon7");
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
            busy_in_call: true,
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
