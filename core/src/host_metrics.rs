//! 主機監控資料：讀主機 agent 主控台「匯出分析資料 (ZIP)」或單台主機的 `metrics.csv`
//!
//! 格式見主機 agent 的匯出文件：`manifest.json`（主機規格）+ `metrics.csv`（10 秒一列，
//! 欄位 `<主機>_<指標>`，空白 = 沒有資料）。只取整合報告用到的指標：
//! `host_cpu`、`host_mem_pct`、`disk_queue_<磁碟>`、`disk_util_<磁碟>`（新版 agent 才有）。
//!
//! 檔案來自使用者，可能是錯的或惡意的：
//! - ZIP 只把兩個檔案讀進記憶體，不解壓到磁碟（路徑穿越、符號連結、執行檔都碰不到）
//! - 讀取時限制實際讀到的大小（不信任 ZIP 標頭寫的大小），擋壓縮炸彈與超大檔案
//! - 限制主機、磁碟、欄位、列數；不合理的數值（NaN、超出範圍）當缺值並計數
//! - 文字欄位放進 HTML 前由整合報告跳脫

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// 讀取上限：檔案本身與解壓後的內容（超過就拒絕，不截斷）
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_CSV_BYTES: u64 = 256 * 1024 * 1024;
/// 數量上限：一份匯出的主機數、每台主機的磁碟數、CSV 欄位數與資料列數
const MAX_HOSTS: usize = 50;
const MAX_DISKS_PER_HOST: usize = 32;
const MAX_COLUMNS: usize = 5_000;
const MAX_ROWS: usize = 1_000_000;
/// 支援的匯出格式版本
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default)]
pub struct HostMetrics {
    pub hosts: Vec<Host>,
}

#[derive(Debug, Clone, Default)]
pub struct Host {
    /// 主控台登錄的主機名稱
    pub name: String,
    pub agent_version: String,
    /// 主機規格（舊版 agent 或只有 CSV 時為 None）
    pub system: Option<HostSystem>,
    /// 取得規格或效能紀錄失敗的原因
    pub errors: Vec<String>,
    pub samples: Vec<HostSample>,
    /// 不合理而略過的數值個數（NaN、無限大、超出範圍）
    pub invalid_values: u64,
}

