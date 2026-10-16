/// CLI 參數（clap derive）
use clap::Parser;
use sipress_core::config::{CpsPattern, CpsStep};

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

    /// CPS 漸增：`START-END`，在 --duration 內從 START 線性升到 END（取代 --cps）
    #[arg(long = "cps-ramp", value_parser = parse_cps_ramp, conflicts_with_all = ["cps_steps", "cps_spike", "cps_batch"])]
    pub cps_ramp: Option<CpsPattern>,

    /// CPS 階梯：`CPS:秒,CPS:秒,...`，依序維持各階，最後一階維持到結束（取代 --cps）
    #[arg(long = "cps-steps", value_parser = parse_cps_steps, conflicts_with_all = ["cps_spike", "cps_batch"])]
    pub cps_steps: Option<CpsPattern>,

    /// CPS 尖峰：`基準,尖峰,開始%,尖峰秒數`，例如 `10,100,50,10` = 平時 10，跑到 50% 時 100 CPS 持續 10 秒（取代 --cps）
    #[arg(long = "cps-spike", value_parser = parse_cps_spike, conflicts_with = "cps_batch")]
    pub cps_spike: Option<CpsPattern>,

    /// 批次：`通數/秒數`，例如 `3/20` = 每 20 秒一次發出 3 通；併發已滿時只補空出的線路，
    /// 剩下的略過（取代 --cps）
    #[arg(long = "cps-batch", value_parser = parse_cps_batch)]
    pub cps_batch: Option<CpsPattern>,

    /// 線路釋放後幾秒才補新的通話（0 = 立即補）；避免新來電剛好在座席掛線的瞬間進線
    #[arg(long = "refill-delay", default_value_t = 0.0)]
    pub refill_delay: f64,

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

    /// 每通使用不同主叫號碼（從 --from 起遞增、保留位數），端到端對照拿不到 P-CIN-CallId 時靠它配對
    #[arg(long = "from-seq")]
    pub caller_number_seq: bool,

    /// 撥號認證帳號：SIPX 對 INVITE 回 401 / 407 時用（未填 = 該通的主叫號碼）
    #[arg(long = "auth-user")]
    pub auth_user: Option<String>,

    /// 撥號認證密碼：設定後收到 401 / 407 會帶 Digest 認證重送 INVITE 一次
    #[arg(long = "auth-pass")]
    pub auth_pass: Option<String>,

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
    #[arg(long = "transport", default_value = "udp", value_parser = ["udp", "tcp", "UDP", "TCP"])]
    pub transport: String,

    /// 模式：caller（民眾端，主動撥出）/ agent（座席端，註冊後等待來電並接聽）
    #[arg(long = "mode", default_value = "caller", value_parser = ["caller", "agent"])]
    pub mode: String,

    /// 座席帳號檔（agent 模式必填）。一行一個帳號，格式同 GUI 匯入：
    /// `extension,username,password[,domain]`、`extension:password[:domain]` 或以空白分隔
    #[arg(long = "accounts")]
    pub accounts_file: Option<std::path::PathBuf>,

    /// 座席流程檔（JSON，GUI「座席流程」可匯出）：註冊後 / 每通結束後 / 結束前要撥的特殊碼，
    /// 例如登入 `*94{agent}*{pwd}`、就緒 `*96`、未就緒 `*97`、登出 `*95`
    #[arg(long = "agent-flow")]
    pub agent_flow_file: Option<std::path::PathBuf>,

    /// 座席帳號未填 domain 時使用的預設值（不設 = SIP server 的 IP）
    #[arg(long = "account-domain", default_value = "")]
    pub account_domain: String,

    /// 座席端：接聽後等待 ACK 的逾時秒數
    #[arg(long = "ack-timeout", default_value_t = 10)]
    pub ack_timeout: u64,

    /// 座席端：通話中等待對方 BYE 的逾時秒數（逾時主動掛斷）
    #[arg(long = "bye-timeout", default_value_t = 150)]
    pub bye_timeout: u64,

    /// 座席端：響鈴幾秒後才接聽，`N` 或區間 `MIN-MAX`（隨機）
    #[arg(long = "ring", value_parser = parse_secs_range, default_value = "0")]
    pub ring: (f64, f64),

    /// 座席端：拒接比例 %（響鈴後回 486 Busy Here）
    #[arg(long = "reject-pct", default_value_t = 0.0)]
    pub reject_pct: f64,

    /// 座席端：不接比例 %（一直響鈴，等 SIPX 逾時 CANCEL 重派）
    #[arg(long = "no-answer-pct", default_value_t = 0.0)]
    pub no_answer_pct: f64,

    /// 座席端：接聽幾秒後由座席主動掛斷，`N` 或區間 `MIN-MAX`（0 = 等對方 BYE）
    #[arg(long = "talk", value_parser = parse_secs_range, default_value = "0")]
    pub talk: (f64, f64),

    /// 座席端：每通結束後立即重新 REGISTER（預設只在 Expires 到期前刷新）。
    /// 只有交換機要求通話後重新註冊才會繼續派話時才需要
    #[arg(long = "reregister-after-call")]
    pub reregister_after_call: bool,

    /// 座席端：各帳號開始註冊的間隔毫秒數（0 = 同時註冊）。大量帳號同時註冊會部分失敗時使用
    #[arg(long = "register-interval", default_value_t = 0)]
    pub register_interval: u64,

    /// 座席端：REGISTER 要求的有效期秒數（伺服器可能改給其他值，在有效期的 2/3 時刷新）
    #[arg(long = "register-expires", default_value_t = 3600)]
    pub register_expires: u32,

    /// 座席端：通話中忙線——已有通話（含響鈴中）時新來電回 486，讓 ACD 改派
    #[arg(long = "busy-in-call")]
    pub busy_in_call: bool,

    /// 座席端：兩通之間的休息秒數（wrap-up），期間來電回 486
    #[arg(long = "wrap-up", default_value_t = 0.0)]
    pub wrap_up: f64,

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

    /// 偏好的 codec：pcma（預設）/ pcmu；實際使用的以 SDP 協商結果為準
    #[arg(long = "codec", default_value = "pcma", value_parser = ["pcma", "pcmu"], ignore_case = true)]
    pub codec: String,

    /// 民眾端：接通後以 RTP（RFC 4733）送出的 DTMF，例如 `1,w2000,2,#`（wN = 等待 N 毫秒）；需搭配 --rtp
    #[arg(long = "dtmf", value_parser = parse_dtmf, requires = "enable_rtp")]
    pub dtmf: Option<String>,

    /// RTP 起始 port（偶數，預設 10000）
    #[arg(long = "rtp-port", default_value_t = 10000)]
    pub rtp_base_port: u16,

    /// 音檔路徑（自動轉換為 G.711A/PCMA 送出）
    /// 支援：.wav（PCM16/A-law/μ-law）、.ul/.ulaw（raw μ-law）、.al/.alaw（raw A-law）
    #[arg(long = "audio")]
    pub audio_file: Option<std::path::PathBuf>,

    /// 結束後把每通通話的追蹤紀錄（P-CIN-CallId、主叫號碼、時間、MOS）寫成 JSON，
    /// 民眾端與座席端各存一份，再用 --correlate 合併
    #[arg(long = "trace-file")]
    pub trace_file: Option<std::path::PathBuf>,

    /// 端到端對照：讀取兩端的 --trace-file 產生合併報告（不執行壓測，檔案順序不拘）。
    /// 搭配 --format 與 --html 輸出
    #[arg(long = "correlate", num_args = 2, value_names = ["TRACE_A", "TRACE_B"])]
    pub correlate: Option<Vec<std::path::PathBuf>>,

    /// 端到端對照：以主叫號碼配對時，座席收到來電最晚可在民眾端撥出後幾秒（含排隊時間）
    #[arg(long = "correlate-window", default_value_t = 120)]
    pub correlate_window: u64,

    /// 分時段統計的時段長度：`300`、`30s`、`5m`、`1h`（0 = 不分段）。報告會附上每段的 ASR、PDD、MOS、失敗碼
    #[arg(long = "interval", value_parser = parse_interval, default_value = "5m")]
    pub interval: u64,

    /// 結束後把分時段統計寫成 CSV（每段一列）
    #[arg(long = "timeline-csv")]
    pub timeline_csv: Option<std::path::PathBuf>,

    /// 顯示詳細 debug log
    #[arg(long)]
    pub verbose: bool,
}

