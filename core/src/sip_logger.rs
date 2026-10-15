/// SIP 完整訊息記錄器
/// 檔名格式：logs/YYYYMMDD_HHMMSS_{role}.sip.log
/// role: agent（壓測發起方）或 user（被叫模擬）
use anyhow::{Context, Result};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

/// 緩衝寫入的最長 flush 間隔（tail -f 觀察 log 時最多延遲這麼久）
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// 緩衝寫入器：每則記錄一次寫入，定期 flush，避免高 CPS 下每則訊息多次 syscall
struct LogWriter {
    out: BufWriter<fs::File>,
    last_flush: Instant,
}

impl LogWriter {
    fn write_record(&mut self, record: &str) {
        let _ = self.out.write_all(record.as_bytes());
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            let _ = self.out.flush();
            self.last_flush = Instant::now();
        }
    }
}

/// 壓測角色（決定 log 檔名，雙端同時執行時兩端各寫各的檔案）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SipRole {
    /// 民眾端（主動撥出）
    Caller,
    /// 座席端（註冊後接聽）
    Agent,
}

impl SipRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Agent => "agent",
        }
    }
}

/// 單次壓測的 SIP log 寫入器
pub struct SipLogger {
    file: Mutex<LogWriter>,
    pub path: PathBuf,
    #[allow(dead_code)]
    role: SipRole,
}

impl SipLogger {
    /// 建立新的 log 檔案
    /// logs_dir：log 目錄（預設 "logs"）
    /// role：agent 或 user
    pub fn new(logs_dir: &str, role: SipRole) -> Result<Self> {
        fs::create_dir_all(logs_dir).with_context(|| format!("無法建立 log 目錄: {}", logs_dir))?;

        let timestamp = Self::timestamp_str();
        let filename = format!("{}_{}.sip.log", timestamp, role.as_str());
        let path = PathBuf::from(logs_dir).join(&filename);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("無法建立 log 檔案: {}", path.display()))?;

        // 寫入 log 標頭
        writeln!(file, "# sipress SIP Log")?;
        writeln!(file, "# 檔案建立時間：{}", timestamp)?;
        writeln!(file, "# 角色：{}", role.as_str())?;
        writeln!(file, "# ----------------------------------------")?;
        writeln!(file)?;

        Ok(Self {
            file: Mutex::new(LogWriter {
                out: BufWriter::with_capacity(64 * 1024, file),
                last_flush: Instant::now(),
            }),
            path,
            role,
        })
    }

    /// 記錄一則 SIP 訊息（帶方向與時間戳記）
    /// direction: "SEND" 或 "RECV"
    pub fn log_message(&self, direction: Direction, msg: &str, peer: &str) {
        let ts = Self::now_str();
        let arrow = match direction {
            Direction::Send => ">>>",
            Direction::Recv => "<<<",
        };

        let header = format!("── {} {} {} [{}] ──", ts, arrow, peer, direction.label());

        // 確保每則訊息之間有空行分隔
        let sep = if msg.ends_with('\n') { "\n" } else { "\n\n" };
        let record = format!("{}\n{}\n{}", header, msg, sep);
        if let Ok(mut f) = self.file.lock() {
            f.write_record(&record);
        }
    }

    /// 記錄引擎事件（非 SIP 原始訊息，例如逾時/錯誤）
    pub fn log_event(&self, call_id: &str, event: &str) {
        let ts = Self::now_str();
        let record = format!("── {} [EVENT] call-id={} → {} ──\n\n", ts, call_id, event);
        if let Ok(mut f) = self.file.lock() {
            f.write_record(&record);
        }
    }

    /// 記錄壓測摘要（寫在 log 尾端）
    pub fn log_summary(&self, summary: &str) {
        if let Ok(mut f) = self.file.lock() {
            let out = &mut f.out;
            let _ = writeln!(out, "# ════════════════════════════════════");
            let _ = writeln!(out, "# 壓測結束摘要");
            let _ = writeln!(out, "# ════════════════════════════════════");
            let _ = writeln!(out, "{}", summary);
            let _ = out.flush();
            f.last_flush = Instant::now();
        }
    }

    /// 檔名用時間戳記 YYYYMMDD_HHMMSS（公開給 CLI 使用）
    pub fn timestamp_for_filename(secs: u64) -> String {
        Self::secs_to_datetime(secs)
    }

    /// 檔名用時間戳記 YYYYMMDD_HHMMSS
    fn timestamp_str() -> String {
        use std::time::UNIX_EPOCH;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        // 手動計算（避免引入 chrono）
        Self::secs_to_datetime(secs)
    }

    /// 行內用時間戳記 HH:MM:SS.mmm
    fn now_str() -> String {
        use std::time::UNIX_EPOCH;
        let dur = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = dur.as_secs();
        let millis = dur.subsec_millis();
        let day_secs = secs % 86400;
        let hh = day_secs / 3600;
        let mm = (day_secs % 3600) / 60;
        let ss = day_secs % 60;
        format!("{:02}:{:02}:{:02}.{:03}", hh, mm, ss, millis)
    }

    /// Unix 秒 → YYYYMMDD_HHMMSS（UTC，不依賴外部 crate）
    fn secs_to_datetime(secs: u64) -> String {
        // 計算日期（Gregorian calendar）
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hh = time_of_day / 3600;
        let mm = (time_of_day % 3600) / 60;
        let ss = time_of_day % 60;

        // 從 1970-01-01 推算年月日
        let mut year = 1970u32;
        let mut remaining = days;
        loop {
            let days_in_year: u64 = if Self::is_leap(year) { 366 } else { 365 };
            if remaining < days_in_year {
                break;
            }
            remaining -= days_in_year;
            year += 1;
        }
        let months = [
            31u64,
            if Self::is_leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut month = 1u32;
        for &days_in_month in &months {
            if remaining < days_in_month {
                break;
            }
            remaining -= days_in_month;
            month += 1;
        }
        let day = remaining + 1;

        format!(
            "{:04}{:02}{:02}_{:02}{:02}{:02}",
            year, month, day, hh, mm, ss
        )
    }

    fn is_leap(year: u32) -> bool {
        (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
    }
}

/// 訊息方向
#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Send,
    Recv,
}