#[derive(Debug, Clone, Default)]
pub struct HostSample {
    /// Unix 秒
    pub time: i64,
    pub cpu_pct: Option<f64>,
    pub mem_pct: Option<f64>,
    /// 磁碟名稱 → 值（沒有資料的時間點不放）
    pub disk_queue: BTreeMap<String, f64>,
    pub disk_util: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HostSystem {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    pub arch: String,
    #[serde(rename = "virtual")]
    pub virt: String,
    pub cpu_model: String,
    pub cores: u64,
    pub physical_cores: u64,
    pub mem_total: u64,
    pub swap_total: u64,
    pub disks: Vec<DiskInfo>,
    pub filesystems: Vec<Filesystem>,
    pub collected_at: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiskInfo {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Filesystem {
    pub mount: String,
    pub device: String,
    pub fstype: String,
    pub total: u64,
    pub used: u64,
    pub used_pct: f64,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    format_version: Option<u32>,
    #[serde(default)]
    hosts: Vec<ManifestHost>,
}

#[derive(Deserialize)]
struct ManifestHost {
    name: String,
    #[serde(default)]
    agent_version: String,
    #[serde(default)]
    system: Option<HostSystem>,
    #[serde(default)]
    errors: Vec<String>,
}

impl HostMetrics {
    /// 讀 ZIP（含 manifest.json）或單台主機的 metrics.csv（欄位沒有主機前綴）
    pub fn load(path: &Path) -> Result<Self> {
        let size = std::fs::metadata(path)
            .with_context(|| format!("無法讀取 {}", path.display()))?
            .len();
        if size > MAX_FILE_BYTES {
            bail!(
                "{} 太大（{} MB，上限 {} MB）",
                path.display(),
                size / 1024 / 1024,
                MAX_FILE_BYTES / 1024 / 1024
            );
        }
        let file =
            std::fs::File::open(path).with_context(|| format!("無法讀取 {}", path.display()))?;
        let bytes = read_limited(file, MAX_FILE_BYTES, "檔案")?;
        if bytes.starts_with(b"PK") {
            Self::from_zip(&bytes).with_context(|| format!("{} 格式錯誤", path.display()))
        } else {
            let name = path
                .file_stem()
                .map_or("host".to_string(), |s| s.to_string_lossy().into_owned());
            Self::from_single_csv(&String::from_utf8_lossy(&bytes), &name)
                .with_context(|| format!("{} 格式錯誤", path.display()))
        }
    }

    fn from_zip(bytes: &[u8]) -> Result<Self> {
        let mut zip =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("不是有效的 ZIP 檔")?;
        // 只讀這兩個檔案，其他內容一律忽略；不解壓到磁碟
        let mut read = |name: &str, limit: u64| -> Result<String> {
            let f = zip.by_name(name).with_context(|| {
                format!("ZIP 內沒有 {}（請用主機 agent 的「匯出分析資料」）", name)
            })?;
            if f.size() > limit {
                bail!("{} 太大（解壓後 {} bytes，上限 {}）", name, f.size(), limit);
            }
            let bytes = read_limited(f, limit, name)?;
            String::from_utf8(bytes).with_context(|| format!("{} 不是 UTF-8 文字", name))
        };
        let manifest: Manifest = serde_json::from_str(&read("manifest.json", MAX_MANIFEST_BYTES)?)
            .context("manifest.json 格式錯誤")?;
        if let Some(v) = manifest.format_version.filter(|v| *v > FORMAT_VERSION) {
            bail!(
                "匯出格式版本 {} 比這個 sipress 支援的 {} 新，請更新 sipress",
                v,
                FORMAT_VERSION
            );
        }
        if manifest.hosts.len() > MAX_HOSTS {
            bail!(
                "主機太多（{} 台，上限 {} 台），請在主控台只勾選受測主機再匯出",
                manifest.hosts.len(),
                MAX_HOSTS
            );
        }
        let csv = read("metrics.csv", MAX_CSV_BYTES)?;
        let mut hosts: Vec<Host> = manifest
            .hosts
            .into_iter()
            .map(|h| Host {
                name: h.name,
                agent_version: h.agent_version,
                system: h.system,
                errors: h.errors,
                ..Default::default()
            })
            .collect();
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        let mut by_host = parse_csv(&csv, |col| split_prefixed(col, &names))?;
        for h in &mut hosts {
            (h.samples, h.invalid_values) = by_host.remove(&h.name).unwrap_or_default();
        }
        Ok(Self { hosts })
    }

    fn from_single_csv(csv: &str, name: &str) -> Result<Self> {
        let mut by_host = parse_csv(csv, |col| Some((String::new(), col.to_string())))?;
        let (samples, invalid_values) = by_host.remove("").unwrap_or_default();
        Ok(Self {
            hosts: vec![Host {
                name: name.to_string(),
                samples,
                invalid_values,
                ..Default::default()
            }],
        })
    }
}

/// 最多讀 `limit` bytes；實際內容超過就拒絕（ZIP 標頭寫的大小可以造假，所以邊讀邊算）
fn read_limited(r: impl Read, limit: u64, what: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(limit + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > limit {
        bail!("{} 太大（超過 {} MB）", what, limit / 1024 / 1024);
    }
    Ok(buf)
}

/// 數值的合理範圍；超出範圍視為錯誤資料
fn valid(field_is_pct: bool, v: f64) -> bool {
    v.is_finite() && v >= 0.0 && if field_is_pct { v <= 100.0 } else { v < 1e6 }
}

/// 錯誤訊息中引用的內容只取開頭，避免超長的一列塞滿訊息
fn snippet(s: &str) -> String {
    let mut out: String = s.chars().take(80).collect();
    if out.len() < s.len() {
        out.push('…');
    }
    out
}

/// `<主機>_<指標>` → (主機, 指標)；主機名稱本身可能含 `_`，所以用最長的已知名稱比對前綴
fn split_prefixed(col: &str, names: &[String]) -> Option<(String, String)> {
    names
        .iter()
        .filter(|n| col.len() > n.len() + 1 && col.starts_with(n.as_str()))
        .filter(|n| col.as_bytes()[n.len()] == b'_')
        .max_by_key(|n| n.len())
        .map(|n| (n.clone(), col[n.len() + 1..].to_string()))
}

/// 每台主機的（樣本, 略過的不合理數值個數）
type ParsedHosts = BTreeMap<String, (Vec<HostSample>, u64)>;

fn parse_csv(csv: &str, split: impl Fn(&str) -> Option<(String, String)>) -> Result<ParsedHosts> {
    let mut lines = csv.trim_start_matches('\u{feff}').lines();
    let header: Vec<&str> = lines
        .next()
        .context("metrics.csv 是空的")?
        .split(',')
        .collect();
    if header.len() > MAX_COLUMNS {
        bail!(
            "metrics.csv 欄位太多（{} 欄，上限 {} 欄）",
            header.len(),
            MAX_COLUMNS
        );
    }
    let time_col = header
        .iter()
        .position(|h| h.trim() == "time")
        .context("metrics.csv 沒有 time 欄位")?;
    enum Field {
        Cpu,
        Mem,
        Queue(String),
        Util(String),
    }
    // 欄位索引 → (主機, 指標)
    let cols: Vec<(usize, String, Field)> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let (host, metric) = split(h.trim())?;
            let field = match metric.as_str() {
                "host_cpu" => Field::Cpu,
                "host_mem_pct" => Field::Mem,
                m => {
                    if let Some(d) = m.strip_prefix("disk_queue_") {
                        Field::Queue(d.to_string())
                    } else if let Some(d) = m.strip_prefix("disk_util_") {
                        Field::Util(d.to_string())
                    } else {
                        return None;
                    }
                }
            };
            Some((i, host, field))
        })
        .collect();
    if cols.is_empty() {
        bail!("metrics.csv 沒有 host_cpu / host_mem_pct / disk_queue_* 欄位");
    }
    let mut disks: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
    for (_, host, field) in &cols {
        if let Field::Queue(d) | Field::Util(d) = field {
            disks.entry(host).or_default().insert(d);
        }
    }
    if let Some((host, d)) = disks.iter().find(|(_, d)| d.len() > MAX_DISKS_PER_HOST) {
        bail!(
            "主機 {} 的磁碟太多（{} 顆，上限 {} 顆）",
            snippet(host),
            d.len(),
            MAX_DISKS_PER_HOST
        );
    }

    let mut out = ParsedHosts::new();
    for (n, line) in lines.filter(|l| !l.trim().is_empty()).enumerate() {
        if n >= MAX_ROWS {
            bail!(
                "metrics.csv 資料太多（超過 {} 列），請縮短匯出的時間範圍",
                MAX_ROWS
            );
        }
        let cells: Vec<&str> = line.split(',').collect();
        let time = parse_rfc3339(cells.get(time_col).copied().unwrap_or(""))
            .with_context(|| format!("時間格式錯誤：{}", snippet(line)))?;
        let mut row: BTreeMap<&str, HostSample> = BTreeMap::new();
        for (i, host, field) in &cols {
            let Some(v) = cells
                .get(*i)
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .and_then(|c| c.parse::<f64>().ok())
            else {
                continue;
            };
            if !valid(matches!(field, Field::Cpu | Field::Mem | Field::Util(_)), v) {
                out.entry(host.clone()).or_default().1 += 1;
                continue;
            }
            let s = row.entry(host.as_str()).or_insert_with(|| HostSample {
                time,
                ..Default::default()
            });
            match field {
                Field::Cpu => s.cpu_pct = Some(v),
                Field::Mem => s.mem_pct = Some(v),
                Field::Queue(d) => {
                    s.disk_queue.insert(d.clone(), v);
                }
                Field::Util(d) => {
                    s.disk_util.insert(d.clone(), v);
                }
            }
        }
        for (host, s) in row {
            out.entry(host.to_string()).or_default().0.push(s);
        }
    }
    Ok(out)
}

/// RFC 3339（`2026-10-28T12:48:20+08:00`、`...Z`，秒以下忽略）→ Unix 秒
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if s.get(4..5)? != "-" || !matches!(s.get(10..11)?, "T" | "t" | " ") {
        return None;
    }
    let rest = s.get(19..)?;
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    // 公曆日期 → 1970-01-01 起的天數（Howard Hinnant 的 days_from_civil）
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const MANIFEST: &str = r#"{"format_version":1,"hosts":[
      {"name":"qs-prod-01","agent_version":"v1.8.2","system":{"hostname":"qs-prod-01","os":"Rocky Linux 9.4","cores":8,
       "mem_total":34359738368,"disks":[{"name":"sda","size":214748364800}],"filesystems":[]}},
      {"name":"qs","agent_version":"v1.7.0","errors":["效能紀錄:agent 版本不支援效能紀錄 (請升級)"]}]}"#;

    const CSV: &str = "\u{feff}time,qs-prod-01_host_cpu,qs-prod-01_host_mem_pct,qs-prod-01_load1,qs-prod-01_disk_queue_sda,qs-prod-01_disk_util_sda,qs-prod-01_AI_Engine.v2_cpu