/// 秒數或秒數區間：`5` → (5, 5)；`2-8` → (2, 8)
fn parse_secs_range(s: &str) -> Result<(f64, f64), String> {
    let num = |v: &str| -> Result<f64, String> {
        let n: f64 = v.trim().parse().map_err(|_| format!("不是數字：{}", v))?;
        if n.is_finite() && n >= 0.0 {
            Ok(n)
        } else {
            Err(format!("需為 ≥ 0 的秒數：{}", v))
        }
    };
    let (min, max) = match s.split_once('-') {
        Some((a, b)) => (num(a)?, num(b)?),
        None => {
            let n = num(s)?;
            (n, n)
        }
    };
    if min > max {
        return Err(format!("區間最小值大於最大值：{}", s));
    }
    Ok((min, max))
}

fn num(v: &str) -> Result<f64, String> {
    let n: f64 = v.trim().parse().map_err(|_| format!("不是數字：{}", v))?;
    if n.is_finite() && n >= 0.0 {
        Ok(n)
    } else {
        Err(format!("需為 ≥ 0 的數字：{}", v))
    }
}

/// `5-50` → 漸增 5 → 50 CPS
fn parse_cps_ramp(s: &str) -> Result<CpsPattern, String> {
    let (a, b) = s
        .split_once('-')
        .ok_or_else(|| format!("格式需為 START-END：{}", s))?;
    let p = CpsPattern::Ramp {
        start: num(a)?,
        end: num(b)?,
    };
    p.validate(1).map_err(|e| e.to_string())?;
    Ok(p)
}

