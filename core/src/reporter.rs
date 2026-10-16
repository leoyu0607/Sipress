/// 報告輸出：Table（終端機）/ JSON / CSV
use crate::stats::FinalReport;

pub struct Reporter;

impl Reporter {
    /// 印出人類可讀的 ASCII table
    pub fn print_table(r: &FinalReport) {
        println!();
        println!("╔══════════════════════════════════════════════════╗");
        println!("║              sipress 壓測報告                    ║");
        println!("╠══════════════════════════════════════════════════╣");
        println!(
            "║  測試時長      {:>10.1} s                      ║",
            r.duration_secs
        );
        println!("╠══════════════════════════════════════════════════╣");
        println!(
            "║  發起通話      {:>10}                          ║",
            r.calls_initiated
        );
        println!(
            "║  接通通話      {:>10}                          ║",
            r.calls_answered
        );
        println!(
            "║  完成通話      {:>10}                          ║",
            r.calls_completed
        );
        println!(
            "║  失敗通話      {:>10}                          ║",
            r.calls_failed
        );
        println!(
            "║  逾時通話      {:>10}                          ║",
            r.calls_timeout
        );
        println!("╠══════════════════════════════════════════════════╣");
        println!("║  ASR           {:>9.2} %                       ║", r.asr);
        println!("║  CCR           {:>9.2} %                       ║", r.ccr);
        println!(
            "║  實際 CPS      {:>10.2}                        ║",
            r.actual_cps
        );
        println!(
            "║  ACD           {:>10.2} s                      ║",
            r.acd_secs
        );
        println!("╠══════════════════════════════════════════════════╣");
        println!(
            "║  PDD p50       {:>9.1} ms                      ║",
            r.pdd_p50_ms
        );
        println!(
            "║  PDD p95       {:>9.1} ms                      ║",
            r.pdd_p95_ms
        );
        println!(
            "║  PDD p99       {:>9.1} ms                      ║",
            r.pdd_p99_ms
        );
        println!(
            "║  PDD max       {:>9.1} ms                      ║",
            r.pdd_max_ms
        );
        println!("╠══════════════════════════════════════════════════╣");
        println!(
            "║  Setup p50     {:>9.1} ms                      ║",
            r.setup_p50_ms
        );
        println!(
            "║  Setup p95     {:>9.1} ms                      ║",
            r.setup_p95_ms
        );
        println!(
            "║  Setup p99     {:>9.1} ms                      ║",
            r.setup_p99_ms
        );
        println!(
            "║  Setup max     {:>9.1} ms                      ║",
            r.setup_max_ms
        );
        println!("╠══════════════════════════════════════════════════╣");
        println!(
            "║  失敗 4xx      {:>10}                          ║",
            r.fail_4xx
        );
        println!(
            "║  失敗 5xx      {:>10}                          ║",
            r.fail_5xx
        );
        println!(
            "║  失敗 6xx      {:>10}                          ║",
            r.fail_6xx
        );
        println!("╚══════════════════════════════════════════════════╝");
        println!();
        Self::print_timeline(r);
        Self::print_agent_dials(r);
    }

    /// 座席流程的撥碼結果（有流程時才印）
    fn print_agent_dials(r: &FinalReport) {
        if r.agent_dials.is_empty() {
            return;
        }
        println!("座席流程撥碼");
        for d in &r.agent_dials {
            let codes: Vec<String> = d
                .fail_codes
                .iter()
                .map(|(c, n)| match c {
                    0 => format!("無回應×{}", n),
                    c => format!("{}×{}", c, n),
                })
                .collect();
            let name = if d.label.is_empty() {
                d.code.clone()
            } else {
                format!("{}（{}）", d.label, d.code)
            };
            println!(
                "  {}  成功 {}  失敗 {}  {}",
                name,
                d.ok,
                d.failed,
                codes.join(" ")
            );
        }
        println!();
    }

    /// 輸出 JSON（適合 pipe 給其他工具）
    /// 分時段統計（兩段以上才印）：找出第幾段開始變差
    fn print_timeline(r: &FinalReport) {
        if r.timeline.len() < 2 {
            return;
        }
        let opt = |v: Option<f64>, prec: usize| {
            v.map_or("—".to_string(), |x| format!("{:.*}", prec, x))
        };
        println!("分時段統計（每段 {:.0} 秒）", r.timeline[0].duration_secs());
        // 中文字佔兩格，標題列直接寫好對齊
        println!("       時段     發起      CPS     ASR%    PDD p50    MOS  失敗碼");
        for b in &r.timeline {
            let codes: Vec<String> = b
                .fail_codes
                .iter()
                .map(|(c, n)| format!("{}×{}", c, n))
                .collect();
            println!(
                "{:>11}  {:>7}  {:>7.2}  {:>7}  {:>9}  {:>5}  {}",
                format!("{}–{}", fmt_clock(b.start_secs), fmt_clock(b.end_secs)),
                b.initiated,
                b.cps(),
                opt(b.asr, 1),
                b.pdd_p50_ms
                    .map_or("—".to_string(), |v| format!("{:.0}ms", v)),
                opt(b.mos, 2),
                codes.join(" "),
            );
        }
        println!();
    }

    pub fn print_json(r: &FinalReport) {
        println!("{}", r.to_json());
    }

    /// 輸出 CSV（適合匯入試算表）
    pub fn print_csv(r: &FinalReport) {
        // Header
        println!(
            "duration_secs,calls_initiated,calls_answered,calls_completed,\
             calls_failed,calls_timeout,asr,ccr,actual_cps,acd_secs,\
             pdd_p50_ms,pdd_p95_ms,pdd_p99_ms,pdd_max_ms,\
             setup_p50_ms,setup_p95_ms,setup_p99_ms,setup_max_ms,\
             fail_4xx,fail_5xx,fail_6xx"
        );
        // Data
        println!(
            "{:.1},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{},{},{}",
            r.duration_secs,
            r.calls_initiated,
            r.calls_answered,
            r.calls_completed,
            r.calls_failed,
            r.calls_timeout,
            r.asr,
            r.ccr,
            r.actual_cps,
            r.acd_secs,
            r.pdd_p50_ms,
            r.pdd_p95_ms,
            r.pdd_p99_ms,
            r.pdd_max_ms,
            r.setup_p50_ms,
            r.setup_p95_ms,
            r.setup_p99_ms,
            r.setup_max_ms,
            r.fail_4xx,
            r.fail_5xx,
            r.fail_6xx,
        );
    }

    /// 依照格式旗標選擇輸出
    pub fn print(r: &FinalReport, format: OutputFormat) {
        match format {
            OutputFormat::Table => Self::print_table(r),
            OutputFormat::Json => Self::print_json(r),
            OutputFormat::Csv => Self::print_csv(r),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormat {
    Table,
    Json,
    Csv,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            other => Err(format!("未知格式: {}", other)),
        }
    }
}

/// 秒數 → `h:mm:ss`（不足一小時為 `m:ss`）
fn fmt_clock(secs: f64) -> String {
    let t = secs.round() as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}
