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
    }

    /// 輸出 JSON（適合 pipe 給其他工具）
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
