/// CLI 參數（clap derive）
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name    = "sipress",
    about   = "SIP 軟體交換機壓測工具",
    version,  // 取自 Cargo.toml，release 時只需改 Cargo.toml / tauri.conf.json
    long_about = None,
)]
pub struct Args {
    /// 目標 SIP 伺服器地址（格式：ip:port）
    #[arg(short = 's', long, default_value = "127.0.0.1:5060")]
    pub server: String,

    /// 最大並發通話數
    #[arg(short = 'c', long = "concurrent", default_value_t = 100)]
    pub max_concurrent: usize,

    /// 每秒通話數（CPS）
    #[arg(long, default_value_t = 10.0)]
    pub cps: f64,

    /// 壓測持續秒數（0 = 不限，直到 Ctrl-C 或 TUI 按 q）
    #[arg(short = 'd', long = "duration", default_value_t = 60)]
    pub duration: u64,

    /// 單通通話持續秒數（0 = 不主動 BYE）
    #[arg(long = "call-duration", default_value_t = 30)]
    pub call_duration: u64,

    /// INVITE 逾時秒數
    #[arg(long = "invite-timeout", default_value_t = 8)]
    pub invite_timeout: u64,

    /// 主叫號碼（接入號 / Caller-ID）
    #[arg(long = "from", alias = "number", default_value = "1000")]
    pub caller: String,

    /// 固定被叫號碼（指定後所有通話都打給這個號碼，覆蓋 --to-prefix/--to-range）
    #[arg(long = "to")]
    pub callee_fixed: Option<String>,

    /// 被叫號碼前綴（與 --to-range 搭配產生隨機被叫；--to 指定時忽略）
    #[arg(long = "to-prefix", default_value = "2")]
    pub callee_prefix: String,

    /// 被叫號碼尾數最大值（0..=N 隨機；--to 指定時忽略）
    #[arg(long = "to-range", default_value_t = 9999)]
    pub callee_range: u64,

    /// 總通話上限（0 或不設 = 不限，依 duration 決定結束）
    #[arg(long = "max-calls", default_value_t = 0)]
    pub max_calls: u64,

    /// 傳輸層協定：udp（預設）/ tcp
    #[arg(long = "transport", default_value = "udp")]
    pub transport: String,

    /// 模式：caller（民眾端，主動撥出）/ agent（座席端，註冊後等待來電並接聽）
    #[arg(long = "mode", default_value = "caller", value_parser = ["caller", "agent"])]
    pub mode: String,

    /// 座席帳號檔（agent 模式必填）。一行一個帳號，格式同 GUI 匯入：
    /// `extension,username,password[,domain]`、`extension:password[:domain]` 或以空白分隔
    #[arg(long = "accounts")]
    pub accounts_file: Option<std::path::PathBuf>,

    /// 座席帳號未填 domain 時使用的預設值（不設 = SIP server 的 IP）
    #[arg(long = "account-domain", default_value = "")]
    pub account_domain: String,

    /// 座席端：接聽後等待 ACK 的逾時秒數
    #[arg(long = "ack-timeout", default_value_t = 10)]
    pub ack_timeout: u64,

    /// 座席端：通話中等待對方 BYE 的逾時秒數（逾時主動掛斷）
    #[arg(long = "bye-timeout", default_value_t = 150)]
    pub bye_timeout: u64,

    /// 本機綁定 IP 或 IP:port（預設自動選擇 IP，port 由系統分配）
    #[arg(long = "local")]
    pub local_addr: Option<String>,

    /// 本機 SIP domain
    #[arg(long = "domain")]
    pub local_domain: Option<String>,

    /// 啟用 TUI 即時儀表板
    #[arg(long)]
    pub tui: bool,

    /// 輸出格式：table（預設）/ json / csv
    #[arg(long = "format", default_value = "table")]
    pub format: String,

    /// 額外輸出 HTML 報告
    #[arg(long)]
    pub html: bool,

    /// HTML 報告輸出目錄
    #[arg(long = "report-dir", default_value = "reports")]
    pub report_dir: String,

    /// SIP log 輸出目錄
    #[arg(long = "logs-dir", default_value = "logs")]
    pub logs_dir: String,

    /// 啟用 RTP 音訊傳送
    #[arg(long = "rtp")]
    pub enable_rtp: bool,

    /// RTP 起始 port（偶數，預設 10000）
    #[arg(long = "rtp-port", default_value_t = 10000)]
    pub rtp_base_port: u16,

    /// 音檔路徑（自動轉換為 G.711A/PCMA 送出）
    /// 支援：.wav（PCM16/A-law/μ-law）、.ul/.ulaw（raw μ-law）、.al/.alaw（raw A-law）
    #[arg(long = "audio")]
    pub audio_file: Option<std::path::PathBuf>,

    /// 顯示詳細 debug log
    #[arg(long)]
    pub verbose: bool,
}