impl Direction {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Send => "→ SERVER",
            Self::Recv => "← SERVER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_and_agent_started_together_write_separate_files() {
        let dir = std::env::temp_dir().join(format!("sipress-logger-test-{}", std::process::id()));
        let dir = dir.to_string_lossy();
        let caller = SipLogger::new(&dir, SipRole::Caller).unwrap();
        let agent = SipLogger::new(&dir, SipRole::Agent).unwrap();
        assert_ne!(caller.path, agent.path, "雙端同一秒啟動不可寫進同一個檔案");
        assert!(caller.path.to_string_lossy().ends_with("_caller.sip.log"));
        assert!(agent.path.to_string_lossy().ends_with("_agent.sip.log"));
    }

    #[test]
    fn test_timestamp_format() {
        let ts = SipLogger::secs_to_datetime(0);
        assert_eq!(ts, "19700101_000000");

        // 2024-05-01 14:30:22 UTC = 1714573822
        let ts2 = SipLogger::secs_to_datetime(1714573822);
        assert_eq!(ts2, "20240501_143022");
    }

    #[test]
    fn records_are_complete_after_summary_flush() {
        // 每個測試用自己的目錄：檔名只到秒，同一秒建立的同角色 logger 會寫到同一個檔案
        let dir =
            std::env::temp_dir().join(format!("sipress-logger-flush-test-{}", std::process::id()));
        let log = SipLogger::new(&dir.to_string_lossy(), SipRole::Agent).unwrap();
        log.log_message(
            Direction::Send,
            "INVITE sip:a@b SIP/2.0\r\nCall-ID: x\r\n\r\n",
            "1.2.3.4:5060",
        );
        log.log_message(Direction::Recv, "SIP/2.0 200 OK", "1.2.3.4:5060");
        log.log_event("x", "TIMEOUT");
        log.log_summary("done");

        // 不 drop logger：摘要寫入時必須已 flush 到檔案
        let content = fs::read_to_string(&log.path).unwrap();
        // 格式與改用緩衝寫入前相同：header、訊息、（不以換行結尾時補一行）、空行
        assert!(content.contains(
            ">>> 1.2.3.4:5060 [→ SERVER] ──\nINVITE sip:a@b SIP/2.0\r\nCall-ID: x\r\n\r\n\n\n── "
        ));
        assert!(content.contains("<<< 1.2.3.4:5060 [← SERVER] ──\nSIP/2.0 200 OK\n\n\n── "));
        assert!(content.contains("[EVENT] call-id=x → TIMEOUT ──\n\n"));
        assert!(content.trim_end().ends_with("done"));
        let _ = fs::remove_dir_all(&dir);
    }
}