/// `10:30,50:60` → 10 CPS 30 秒、50 CPS 60 秒
fn parse_cps_steps(s: &str) -> Result<CpsPattern, String> {
    let levels = s
        .split(',')
        .map(|part| {
            let (cps, secs) = part
                .split_once(':')
                .ok_or_else(|| format!("每一階格式需為 CPS:秒：{}", part))?;
            let duration_secs = secs
                .trim()
                .parse()
                .map_err(|_| format!("秒數需為整數：{}", secs))?;
            Ok(CpsStep {
                cps: num(cps)?,
                duration_secs,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let p = CpsPattern::Step { levels };
    p.validate(0).map_err(|e| e.to_string())?;
    Ok(p)
}

/// `10,100,50,10` → 平時 10、在 50% 時 100 CPS 持續 10 秒
fn parse_cps_spike(s: &str) -> Result<CpsPattern, String> {
    let v: Vec<&str> = s.split(',').collect();
    let [base, peak, at, secs] = v[..] else {
        return Err(format!("格式需為 基準,尖峰,開始%,尖峰秒數：{}", s));
    };
    let p = CpsPattern::Spike {
        base: num(base)?,
        peak: num(peak)?,
        spike_at_pct: num(at.trim_end_matches('%'))?,
        spike_duration_secs: secs
            .trim()
            .parse()
            .map_err(|_| format!("尖峰秒數需為整數：{}", secs))?,
    };
    p.validate(1).map_err(|e| e.to_string())?;
    Ok(p)
}

/// `3/20` → 每 20 秒一次發出 3 通
fn parse_cps_batch(s: &str) -> Result<CpsPattern, String> {
    let (count, every) = s
        .split_once('/')
        .ok_or_else(|| format!("格式需為 通數/秒數：{}", s))?;
    let p = CpsPattern::Batch {
        count: count
            .trim()
            .parse()
            .map_err(|_| format!("通數需為整數：{}", count))?,
        every_secs: num(every)?,
    };
    p.validate(0).map_err(|e| e.to_string())?;
    Ok(p)
}

/// `300` / `30s` / `5m` / `1h` → 秒
fn parse_interval(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let (num, mul) = match s.char_indices().last() {
        Some((i, 's')) => (&s[..i], 1),
        Some((i, 'm')) => (&s[..i], 60),
        Some((i, 'h')) => (&s[..i], 3600),
        _ => (s, 1),
    };
    num.trim()
        .parse::<u64>()
        .map(|n| n * mul)
        .map_err(|_| format!("格式需為秒數或加上 s / m / h，例如 300、5m、1h：{}", s))
}

fn parse_dtmf(s: &str) -> Result<String, String> {
    sipress_core::rtp::dtmf::validate_dtmf_sequence(s)?;
    Ok(s.trim().to_string())
}

impl Args {
    pub fn mode(&self) -> sipress_core::config::Mode {
        if self.mode == "agent" {
            sipress_core::config::Mode::Agent
        } else {
            sipress_core::config::Mode::Caller
        }
    }

    /// 指定的 CPS 模式（沒有指定 = 固定 --cps）
    pub fn cps_pattern(&self) -> CpsPattern {
        self.cps_ramp
            .clone()
            .or_else(|| self.cps_steps.clone())
            .or_else(|| self.cps_spike.clone())
            .or_else(|| self.cps_batch.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from([&["sipress"], args].concat())
    }

    #[test]
    fn cps_pattern_flags() {
        assert!(matches!(
            parse(&["--cps-ramp", "5-50"]).unwrap().cps_pattern(),
            CpsPattern::Ramp { start, end } if start == 5.0 && end == 50.0
        ));
        match parse(&["--cps-steps", "10:30, 50:60"])
            .unwrap()
            .cps_pattern()
        {
            CpsPattern::Step { levels } => {
                assert_eq!(levels.len(), 2);
                assert_eq!((levels[1].cps, levels[1].duration_secs), (50.0, 60));
            }
            p => panic!("{:?}", p),
        }
        assert!(matches!(
            parse(&["--cps-spike", "10,100,50%,10"]).unwrap().cps_pattern(),
            CpsPattern::Spike { base, peak, spike_at_pct, spike_duration_secs: 10 }
                if base == 10.0 && peak == 100.0 && spike_at_pct == 50.0
        ));
        assert!(matches!(
            parse(&["--cps-batch", "3/20"]).unwrap().cps_pattern(),
            CpsPattern::Batch { count: 3, every_secs } if every_secs == 20.0
        ));
        assert!(matches!(
            parse(&[]).unwrap().cps_pattern(),
            CpsPattern::Flat
        ));
        assert_eq!(parse(&["--refill-delay", "2.5"]).unwrap().refill_delay, 2.5);

        for bad in [
            &["--cps-ramp", "5"][..],
            &["--cps-ramp", "0-0"],
            &["--cps-steps", "10"],
            &["--cps-steps", "0:30"],
            &["--cps-spike", "10,100,50"],
            &["--cps-spike", "10,100,150,5"],
            &["--cps-ramp", "1-5", "--cps-spike", "10,100,50,10"],
            &["--cps-batch", "3"],
            &["--cps-batch", "0/20"],
            &["--cps-batch", "3/0"],
            &["--cps-batch", "3/20", "--cps-ramp", "1-5"],
        ] {
            assert!(parse(bad).is_err(), "{:?} 應被拒絕", bad);
        }
    }

    #[test]
    fn interval_flag() {
        assert_eq!(parse(&[]).unwrap().interval, 300);
        for (v, secs) in [("90", 90), ("30s", 30), ("5m", 300), ("1h", 3600), ("0", 0)] {
            assert_eq!(parse(&["--interval", v]).unwrap().interval, secs, "{}", v);
        }
        for bad in ["", "m", "5x", "-1", "1.5m"] {
            assert!(parse(&["--interval", bad]).is_err(), "{:?} 應被拒絕", bad);
        }
    }

    #[test]
    fn codec_and_dtmf_flags() {
        assert_eq!(parse(&[]).unwrap().codec, "pcma");
        assert_eq!(
            parse(&["--codec", "PCMU"]).unwrap().codec.to_lowercase(),
            "pcmu"
        );
        assert!(parse(&["--codec", "g729"]).is_err());
        let a = parse(&["--rtp", "--dtmf", "1,w500,#"]).unwrap();
        assert_eq!(a.dtmf.as_deref(), Some("1,w500,#"));
        assert!(parse(&["--rtp", "--dtmf", "12"]).is_err(), "格式錯誤");
        assert!(parse(&["--dtmf", "1"]).is_err(), "DTMF 需要 --rtp");
    }

    #[test]
    fn secs_range_parses_single_value_and_range() {
        assert_eq!(parse_secs_range("0"), Ok((0.0, 0.0)));
        assert_eq!(parse_secs_range("2.5"), Ok((2.5, 2.5)));
        assert_eq!(parse_secs_range("2-8"), Ok((2.0, 8.0)));
        for bad in ["", "x", "8-2", "1-", "-1", "inf"] {
            assert!(parse_secs_range(bad).is_err(), "{:?} 應被拒絕", bad);
        }
    }
}
