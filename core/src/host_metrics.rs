//! 主機監控資料：讀主機 agent 主控台「匯出分析資料 (ZIP)」或單台主機的 `metrics.csv`
//!
//! 格式見主機 agent 的匯出文件：`manifest.json`（主機規格）+ `metrics.csv`（10 秒一列，
//! 欄位 `<主機>_<指標>`，空白 = 沒有資料）。只取整合報告用到的指標：
//! `host_cpu`、`host_mem_pct`、`disk_queue_<磁碟>`、`disk_util_<磁碟>`（新版 agent 才有）。

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

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
        let bytes = std::fs::read(path).with_context(|| format!("無法讀取 {}", path.display()))?;
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
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        let mut read = |name: &str| -> Result<String> {
            let mut f = zip
                .by_name(name)
                .with_context(|| format!("ZIP 內沒有 {}", name))?;
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Ok(s)
        };
        let manifest: Manifest = serde_json::from_str(&read("manifest.json")?)?;
        let csv = read("metrics.csv")?;
        let mut hosts: Vec<Host> = manifest
            .hosts
            .into_iter()
            .map(|h| Host {
                name: h.name,
                agent_version: h.agent_version,
                system: h.system,
                errors: h.errors,
                samples: Vec::new(),
            })
            .collect();
        let names: Vec<String> = hosts.iter().map(|h| h.name.clone()).collect();
        let mut by_host = parse_csv(&csv, |col| split_prefixed(col, &names))?;
        for h in &mut hosts {
            h.samples = by_host.remove(&h.name).unwrap_or_default();
        }
        Ok(Self { hosts })
    }

    fn from_single_csv(csv: &str, name: &str) -> Result<Self> {
        let mut by_host = parse_csv(csv, |col| Some((String::new(), col.to_string())))?;
        Ok(Self {
            hosts: vec![Host {
                name: name.to_string(),
                samples: by_host.remove("").unwrap_or_default(),
                ..Default::default()
            }],
        })
    }
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

fn parse_csv(
    csv: &str,
    split: impl Fn(&str) -> Option<(String, String)>,
) -> Result<BTreeMap<String, Vec<HostSample>>> {
    let mut lines = csv.trim_start_matches('\u{feff}').lines();
    let header: Vec<&str> = lines
        .next()
        .context("metrics.csv 是空的")?
        .split(',')
        .collect();
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

    let mut out: BTreeMap<String, Vec<HostSample>> = BTreeMap::new();
    for line in lines.filter(|l| !l.trim().is_empty()) {
        let cells: Vec<&str> = line.split(',').collect();
        let time = parse_rfc3339(cells.get(time_col).copied().unwrap_or(""))
            .with_context(|| format!("時間格式錯誤：{}", line))?;
        let mut row: BTreeMap<&str, HostSample> = BTreeMap::new();
        for (i, host, field) in &cols {
            let Some(v) = cells.get(*i).and_then(|c| c.trim().parse::<f64>().ok()) else {
                continue;
            };
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
            out.entry(host.to_string()).or_default().push(s);
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
}