2026-10-28T12:48:20+08:00,31.2,40.5,0.5,0.61,12.5,3
2026-10-28T12:48:30+08:00,,,,,,
2026-10-28T12:48:40+08:00,52,41,0.6,7.02,88,4
";

    fn zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, body) in files {
            w.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        buf.into_inner()
    }

    #[test]
    fn reads_agent_export_zip() {
        let m = HostMetrics::from_zip(&zip(&[("manifest.json", MANIFEST), ("metrics.csv", CSV)]))
            .unwrap();
        assert_eq!(m.hosts.len(), 2);
        let h = &m.hosts[0];
        assert_eq!(h.system.as_ref().unwrap().disks[0].size, 214748364800);
        assert_eq!(h.samples.len(), 2, "空白列 = 沒有資料，不是 0");
        assert_eq!(
            h.samples[0].time,
            parse_rfc3339("2026-10-28T04:48:20Z").unwrap()
        );
        assert_eq!(h.samples[0].cpu_pct, Some(31.2));
        assert_eq!(h.samples[1].disk_queue["sda"], 7.02);
        assert_eq!(h.samples[1].disk_util["sda"], 88.0);
        // 服務指標（含 _ 的服務名稱）不會被當成主機指標
        assert!(m.hosts[1].samples.is_empty());
        assert_eq!(m.hosts[1].errors.len(), 1);
    }

    #[test]
    fn reads_single_host_csv() {
        let csv =
            "time,host_cpu,host_mem_pct,disk_queue_nvme0n1\n2026-10-28T12:48:20+08:00,10,20,0.5\n";
        let m = HostMetrics::from_single_csv(csv, "web1").unwrap();
        assert_eq!(m.hosts[0].name, "web1");
        assert_eq!(m.hosts[0].samples[0].disk_queue["nvme0n1"], 0.5);
        assert!(HostMetrics::from_single_csv("time,foo\n", "x").is_err());
    }

    #[test]
    fn prefix_prefers_longest_host_name() {
        let names = vec!["qs".to_string(), "qs_prod".to_string()];
        assert_eq!(
            split_prefixed("qs_prod_host_cpu", &names),
            Some(("qs_prod".into(), "host_cpu".into()))
        );
        assert_eq!(
            split_prefixed("qs_host_cpu", &names),
            Some(("qs".into(), "host_cpu".into()))
        );
        assert_eq!(split_prefixed("other_host_cpu", &names), None);
    }

    #[test]
    fn parses_rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2024-05-01T14:30:22Z"), Some(1714573822));
        assert_eq!(parse_rfc3339("2024-05-01T22:30:22+08:00"), Some(1714573822));
        assert_eq!(
            parse_rfc3339("2024-05-01T09:30:22.123-05:00"),
            Some(1714573822)
        );
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1709164800));
        assert_eq!(parse_rfc3339("not a time"), None);
    }

    #[test]
    fn rejects_oversized_content_without_reading_it_all() {
        // 讀取上限以實際讀到的內容為準
        assert!(read_limited(&[0u8; 10][..], 10, "x").is_ok());
        assert!(read_limited(&[0u8; 11][..], 10, "x").is_err());
        // manifest.json 解壓後 2 MB（超過 1 MB），壓縮後只有幾 KB
        let padded = format!(r#"{{"hosts":[]{}}}"#, " ".repeat(2 * 1024 * 1024));
        let z = zip(&[("manifest.json", &padded), ("metrics.csv", CSV)]);
        assert!(z.len() < 64 * 1024);
        let err = HostMetrics::from_zip(&z).unwrap_err().to_string();
        assert!(err.contains("manifest.json 太大"), "{}", err);
    }

    #[test]
    fn rejects_unexpected_structure() {
        let err =
            |files: &[(&str, &str)]| HostMetrics::from_zip(&zip(files)).unwrap_err().to_string();
        assert!(err(&[("metrics.csv", CSV)]).contains("ZIP 內沒有 manifest.json"));
        assert!(err(&[
            ("manifest.json", r#"{"format_version":2,"hosts":[]}"#),
            ("metrics.csv", CSV)
        ])
        .contains("格式版本 2"));
        let many_hosts = format!(
            r#"{{"hosts":[{}]}}"#,
            (0..51)
                .map(|i| format!(r#"{{"name":"h{}"}}"#, i))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(err(&[("manifest.json", &many_hosts), ("metrics.csv", CSV)]).contains("主機太多"));
        assert!(HostMetrics::from_zip(b"PK\x03\x04 not a zip").is_err());

        let wide = format!(
            "time,{}\n",
            (0..5001)
                .map(|i| format!("c{}", i))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(HostMetrics::from_single_csv(&wide, "h")
            .unwrap_err()
            .to_string()
            .contains("欄位太多"));
        let disks = format!(
            "time,{}\n",
            (0..33)
                .map(|i| format!("disk_queue_d{}", i))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(HostMetrics::from_single_csv(&disks, "h")
            .unwrap_err()
            .to_string()
            .contains("磁碟太多"));
        let long_line = format!("time,host_cpu\n{},1\n", "x".repeat(10_000));
        let msg = HostMetrics::from_single_csv(&long_line, "h")
            .unwrap_err()
            .to_string();
        assert!(msg.len() < 400, "錯誤訊息不應包含整列內容");
    }

    #[test]
    fn skips_unreasonable_values() {
        let csv = "time,host_cpu,host_mem_pct,disk_queue_sda,disk_util_sda
2026-10-28T12:48:20+08:00,NaN,inf,-1,101
2026-10-28T12:48:30+08:00,150,50,3.5,40
2026-10-28T12:48:40+08:00,,,,
";
        let m = HostMetrics::from_single_csv(csv, "h").unwrap();
        let h = &m.hosts[0];
        assert_eq!(h.invalid_values, 5);
        assert_eq!(h.samples.len(), 1, "第一列全部不合理、第三列全空");
        let s = &h.samples[0];
        assert_eq!((s.cpu_pct, s.mem_pct), (None, Some(50.0)));
        assert_eq!((s.disk_queue["sda"], s.disk_util["sda"]), (3.5, 40.0));
    }
}
