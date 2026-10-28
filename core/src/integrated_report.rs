//! 整合壓測報告：讀壓測數據檔（`<時間>_result.json`），依門檻產生交給客戶的 HTML 報告
//!
//! 版面以 A4 列印為準（瀏覽器「列印 → 另存 PDF」即可輸出 PDF）。圖表用 10 秒時間軸，
//! 點數過多時合併相鄰時段；合併時品質指標取較差的值（MOS 取低、掉包與 Jitter 取高）。

use crate::correlation_html::esc;
use crate::host_metrics::{parse_rfc3339, Host, HostMetrics, HostSample};
use crate::result_file::ResultFile;
use crate::rtp::stats::CallQuality;
use crate::thresholds::Thresholds;
use crate::timeline::TimelineBucket;
use anyhow::Result;
use serde_json::Value;
use std::fmt::Write as _;
use std::{fs, path::PathBuf};

/// 每張圖最多的點數
const MAX_POINTS: usize = 300;

// 配色參考監控工具（Zabbix / VoIPmonitor）：主要指標用填色面積，其他線用高對比色
const C_GREEN: &str = "#16a34a";
const C_BLUE: &str = "#1d4ed8";
const C_RED: &str = "#dc2626";
const C_ORANGE: &str = "#ea580c";
const C_GRAY: &str = "#6b7280";
const C_INK: &str = "#111827";
const C_AVG: &str = "#374151";
const C_LIMIT: &str = "#f59e0b";

pub struct IntegratedReport;

impl IntegratedReport {
    /// 寫到 `<dir>/<timestamp>_integrated_report.html`
    pub fn save(
        file: &ResultFile,
        th: &Thresholds,
        hosts: Option<&HostMetrics>,
        output_dir: &str,
        timestamp: &str,
    ) -> Result<PathBuf> {
        fs::create_dir_all(output_dir)?;
        let path = PathBuf::from(output_dir).join(format!("{}_integrated_report.html", timestamp));
        fs::write(&path, Self::render(file, th, hosts))?;
        Ok(path)
    }

    /// `hosts` = 主機 agent 匯出的效能資料（沒有時不列主機資源）
    pub fn render(file: &ResultFile, th: &Thresholds, hosts: Option<&HostMetrics>) -> String {
        let r = &file.report;
        let points = merge(&r.fine_timeline);
        let hosts: Vec<HostView> = hosts.map_or(Vec::new(), |h| host_views(file, h));
        let mut checks = checks(file, th);
        for h in &hosts {
            checks.extend(h.checks(th));
        }
        let mut out = String::with_capacity(128 * 1024);
        let _ = write!(
            out,
            r#"<!DOCTYPE html>
<html lang="zh-Hant">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SIP 壓力測試報告</title>
<style>{CSS}</style>
</head>
<body>
<main class="page">
<header class="cover">
  <div class="brand">sipress</div>
  <h1>SIP 壓力測試報告</h1>
  <div class="meta">伺服器 <strong>{server}</strong> · 測試開始 <time class="local" data-utc="{start}">{start_txt}</time></div>
</header>
"#,
            server = esc(&cfg_str(&file.config, "server_addr")),
            start = esc(&r.started_at),
            start_txt = if r.started_at.is_empty() {
                "—".to_string()
            } else {
                esc(&r.started_at.replace('T', " ").replace('Z', " UTC"))
            },
        );

        // ── 1. 結論 ──
        let verdict = overall(&checks);
        let _ = write!(
            out,
            r#"<section class="card">
<h2>1. 結論</h2>
<div class="verdict {cls}">{label}</div>
{summary}
<table class="tbl checks">
<thead><tr><th>項目</th><th>結果</th><th>門檻</th><th>判定</th></tr></thead>
<tbody>{rows}</tbody>
</table>
</section>
"#,
            cls = verdict.cls(),
            label = verdict.label(),
            summary = summary_text(file, th, &checks, &points),
            rows = checks.iter().map(Check::row).collect::<String>(),
        );

        // ── 2. 測試條件 ──
        let _ = write!(
            out,
            r#"<section class="card">
<h2>2. 測試條件</h2>
<table class="tbl kv"><tbody>{rows}</tbody></table>
</section>
"#,
            rows = conditions(file)
                .iter()
                .map(|(k, v)| format!("<tr><th>{}</th><td>{}</td></tr>", k, v))
                .collect::<String>(),
        );

        // ── 3. 通話量 ──
        let x: Vec<f64> = points.iter().map(|p| p.t).collect();
        let bucket = points.first().map_or(10.0, |p| p.span);
        let _ = write!(
            out,
            r#"<section class="card">
<h2>3. 通話量</h2>
<p class="hint">每 {bucket} 一點；橫軸為測試開始後的時間（開啟報告時會換成當地時間）。</p>
{c1}{c2}{c3}
</section>
"#,
            bucket = duration_label(bucket),
            c1 = Chart {
                title: "並發通話數".into(),
                unit: "通",
                y_min: 0.0,
                y_max: None,
                series: vec![Series::new(
                    "時段結束時進行中的通話",
                    C_GREEN,
                    points.iter().map(|p| Some(p.active)).collect()
                )
                .area()],
                limit: None,
                right: None,
                summary: None,
            }
            .svg(&x),
            c2 = Chart {
                title: "每秒發起通話數（CPS）".into(),
                unit: "",
                y_min: 0.0,
                y_max: None,
                series: vec![Series::new(
                    "CPS",
                    C_BLUE,
                    points.iter().map(|p| Some(p.cps)).collect()
                )
                .area()],
                limit: None,
                right: None,
                summary: None,
            }
            .svg(&x),
            c3 = Chart {
                title: "接通率（ASR）".into(),
                unit: "%",
                y_min: 0.0,
                y_max: Some(100.0),
                series: vec![Series::new(
                    "本時段接通 ÷ 有結果的通話",
                    C_GREEN,
                    points.iter().map(|p| p.asr).collect()
                )
                .area()],
                limit: None,
                right: None,
                summary: None,
            }
            .svg(&x),
        );

        // ── 4. 聲音品質 ──
        if points.iter().any(|p| p.rtp.is_some()) {
            let rtp = |f: fn(&RtpPoint) -> f64| -> Vec<Option<f64>> {
                points.iter().map(|p| p.rtp.as_ref().map(f)).collect()
            };
            let _ = write!(
                out,
                r#"<section class="card">
<h2>4. 聲音品質</h2>
<p class="hint">每個時段只計算該時段內新增的封包；紅色底色為超過門檻的時段，橘黃色虛線為門檻，灰色點線為平均；MOS 圖的黑色虛線為並發通話數（右軸）。</p>
{c1}{c2}{c3}
</section>
"#,
                c1 = Chart {
                    title: "MOS".into(),
                    unit: "",
                    y_min: 1.0,
                    y_max: Some(5.0),
                    series: vec![
                        Series::new(
                            "MOS（jitter buffer 200 ms）平均",
                            C_BLUE,
                            rtp(|r| r.mos_jb200_avg)
                        ),
                        Series::new(
                            "MOS（jitter buffer 200 ms）P5",
                            C_RED,
                            rtp(|r| r.mos_jb200_p5)
                        ),
                        Series::new("MOS（E-Model）平均", C_GRAY, rtp(|r| r.mos_avg)),
                    ],
                    limit: Some(Limit::Min(th.mos)),
                    right: Some(RightAxis {
                        unit: "通",
                        y_max: None,
                        series: vec![Series::new(
                            "並發通話數",
                            C_INK,
                            points.iter().map(|p| Some(p.active)).collect(),
                        )
                        .dashed()],
                    }),
                    summary: None,
                }
                .svg(&x),
                c2 = Chart {
                    title: "掉包率".into(),
                    unit: "%",
                    y_min: 0.0,
                    y_max: None,
                    series: vec![
                        Series::new("整體（時段內所有通話）", C_GREEN, rtp(|r| r.loss_pct)).area(),
                        Series::new("最嚴重的單通電話", C_ORANGE, rtp(|r| r.loss_max_pct)),
                    ],
                    limit: Some(Limit::Max(th.loss_pct)),
                    right: None,
                    summary: None,
                }
                .svg(&x),
                c3 = Chart {
                    title: "Jitter".into(),
                    unit: "ms",
                    y_min: 0.0,
                    y_max: None,
                    series: vec![
                        Series::new("平均", C_GREEN, rtp(|r| r.jitter_avg_ms)).area(),
                        Series::new("每通電話的 P95", C_ORANGE, rtp(|r| r.jitter_p95_ms)),
                    ],
                    limit: Some(Limit::Max(th.jitter_ms)),
                    right: None,
                    summary: None,
                }
                .svg(&x),
            );
        } else {
            out.push_str(
                r#"<section class="card"><h2>4. 聲音品質</h2><p class="hint">這次測試未啟用 RTP，沒有聲音品質數據。</p></section>
"#,
            );
        }

        // ── 5. 主機資源 ──
        if !hosts.is_empty() {
            out.push_str(
                r#"<section class="card">
<h2>5. 受測主機資源</h2>
<p class="hint">資料來源：主機 agent 匯出的效能紀錄（10 秒取樣），只取測試期間的資料。判定看 10 秒取樣的 P95；10 秒的短暫尖峰沒有被平均掉，最高值通常比 1 分鐘取樣的監控工具（例如 Zabbix）高。</p>
"#,
            );
            for h in &hosts {
                out.push_str(&h.section(th));
            }
            out.push_str("</section>\n");
        }

        let _ = write!(
            out,
            r#"<section class="card defs">
<h2>{n}. 指標說明</h2>
{DEFINITIONS}
</section>
</main>
<footer class="footer">sipress v{ver}（數據由 v{src} 產生）</footer>
<script>{SCRIPT}</script>
</body></html>"#,
            n = if hosts.is_empty() { 5 } else { 6 },
            ver = env!("CARGO_PKG_VERSION"),
            src = esc(&file.sipress_version),
        );
        out
    }
}

// ── 判定 ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Verdict {
    Pass,
    Fail,
    /// 沒有數據可判定
    NoData,
}

impl Verdict {
    fn cls(self) -> &'static str {
        match self {
            Self::Pass => "good",
            Self::Fail => "bad",
            Self::NoData => "info",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "通過",
            Self::Fail => "未通過",
            Self::NoData => "無法判定",
        }
    }
}

struct Check {
    name: String,
    value: Option<f64>,
    unit: &'static str,
    limit: Limit,
    note: String,
}

impl Check {
    fn verdict(&self) -> Verdict {
        match self.value {
            None => Verdict::NoData,
            Some(v) if self.limit.ok(v) => Verdict::Pass,
            Some(_) => Verdict::Fail,
        }
    }

    fn row(&self) -> String {
        let v = self.verdict();
        let value = self.value.map_or("—".to_string(), |x| {
            format!("{}{}", fmt_num(x), self.unit)
        });
        format!(
            r#"<tr><td>{name}</td><td class="num">{value}<div class="sub">{note}</div></td><td class="num">{limit}{unit}</td><td><span class="tag {cls}">{label}</span></td></tr>"#,
            name = self.name,
            note = self.note,
            limit = self.limit.label(),
            unit = self.unit,
            cls = v.cls(),
            label = v.label(),
        )
    }
}

fn checks(file: &ResultFile, th: &Thresholds) -> Vec<Check> {
    let r = &file.report;
    let q = r.call_quality.as_ref();
    let mos_p5 = q.and_then(|q| CallQuality::quantile(&q.mos_jb200_hist, 0.05));
    let mos_note = q.map_or(String::new(), |q| {
        let ok = q.calls - CallQuality::count_below(&q.mos_jb200_hist, th.mos);
        format!(
            "每通電話的 P5；{} 通中 {}（{:.1}%）達標",
            q.calls,
            ok,
            ok as f64 / q.calls as f64 * 100.0
        )
    });
    vec![
        Check {
            name: "MOS（jitter buffer 200 ms 模擬）".into(),
            value: mos_p5,
            unit: "",
            limit: Limit::Min(th.mos),
            note: mos_note,
        },
        Check {
            name: "掉包率（整體）".into(),
            value: r.loss_rate_pct,
            unit: "%",
            limit: Limit::Max(th.loss_pct),
            note: q.map_or(String::new(), |q| {
                format!("最嚴重的單通電話 {}%", fmt_num(q.loss_max_pct))
            }),
        },
        Check {
            name: "Jitter".into(),
            value: q.map(|q| q.jitter_p95_ms),
            unit: " ms",
            limit: Limit::Max(th.jitter_ms),
            note: "每通電話的 P95".into(),
        },
    ]
}

fn overall(checks: &[Check]) -> Verdict {
    let v: Vec<Verdict> = checks.iter().map(Check::verdict).collect();
    if v.contains(&Verdict::Fail) {
        Verdict::Fail
    } else if v.iter().all(|x| *x == Verdict::NoData) {
        Verdict::NoData
    } else {
        Verdict::Pass
    }
}

/// 結論文字：規則產生
fn summary_text(file: &ResultFile, th: &Thresholds, checks: &[Check], points: &[Point]) -> String {
    let r = &file.report;
    let peak = points.iter().map(|p| p.active).fold(0.0, f64::max);
    let mut lines = vec![format!(
        "本次測試持續 {}，共發起 {} 通，接通 {} 通（接通率 {:.1}%），最高並發 {:.0} 通。",
        duration_label(r.duration_secs),
        r.calls_initiated,
        r.calls_answered,
        r.asr,
        peak
    )];
    for c in checks {
        let Some(v) = c.value else { continue };
        let pass = c.limit.ok(v);
        lines.push(format!(
            "{}為 {}{}，{}門檻 {}{}，判定{}。",
            c.name,
            fmt_num(v),
            c.unit,
            match (&c.limit, pass) {
                (Limit::Min(_), true) => "高於",
                (Limit::Min(_), false) => "低於",
                (Limit::Max(_), true) => "低於",
                (Limit::Max(_), false) => "超過",
            },
            fmt_num(c.limit.value()),
            c.unit,
            if pass { "正常" } else { "未達標" }
        ));
    }
    // 超過門檻的時段
    let bad: Vec<&Point> = points
        .iter()
        .filter(|p| {
            p.rtp.as_ref().is_some_and(|q| {
                q.mos_jb200_avg < th.mos
                    || q.loss_pct > th.loss_pct
                    || q.jitter_avg_ms > th.jitter_ms
            })
        })
        .collect();
    if let (Some(first), Some(last)) = (bad.first(), bad.last()) {
        lines.push(format!(
            "以時段來看，有 {} 個時段（共約 {}）的聲音品質超過門檻，出現在 <span class=\"clock\" data-offset=\"{:.0}\">{}</span> 至 <span class=\"clock\" data-offset=\"{:.0}\">{}</span> 之間，詳見第 4 節的圖表。",
            bad.len(),
            duration_label(bad.iter().map(|p| p.span).sum()),
            first.t - first.span / 2.0,
            clock(first.t - first.span / 2.0),
            last.t + last.span / 2.0,
            clock(last.t + last.span / 2.0),
        ));
    } else if points.iter().any(|p| p.rtp.is_some()) {
        lines.push("每個時段的聲音品質都在門檻內。".into());
    }
    format!(
        "<div class=\"summary\">{}</div>",
        lines
            .iter()
            .map(|l| format!("<p>{}</p>", l))
            .collect::<String>()
    )
}

fn conditions(file: &ResultFile) -> Vec<(&'static str, String)> {
    let c = &file.config;
    let r = &file.report;
    let mode = match cfg_str(c, "mode").as_str() {
        "agent" => "座席端（接聽）",
        _ => "民眾端（撥出）",
    };
    let mut rows = vec![
        ("測試模式", mode.to_string()),
        ("SIP 伺服器", esc(&cfg_str(c, "server_addr"))),
        ("測試時長", duration_label(r.duration_secs)),
    ];
    if cfg_str(c, "mode") != "agent" {
        rows.push(("目標 CPS", cfg_num(c, "cps")));
        rows.push(("並發上限", cfg_num(c, "max_concurrent_calls")));
        rows.push((
            "每通通話時長",
            format!("{} 秒", cfg_num(c, "call_duration_secs")),
        ));
    } else {
        rows.push(("座席帳號數", cfg_num(c, "agent_account_count")));
    }
    rows.push((
        "語音",
        if c.get("enable_rtp").and_then(Value::as_bool) == Some(true) {
            match cfg_str(c, "codec").as_str() {
                "pcmu" => "G.711 μ-law（PCMU）".to_string(),
                _ => "G.711 A-law（PCMA）".to_string(),
            }
        } else {
            "未啟用 RTP（只測 SIP 信令）".to_string()
        },
    ));
    rows.push(("統計取樣", "10 秒一段".to_string()));
    rows
}

// ── 主機資源 ──────────────────────────────────────────────────────

/// 一台主機在測試期間的資料
struct HostView<'a> {
    host: &'a Host,
    /// 測試期間的樣本與相對測試開始的秒數
    samples: Vec<(f64, &'a HostSample)>,
    disks: Vec<String>,
}

fn host_views<'a>(file: &ResultFile, hm: &'a HostMetrics) -> Vec<HostView<'a>> {
    let start = parse_rfc3339(&file.report.started_at);
    let dur = file.report.duration_secs;
    hm.hosts
        .iter()
        .map(|host| {
            let samples: Vec<(f64, &HostSample)> = match start {
                Some(t0) => host
                    .samples
                    .iter()
                    .map(|s| ((s.time - t0) as f64, s))
                    .filter(|(t, _)| *t >= -10.0 && *t <= dur + 10.0)
                    .collect(),
                None => Vec::new(),
            };
            let mut disks: Vec<String> = samples
                .iter()
                .flat_map(|(_, s)| s.disk_queue.keys().chain(s.disk_util.keys()))
                .cloned()
                .collect();
            disks.sort();
            disks.dedup();
            HostView {
                host,
                samples,
                disks,
            }
        })
        .collect()
}

/// 依點數上限把 10 秒樣本分組
fn host_groups<'a, 'b>(samples: &'b [(f64, &'a HostSample)]) -> Vec<&'b [(f64, &'a HostSample)]> {
    let k = samples.len().div_ceil(MAX_POINTS).max(1);
    samples.chunks(k).collect()
}

fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (n, sum) = v.fold((0usize, 0.0), |(n, s), x| (n + 1, s + x));
    (n > 0).then(|| sum / n as f64)
}

/// 原始樣本的（平均, 最高, P95）
fn stats(v: &[f64]) -> Option<(f64, f64, f64)> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = ((0.95 * s.len() as f64).ceil() as usize).clamp(1, s.len());
    Some((
        s.iter().sum::<f64>() / s.len() as f64,
        s[s.len() - 1],
        s[rank - 1],
    ))
}

impl HostView<'_> {
    fn values(&self, f: impl Fn(&HostSample) -> Option<f64>) -> Vec<f64> {
        self.samples.iter().filter_map(|(_, s)| f(s)).collect()
    }

    fn checks(&self, th: &Thresholds) -> Vec<Check> {
        let name = &self.host.name;
        let mut out = Vec::new();
        let mut push = |label: String, v: Vec<f64>, unit: &'static str, limit: f64| {
            if let Some((avg, max, p95)) = stats(&v) {
                out.push(Check {
                    name: label,
                    value: Some(p95),
                    unit,
                    limit: Limit::Max(limit),
                    note: format!(
                        "10 秒取樣的 P95；平均 {}{}，最高 {}{}",
                        fmt_num(avg),
                        unit,
                        fmt_num(max),
                        unit
                    ),
                });
            }
        };
        push(
            format!("CPU 使用率（{}）", esc(name)),
            self.values(|s| s.cpu_pct),
            "%",
            th.cpu_pct,
        );
        push(
            format!("記憶體使用率（{}）", esc(name)),
            self.values(|s| s.mem_pct),
            "%",
            th.mem_pct,
        );
        for d in &self.disks {
            push(
                format!("Disk Queue（{} {}）", esc(name), esc(d)),
                self.values(|s| s.disk_queue.get(d).copied()),
                "",
                th.disk_queue,
            );
        }
        out
    }

    fn section(&self, th: &Thresholds) -> String {
        let h = self.host;
        let mut out = format!(r#"<h3>{}</h3>"#, esc(&h.name));
        // 規格
        if let Some(sys) = &h.system {
            let mut rows = vec![
                ("作業系統", esc(&sys.os)),
                ("核心", esc(&format!("{} {}", sys.kernel, sys.arch))),
                (
                    "CPU",
                    esc(&format!(
                        "{}（{} 核心，實體 {}）",
                        sys.cpu_model, sys.cores, sys.physical_cores
                    )),
                ),
                (
                    "記憶體",
                    format!("{}，swap {}", bytes(sys.mem_total), bytes(sys.swap_total)),
                ),
            ];
            if !sys.virt.is_empty() {
                rows.push(("虛擬化", esc(&sys.virt)));
            }
            if !sys.disks.is_empty() {
                rows.push((
                    "磁碟",
                    sys.disks
                        .iter()
                        .map(|d| format!("{} {}", esc(&d.name), bytes(d.size)))
                        .collect::<Vec<_>>()
                        .join("、"),
                ));
            }
            for fs in &sys.filesystems {
                rows.push((
                    "檔案系統",
                    format!(
                        "{}（{}）已用 {} / {}（{}%）",
                        esc(&fs.mount),
                        esc(&fs.fstype),
                        bytes(fs.used),
                        bytes(fs.total),
                        fmt_num(fs.used_pct)
                    ),
                ));
            }
            out.push_str(r#"<table class="tbl kv"><tbody>"#);
            for (k, v) in rows {
                out.push_str(&format!("<tr><th>{}</th><td>{}</td></tr>", k, v));
            }
            out.push_str("</tbody></table>");
            out.push_str(&format!(
                r#"<p class="hint">規格為主機 agent {} 匯出當下（{}）的狀態。</p>"#,
                esc(&h.agent_version),
                esc(&sys.collected_at)
            ));
        }
        for e in &h.errors {
            out.push_str(&format!(r#"<p class="note">{}</p>"#, esc(e)));
        }
        if self.samples.is_empty() {
            out.push_str(r#"<p class="note">測試期間沒有這台主機的效能資料：請確認匯出的時間範圍涵蓋測試時間。</p>"#);
            return out;
        }

        let groups = host_groups(&self.samples);
        let x: Vec<f64> = groups
            .iter()
            .map(|g| (g[0].0 + g[g.len() - 1].0) / 2.0)
            .collect();
        let series = |f: &dyn Fn(&HostSample) -> Option<f64>| -> Vec<Option<f64>> {
            groups
                .iter()
                .map(|g| mean(g.iter().filter_map(|(_, s)| f(s))))
                .collect()
        };
        let summary = |v: Vec<f64>| stats(&v).map(|(a, m, _)| (a, m));
        out.push_str(
            &Chart {
                title: "CPU 使用率".into(),
                unit: "%",
                y_min: 0.0,
                y_max: Some(100.0),
                series: vec![Series::new("CPU", C_GREEN, series(&|s| s.cpu_pct)).area()],
                limit: Some(Limit::Max(th.cpu_pct)),
                right: None,
                summary: summary(self.values(|s| s.cpu_pct)),
            }
            .svg(&x),
        );
        out.push_str(
            &Chart {
                title: "記憶體使用率".into(),
                unit: "%",
                y_min: 0.0,
                y_max: Some(100.0),
                series: vec![Series::new("記憶體", C_BLUE, series(&|s| s.mem_pct)).area()],
                limit: Some(Limit::Max(th.mem_pct)),
                right: None,
                summary: summary(self.values(|s| s.mem_pct)),
            }
            .svg(&x),
        );
        for d in &self.disks {
            let size = h
                .system
                .as_ref()
                .and_then(|s| s.disks.iter().find(|x| &x.name == d))
                .map_or(String::new(), |x| format!(" {}", bytes(x.size)));
            let util = series(&|s| s.disk_util.get(d).copied());
            out.push_str(
                &Chart {
                    title: format!("磁碟 {}{}", esc(d), size),
                    unit: "",
                    y_min: 0.0,
                    y_max: None,
                    series: vec![Series::new(
                        "Disk Queue（平均 I/O 佇列長度）",
                        C_GREEN,
                        series(&|s| s.disk_queue.get(d).copied()),
                    )
                    .area()],
                    limit: Some(Limit::Max(th.disk_queue)),
                    right: util.iter().any(Option::is_some).then(|| RightAxis {
                        unit: "%",
                        y_max: Some(100.0),
                        series: vec![Series::new("忙碌率（utilization）", C_BLUE, util)],
                    }),
                    summary: summary(self.values(|s| s.disk_queue.get(d).copied())),
                }
                .svg(&x),
            );
        }
        out
    }
}

/// bytes → GB / TB（1024 進位）
fn bytes(b: u64) -> String {
    let gb = b as f64 / 1024f64.powi(3);
    if gb >= 1024.0 {
        format!("{} TB", fmt_num((gb / 1024.0 * 10.0).round() / 10.0))
    } else {
        format!("{} GB", fmt_num(gb.round()))
    }
}

// ── 時段合併 ──────────────────────────────────────────────────────

struct RtpPoint {
    loss_pct: f64,
    loss_max_pct: f64,
    jitter_avg_ms: f64,
    jitter_p95_ms: f64,
    mos_avg: f64,
    mos_jb200_avg: f64,
    mos_jb200_p5: f64,
}

struct Point {
    /// 時段中點（測試開始後秒數）
    t: f64,
    /// 時段長度（秒）
    span: f64,
    cps: f64,
    /// 時段內最高的「段尾進行中通話數」
    active: f64,
    asr: Option<f64>,
    rtp: Option<RtpPoint>,
}

/// 依點數上限合併相鄰時段：計數相加後重算比率，品質指標取較差值
fn merge(buckets: &[TimelineBucket]) -> Vec<Point> {
    let k = buckets.len().div_ceil(MAX_POINTS).max(1);
    buckets
        .chunks(k)
        .map(|g| {
            let (start, end) = (g[0].start_secs, g[g.len() - 1].end_secs);
            let sum = |f: fn(&TimelineBucket) -> u64| g.iter().map(f).sum::<u64>();
            let decided = sum(|b| b.answered) + sum(|b| b.failed) + sum(|b| b.timeout);
            let rtps: Vec<_> = g.iter().filter_map(|b| b.rtp.as_ref()).collect();
            let rtp = (!rtps.is_empty()).then(|| {
                let calls: u64 = rtps.iter().map(|r| r.calls).sum();
                let exp: u64 = rtps.iter().map(|r| r.expected_packets).sum();
                let lost: u64 = rtps.iter().map(|r| r.lost_packets).sum();
                let w = |f: fn(&crate::timeline::RtpBucketStats) -> f64| {
                    rtps.iter().map(|r| f(r) * r.calls as f64).sum::<f64>() / calls.max(1) as f64
                };
                let max = |f: fn(&crate::timeline::RtpBucketStats) -> f64| {
                    rtps.iter().map(|r| f(r)).fold(f64::MIN, f64::max)
                };
                let min = |f: fn(&crate::timeline::RtpBucketStats) -> f64| {
                    rtps.iter().map(|r| f(r)).fold(f64::MAX, f64::min)
                };
                RtpPoint {
                    loss_pct: if exp > 0 {
                        lost as f64 / exp as f64 * 100.0
                    } else {
                        w(|r| r.loss_pct)
                    },
                    loss_max_pct: max(|r| r.loss_max_pct),
                    jitter_avg_ms: w(|r| r.jitter_avg_ms),
                    jitter_p95_ms: max(|r| r.jitter_p95_ms),
                    mos_avg: w(|r| r.mos_avg),
                    mos_jb200_avg: w(|r| r.mos_jb200_avg),
                    mos_jb200_p5: min(|r| r.mos_jb200_p5),
                }
            });
            Point {
                t: (start + end) / 2.0,
                span: end - start,
                cps: sum(|b| b.initiated) as f64 / (end - start).max(0.001),
                active: g.iter().map(|b| b.active).max().unwrap_or(0) as f64,
                asr: (decided > 0).then(|| sum(|b| b.answered) as f64 / decided as f64 * 100.0),
                rtp,
            }
        })
        .collect()
}

// ── 圖表 ──────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Limit {
    /// 不得低於
    Min(f64),
    /// 不得高於
    Max(f64),
}

impl Limit {
    fn value(self) -> f64 {
        match self {
            Self::Min(v) | Self::Max(v) => v,
        }
    }
    fn ok(self, v: f64) -> bool {
        match self {
            Self::Min(l) => v >= l,
            Self::Max(l) => v <= l,
        }
    }
    fn label(self) -> String {
        match self {
            Self::Min(v) => format!("≥ {}", fmt_num(v)),
            Self::Max(v) => format!("≤ {}", fmt_num(v)),
        }
    }
}

struct Series {
    name: &'static str,
    color: &'static str,
    y: Vec<Option<f64>>,
    /// 線下填色（面積圖）
    fill: bool,
    /// 虛線
    dash: bool,
}

impl Series {
    fn new(name: &'static str, color: &'static str, y: Vec<Option<f64>>) -> Self {
        Self {
            name,
            color,
            y,
            fill: false,
            dash: false,
        }
    }

    fn area(mut self) -> Self {
        self.fill = true;
        self
    }

    fn dashed(mut self) -> Self {
        self.dash = true;
        self
    }

    /// 折線（None 斷開）；`base` = 填色面積的底線 y 座標
    fn path(&self, pts: &[Option<(f64, f64)>], base: f64, width: f64) -> String {
        let mut out = String::new();
        for seg in pts.split(Option::is_none) {
            let seg: Vec<(f64, f64)> = seg.iter().flatten().copied().collect();
            let Some(&(x0, y0)) = seg.first() else {
                continue;
            };
            if seg.len() == 1 {
                let _ = write!(
                    out,
                    r#"<circle cx="{x0:.1}" cy="{y0:.1}" r="2" fill="{}"/>"#,
                    self.color
                );
                continue;
            }
            let line: Vec<String> = seg
                .iter()
                .map(|(x, y)| format!("{:.1},{:.1}", x, y))
                .collect();
            if self.fill {
                let x1 = seg[seg.len() - 1].0;
                let _ = write!(
                    out,
                    r#"<polygon fill="{}" fill-opacity="0.28" stroke="none" points="{x0:.1},{base:.1} {} {x1:.1},{base:.1}"/>"#,
                    self.color,
                    line.join(" ")
                );
            }
            let _ = write!(
                out,
                r#"<polyline fill="none" stroke="{}" stroke-width="{width}"{} stroke-linejoin="round" points="{}"/>"#,
                self.color,
                if self.dash {
                    r#" stroke-dasharray="6 4""#
                } else {
                    ""
                },
                line.join(" ")
            );
        }
        out
    }
}

/// 一張折線圖：第一條線為主線（畫平均線、標示超過門檻的時段，下方列平均與最高 / 最低）
struct Chart {
    title: String,
    unit: &'static str,
    y_min: f64,
    y_max: Option<f64>,
    series: Vec<Series>,
    limit: Option<Limit>,
    /// 右側 Y 軸的線（例如磁碟忙碌率）
    right: Option<RightAxis>,
    /// 下方統計的（平均, 最高 / 最低）：圖上的點是合併後的值時，改用原始資料算
    summary: Option<(f64, f64)>,
}

/// 右側 Y 軸：從 0 開始
struct RightAxis {
    unit: &'static str,
    y_max: Option<f64>,
    series: Vec<Series>,
}

impl Chart {
    fn svg(&self, x: &[f64]) -> String {
        const W: f64 = 720.0;
        const H: f64 = 220.0;
        const L: f64 = 48.0;
        const T: f64 = 12.0;
        const B: f64 = 26.0;
        let rm = if self.right.is_some() { 44.0 } else { 14.0 };
        let main = &self.series[0];
        let vals: Vec<f64> = main.y.iter().flatten().copied().collect();
        if x.len() < 2 || vals.is_empty() {
            return format!(
                r#"<figure class="chart"><figcaption>{}</figcaption><p class="hint">沒有這段時間的資料，無法繪圖。</p></figure>"#,
                self.title
            );
        }
        let x0 = x[0].min(0.0);
        let x1 = x[x.len() - 1];
        let all_max = self
            .series
            .iter()
            .flat_map(|s| s.y.iter().flatten())
            .copied()
            .fold(f64::MIN, f64::max);
        let top = self.y_max.unwrap_or_else(|| {
            let m = all_max.max(self.limit.map_or(0.0, |l| l.value()));
            nice_ceil(if m <= self.y_min {
                self.y_min + 1.0
            } else {
                m * 1.1
            })
        });
        let px = |v: f64| L + (v - x0) / (x1 - x0).max(1e-9) * (W - L - rm);
        let py =
            |v: f64| H - B - ((v - self.y_min) / (top - self.y_min)).clamp(0.0, 1.0) * (H - T - B);

        let mut g = String::new();
        // 格線與 Y 軸刻度：分成 4 或 5 格，取每格是整齊數字的那一種
        let range = top - self.y_min;
        let rows = if is_nice(range / 5.0) { 5 } else { 4 };
        for i in 0..=rows {
            let v = self.y_min + range * i as f64 / rows as f64;
            let _ = write!(
                g,
                r##"<line x1="{L}" y1="{y:.1}" x2="{x2}" y2="{y:.1}" stroke="#e6e9ef"/><text x="{tx}" y="{ty:.1}" class="ax" text-anchor="end">{v}</text>"##,
                y = py(v),
                x2 = W - rm,
                tx = L - 6.0,
                ty = py(v) + 4.0,
                v = fmt_num(v),
            );
        }
        // X 軸刻度：整齊的間隔，最多 6 格（data-offset 讓瀏覽器換成當地時間）
        let step = [
            10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0, 10800.0, 21600.0,
        ]
        .into_iter()
        .find(|s| (x1 - x0) / s <= 6.0)
        .unwrap_or(43200.0);
        let mut t = (x0 / step).ceil() * step;
        while t <= x1 + 1e-9 {
            let anchor = if px(t) > W - rm - 30.0 {
                "end"
            } else {
                "middle"
            };
            let _ = write!(
                g,
                r#"<text x="{x:.1}" y="{y}" class="ax clock" data-offset="{t:.0}" text-anchor="{anchor}">{label}</text>"#,
                x = px(t),
                y = H - 8.0,
                label = clock(t),
            );
            t += step;
        }
        // 超過門檻的時段
        if let Some(limit) = self.limit {
            let half = |i: usize| {
                let prev = if i > 0 { x[i - 1] } else { x0 };
                let next = if i + 1 < x.len() { x[i + 1] } else { x1 };
                ((x[i] + prev) / 2.0, (x[i] + next) / 2.0)
            };
            for (i, v) in main.y.iter().enumerate() {
                if v.is_some_and(|v| !limit.ok(v)) {
                    let (a, b) = half(i);
                    let _ = write!(
                        g,
                        r##"<rect x="{x:.1}" y="{T}" width="{w:.1}" height="{h}" fill="#dc2626" fill-opacity="0.16"/>"##,
                        x = px(a),
                        w = (px(b) - px(a)).max(1.0),
                        h = H - T - B,
                    );
                }
            }
        }
        // 折線（後面的線先畫，主線在最上層；右軸的線畫在主線下面）
        let mut lines = String::new();
        for s in self.series.iter().rev() {
            let pts: Vec<Option<(f64, f64)>> = x
                .iter()
                .zip(&s.y)
                .map(|(xi, yi)| yi.map(|v| (px(*xi), py(v))))
                .collect();
            let width = if std::ptr::eq(s, main) { 2.2 } else { 1.6 };
            lines.push_str(&s.path(&pts, py(self.y_min), width));
        }
        // 右側 Y 軸
        if let Some(ra) = &self.right {
            let rtop = ra.y_max.unwrap_or_else(|| {
                let m = ra
                    .series
                    .iter()
                    .flat_map(|s| s.y.iter().flatten())
                    .copied()
                    .fold(0.0, f64::max);
                nice_ceil((m * 1.1).max(1.0))
            });
            let pyr = |v: f64| H - B - (v / rtop).clamp(0.0, 1.0) * (H - T - B);
            for i in 0..=rows {
                let v = rtop * i as f64 / rows as f64;
                let _ = write!(
                    g,
                    r#"<text x="{tx}" y="{ty:.1}" class="ax" text-anchor="start">{v}</text>"#,
                    tx = W - rm + 6.0,
                    ty = pyr(v) + 4.0,
                    v = fmt_num(v),
                );
            }
            for s in &ra.series {
                let pts: Vec<Option<(f64, f64)>> = x
                    .iter()
                    .zip(&s.y)
                    .map(|(xi, yi)| yi.map(|v| (px(*xi), pyr(v))))
                    .collect();
                g.push_str(&s.path(&pts, pyr(0.0), 1.6));
            }
        }
        g.push_str(&lines);
        // 平均線、門檻線
        let avg = vals.iter().sum::<f64>() / vals.len() as f64;
        let _ = write!(
            g,
            r#"<line x1="{L}" y1="{y:.1}" x2="{x2}" y2="{y:.1}" stroke="{C_AVG}" stroke-width="1.4" stroke-dasharray="2 3"/><text x="{tx}" y="{ty:.1}" class="tag-l" fill="{C_AVG}" text-anchor="start">平均 {v}</text>"#,
            y = py(avg),
            x2 = W - rm,
            tx = L + 4.0,
            ty = py(avg) - 4.0,
            v = fmt_num(avg),
        );
        if let Some(limit) = self.limit {
            let _ = write!(
                g,
                r##"<line x1="{L}" y1="{y:.1}" x2="{x2}" y2="{y:.1}" stroke="{C_LIMIT}" stroke-width="2" stroke-dasharray="8 4"/><text x="{tx}" y="{ty:.1}" class="tag-l" fill="#b45309" text-anchor="end">門檻 {v}{u}</text>"##,
                y = py(limit.value()),
                x2 = W - rm,
                tx = W - rm - 4.0,
                ty = match limit {
                    Limit::Min(_) => py(limit.value()) + 14.0,
                    Limit::Max(_) => py(limit.value()) - 4.0,
                },
                v = fmt_num(limit.value()),
                u = self.unit.trim(),
            );
        }

        // 圖例與統計
        let mut legend: String = self
            .series
            .iter()
            .map(|s| format!(r#"<span>{}{}</span>"#, swatch(s), s.name))
            .collect();
        if let Some(ra) = &self.right {
            for s in &ra.series {
                let _ = write!(
                    legend,
                    r#"<span>{}{}（右軸，{}）</span>"#,
                    swatch(s),
                    s.name,
                    ra.unit
                );
            }
        }
        legend.push_str(r#"<span><i class="dash avg"></i>平均</span>"#);
        if self.limit.is_some() {
            legend.push_str(r#"<span><i class="dash limit"></i>門檻</span>"#);
        }
        let worst = match self.limit {
            Some(Limit::Min(_)) => ("最低", vals.iter().copied().fold(f64::MAX, f64::min)),
            _ => ("最高", vals.iter().copied().fold(f64::MIN, f64::max)),
        };
        let (avg_txt, worst) = match self.summary {
            Some((a, w)) => (a, (worst.0, w)),
            None => (avg, worst),
        };
        let mut stats = format!(
            "{}：平均 <b>{}{}</b> · {} <b>{}{}</b>",
            match main.name.trim_end_matches("平均") {
                "" => self.title.as_str(),
                n => n,
            },
            fmt_num(avg_txt),
            self.unit,
            worst.0,
            fmt_num(worst.1),
            self.unit
        );
        if let Some(limit) = self.limit {
            let n = main.y.iter().flatten().filter(|v| !limit.ok(**v)).count();
            let _ = write!(
                stats,
                " · 門檻 <b>{}{}</b> · 超過門檻的時段 <b>{}</b> / {}",
                limit.label(),
                self.unit,
                n,
                vals.len()
            );
        }
        format!(
            r#"<figure class="chart"><figcaption>{title}{unit}</figcaption>
<svg viewBox="0 0 {W} {H}" role="img" aria-label="{title}">{g}</svg>
<div class="legend">{legend}</div><div class="stats">{stats}</div></figure>
"#,
            title = self.title,
            unit = if self.unit.is_empty() {
                String::new()
            } else {
                format!("（{}）", self.unit.trim())
            },
        )
    }
}

/// 圖例的色塊：面積 = 方塊、虛線 = 虛線、其他 = 實線
fn swatch(s: &Series) -> String {
    if s.fill {
        format!(
            r#"<i class="area" style="background:{}55;border-color:{}"></i>"#,
            s.color, s.color
        )
    } else if s.dash {
        format!(r#"<i class="dash" style="border-color:{}"></i>"#, s.color)
    } else {
        format!(r#"<i style="background:{}"></i>"#, s.color)
    }
}

// ── 輔助函式 ──────────────────────────────────────────────────────

fn cfg_str(c: &Value, key: &str) -> String {
    c.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn cfg_num(c: &Value, key: &str) -> String {
    c.get(key)
        .and_then(Value::as_f64)
        .map_or("—".to_string(), fmt_num)
}

/// 數字：整數不帶小數，其餘最多 2 位
fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{:.0}", v)
    } else {
        let s = format!("{:.2}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 是否為 1 / 2 / 2.5 / 5 × 10ⁿ
fn is_nice(v: f64) -> bool {
    if v <= 0.0 {
        return false;
    }
    let m = v / 10f64.powf(v.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .any(|n| (m - n).abs() < 1e-6)
}

/// 往上取到 1 / 2 / 5 × 10ⁿ
fn nice_ceil(v: f64) -> f64 {
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * p)
        .find(|c| *c >= v)
        .unwrap_or(10.0 * p)
}

fn clock(secs: f64) -> String {
    let t = secs.max(0.0).round() as u64;
    format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

fn duration_label(secs: f64) -> String {
    let t = secs.round() as u64;
    match (t / 3600, t / 60 % 60, t % 60) {
        (0, 0, s) => format!("{} 秒", s),
        (0, m, 0) => format!("{} 分", m),
        (0, m, s) => format!("{} 分 {} 秒", m, s),
        (h, 0, _) => format!("{} 小時", h),
        (h, m, _) => format!("{} 小時 {} 分", h, m),
    }
}

const DEFINITIONS: &str = r#"<dl>
<dt>MOS（jitter buffer 200 ms 模擬）</dt>
<dd>判定用的 MOS。模擬話機的 200 ms 固定緩衝：封包延遲比基準（最近 30 至 60 秒內最小的延遲）多出 200 ms 以上就來不及播放，視同遺失，再以 ITU-T G.107 E-Model（G.711）換算成 MOS。完全不掉包時最高 4.41。判定看<strong>每通電話的 P5</strong>：95% 的通話不低於這個值。</dd>
<dt>MOS（E-Model）</dt>
<dd>只用掉包率與 Jitter 估算，Jitter 低於 150 ms 不扣分，所以主要反映掉包。列出供參考。</dd>
<dt>掉包率（整體）</dt>
<dd>所有通話的掉包封包數加總 ÷ 應收封包數加總（依 RTP 序號計算）。不是每通電話掉包率的平均；另列最嚴重的單通電話。</dd>
<dt>Jitter</dt>
<dd>RFC 3550 的到達間隔變異（平滑值）。判定看每通電話的 P95。</dd>
<dt>時段統計</dt>
<dd>每 10 秒一個時段，每個時段只計算該時段內新增的封包，短暫的品質下降不會被整通電話平均掉。測試很長時，圖表會合併相鄰時段，合併後的 MOS P5 取最低、掉包與 Jitter P95 取最高。</dd>
<dt>主機資源</dt>
<dd>CPU、記憶體使用率與 Disk Queue（該磁碟 10 秒內的平均 I/O 佇列長度，同 iostat 的 aqu-sz）取自主機 agent 的 10 秒取樣，判定看測試期間的 P95：95% 的時間不超過這個值。磁碟忙碌率（utilization）為 I/O 進行中的時間比例，只有新版 agent 有。圖表點數過多時以平均合併，下方的平均與最高用原始 10 秒資料計算。</dd>
<dt>限制</dt>
<dd>MOS 為依封包統計的估算值，沒有分析實際聲音內容；jitter buffer 為固定緩衝的近似，數字不會與 VoIPmonitor 等工具完全一致。</dd>
</dl>"#;

/// 把 UTC 時間與時間軸刻度換成檢視者的當地時間
const SCRIPT: &str = r#"(function(){
var el=document.querySelector('time.local');if(!el||!el.dataset.utc)return;
var t0=new Date(el.dataset.utc);if(isNaN(t0))return;
function p(n){return String(n).padStart(2,'0')}
var off=-t0.getTimezoneOffset(),tz='UTC'+(off>=0?'+':'-')+Math.floor(Math.abs(off)/60)+(off%60?':'+p(Math.abs(off)%60):'');
el.textContent=t0.getFullYear()+'/'+p(t0.getMonth()+1)+'/'+p(t0.getDate())+' '+p(t0.getHours())+':'+p(t0.getMinutes())+':'+p(t0.getSeconds())+'（'+tz+'）';
document.querySelectorAll('.clock[data-offset]').forEach(function(n){
var d=new Date(t0.getTime()+Number(n.dataset.offset)*1000);n.textContent=p(d.getHours())+':'+p(d.getMinutes())+':'+p(d.getSeconds());});
})();"#;

const CSS: &str = r#"
:root{--bg:#f5f6f8;--surface:#fff;--border:#dde1e9;--text:#1a2035;--muted:#6b7591;--light:#9ba3b8;
--accent:#1a6ef5;--green:#1a9e5c;--green-bg:#e8f7f0;--yellow:#c47d00;--yellow-bg:#fff8e6;--red:#d93025;--red-bg:#fce8e6;--blue-bg:#e8f0fe;
--mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;--sans:system-ui,-apple-system,"Segoe UI","Noto Sans TC","Microsoft JhengHei",sans-serif}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--text);font-family:var(--sans);font-size:14px;line-height:1.65}
.page{max-width:860px;margin:0 auto;padding:24px 16px 32px;display:grid;gap:16px}
.cover{padding:8px 0 4px}
.brand{font-family:var(--mono);color:var(--accent);font-weight:600}
h1{font-size:26px;margin:4px 0 6px}
.meta{color:var(--muted);font-size:13px}.meta strong{color:var(--text)}
.card{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:18px 20px}
h2{font-size:16px;margin-bottom:12px}
.verdict{display:inline-block;font-size:18px;font-weight:700;padding:4px 16px;border-radius:6px;margin-bottom:12px}
.verdict.good{background:var(--green-bg);color:var(--green)}.verdict.bad{background:var(--red-bg);color:var(--red)}.verdict.info{background:var(--blue-bg);color:var(--accent)}
.summary p{margin-bottom:6px}
h3{font-size:15px;margin:18px 0 6px;padding-top:12px;border-top:1px solid var(--border)}
.note{background:var(--yellow-bg);border:1px solid #f0d9a8;color:#6b4a00;border-radius:6px;padding:8px 12px;font-size:13px;margin:8px 0}
.tbl{width:100%;border-collapse:collapse;font-size:13px;margin-top:10px}
.tbl th{text-align:left;font-size:12px;color:var(--muted);padding:7px 10px;border-bottom:2px solid var(--border)}
.tbl td{padding:7px 10px;border-bottom:1px solid var(--border);vertical-align:top}
.tbl.kv th{width:30%;border-bottom:1px solid var(--border);font-weight:600}
.num{font-family:var(--mono)}
.sub{font-family:var(--sans);font-size:11px;color:var(--muted)}
.tag{display:inline-block;padding:1px 10px;border-radius:10px;font-size:12px;font-weight:600}
.tag.good{background:var(--green-bg);color:var(--green)}.tag.bad{background:var(--red-bg);color:var(--red)}.tag.info{background:var(--blue-bg);color:var(--accent)}
.hint{font-size:12px;color:var(--muted);margin-bottom:8px}
.chart{margin:14px 0 6px}
.chart figcaption{font-weight:600;font-size:14px;margin-bottom:4px}
.chart svg{width:100%;height:auto;display:block}
.ax{font-size:11px;fill:var(--muted);font-family:var(--mono)}
.legend{display:flex;flex-wrap:wrap;gap:4px 14px;font-size:12px;color:var(--muted);margin-top:2px}
.legend i{display:inline-block;width:14px;height:3px;margin-right:5px;vertical-align:middle}
.legend i.dash{height:0;border-top:2px dashed}.legend i.avg{border-color:#374151;border-top-style:dotted}.legend i.limit{border-color:#f59e0b}
.legend i.area{height:10px;border-top:2px solid}
.tag-l{font-size:11px;font-weight:600;paint-order:stroke;stroke:#fff;stroke-width:3px}
.stats{font-size:12px;margin-top:4px}.stats b{font-family:var(--mono)}
.defs dt{font-weight:600;margin-top:10px}.defs dd{color:var(--muted);font-size:13px}
.footer{text-align:center;font-size:11px;color:var(--light);padding:12px;font-family:var(--mono)}
@page{size:A4;margin:14mm 12mm}
@media print{body{background:#fff;font-size:12px}.page{max-width:none;padding:0;gap:10px}
.card{border:none;padding:0;break-inside:auto}.chart,.tbl tr{break-inside:avoid}h2{break-after:avoid}
section+section{break-before:auto}.footer{padding:6px}}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::rtp::stats::RtpStatsSnapshot;
    use crate::stats::{DetailedStats, FinalReport, LiveStats};
    use crate::timeline::RtpBucketStats;

    fn bucket(i: usize, mos: f64, loss: f64) -> TimelineBucket {
        TimelineBucket {
            start_secs: i as f64 * 10.0,
            end_secs: (i + 1) as f64 * 10.0,
            initiated: 10,
            answered: 10,
            active: 50,
            rtp: Some(RtpBucketStats {
                calls: 50,
                expected_packets: 25_000,
                lost_packets: (25_000.0 * loss / 100.0) as u64,
                loss_pct: loss,
                loss_max_pct: loss * 3.0,
                jitter_avg_ms: 5.0,
                jitter_p95_ms: 9.0,
                mos_avg: mos,
                mos_min: mos - 0.5,
                mos_p5: mos - 0.2,
                mos_jb50_avg: mos,
                mos_jb200_avg: mos,
                mos_jb200_min: mos - 0.5,
                mos_jb200_p5: mos - 0.2,
                below_threshold: 0,
            }),
            ..Default::default()
        }
    }

    fn file(buckets: Vec<TimelineBucket>, calls: &[(f64, f64)]) -> ResultFile {
        let mut r = FinalReport::build(
            &LiveStats::default().snapshot(),
            &DetailedStats::default(),
            buckets.len() as f64 * 10.0,
            None,
        );
        r.started_at = "2026-10-28T05:00:00Z".into();
        r.fine_timeline = buckets;
        r.loss_rate_pct = Some(0.2);
        let snaps: Vec<_> = calls
            .iter()
            .map(|&(mos, jb)| RtpStatsSnapshot {
                recv_packets: 1000,
                mos,
                mos_jb200: jb,
                jitter_ms: 6.0,
                ..Default::default()
            })
            .collect();
        r.call_quality = CallQuality::from_snapshots(&snaps);
        ResultFile::new(&Config::default(), &r)
    }

    #[test]
    fn passes_when_all_checks_within_thresholds() {
        let f = file(
            (0..30).map(|i| bucket(i, 4.3, 0.1)).collect(),
            &[(4.4, 4.3); 20],
        );
        let html = IntegratedReport::render(&f, &Thresholds::default(), None);
        assert!(html.contains(r#"<div class="verdict good">通過</div>"#));
        assert!(html.contains("每個時段的聲音品質都在門檻內"));
        assert!(html.contains("@page{size:A4"));
        assert!(html.contains(r#"data-utc="2026-10-28T05:00:00Z""#));
    }

    #[test]
    fn fails_and_locates_bad_periods() {
        let mut b: Vec<_> = (0..30).map(|i| bucket(i, 4.3, 0.1)).collect();
        b[12] = bucket(12, 3.1, 2.0);
        b[13] = bucket(13, 3.0, 2.5);
        let mut calls = vec![(4.4, 4.3); 18];
        calls.extend([(3.4, 3.0), (3.5, 3.1)]);
        let html = IntegratedReport::render(&file(b, &calls), &Thresholds::default(), None);
        assert!(html.contains(r#"<div class="verdict bad">未通過</div>"#));
        // 20 通的 P5 = 最低的一通（3.0）
        assert!(html.contains("判定未達標"));
        assert!(html.contains("有 2 個時段"), "{}", html);
        assert!(html.contains("0:02:00"), "第一個不良時段從 2 分開始");
        assert!(html.contains("18（90.0%）達標"));
    }

    #[test]
    fn merges_long_runs_keeping_worst_quality() {
        // 1000 段 → 每 4 段合併成一點（250 點）
        let mut b: Vec<_> = (0..1000).map(|i| bucket(i, 4.3, 0.1)).collect();
        b[5] = bucket(5, 2.0, 5.0);
        let p = merge(&b);
        assert_eq!(p.len(), 250);
        let q = p[1].rtp.as_ref().unwrap();
        assert!((q.mos_jb200_p5 - 1.8).abs() < 1e-9, "P5 取最低");
        assert!((q.loss_max_pct - 15.0).abs() < 1e-9, "最嚴重單通取最高");
        assert!((p[1].cps - 1.0).abs() < 1e-9);
        assert_eq!(p[1].span, 40.0);
    }

    #[test]
    fn handles_runs_without_rtp() {
        let b: Vec<_> = (0..10)
            .map(|i| TimelineBucket {
                rtp: None,
                ..bucket(i, 4.3, 0.0)
            })
            .collect();
        let mut f = file(b, &[]);
        f.report.loss_rate_pct = None;
        let html = IntegratedReport::render(&f, &Thresholds::default(), None);
        assert!(html.contains("無法判定"));
        assert!(html.contains("未啟用 RTP，沒有聲音品質數據"));
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(fmt_num(4.0), "4");
        assert_eq!(fmt_num(0.5), "0.5");
        assert_eq!(fmt_num(4.214), "4.21");
        assert_eq!(fmt_num(48.004), "48");
        assert_eq!(nice_ceil(0.55), 1.0);
        assert_eq!(nice_ceil(33.0), 50.0);
        assert_eq!(duration_label(7260.0), "2 小時 1 分");
    }

    #[test]
    fn host_section_judges_p95_and_lists_specs() {
        use crate::host_metrics::{DiskInfo, HostSystem};
        let f = file(
            (0..30).map(|i| bucket(i, 4.3, 0.1)).collect(),
            &[(4.4, 4.3); 20],
        );
        let t0 = parse_rfc3339(&f.report.started_at).unwrap();
        // 測試 300 秒：CPU 多數 50%、偶爾 95%（P95 仍為 50）；Disk Queue 持續 3（超過門檻 2）
        let samples = (-30..=330)
            .step_by(10)
            .map(|t| HostSample {
                time: t0 + t,
                cpu_pct: Some(if t == 100 { 95.0 } else { 50.0 }),
                mem_pct: Some(60.0),
                disk_queue: [("sda".to_string(), 3.0)].into(),
                disk_util: [("sda".to_string(), 40.0)].into(),
            })
            .collect();
        let hm = HostMetrics {
            hosts: vec![
                Host {
                    name: "db_01".into(),
                    agent_version: "v1.9.0".into(),
                    system: Some(HostSystem {
                        os: "Rocky Linux 9.4".into(),
                        cores: 8,
                        mem_total: 32 * 1024u64.pow(3),
                        disks: vec![DiskInfo {
                            name: "sda".into(),
                            size: 200 * 1024u64.pow(3),
                        }],
                        ..Default::default()
                    }),
                    samples,
                    ..Default::default()
                },
                Host {
                    name: "old".into(),
                    errors: vec!["效能紀錄:agent 版本不支援效能紀錄 (請升級)".into()],
                    ..Default::default()
                },
            ],
        };
        let html = IntegratedReport::render(&f, &Thresholds::default(), Some(&hm));
        assert!(html.contains("5. 受測主機資源") && html.contains("6. 指標說明"));
        assert!(html.contains("Rocky Linux 9.4") && html.contains("32 GB"));
        assert!(html.contains("磁碟 sda 200 GB"));
        assert!(html.contains("右軸"), "有 utilization 時畫右軸");
        // CPU P95 = 50 → 通過；Disk Queue P95 = 3 → 未通過
        assert!(html.contains("CPU 使用率（db_01）"));
        assert!(html.contains("最高 95%"));
        assert!(html.contains(r#"<div class="verdict bad">未通過</div>"#));
        assert!(html.contains("Disk Queue（db_01 sda）"));
        assert!(html.contains("agent 版本不支援"));
        assert!(html.contains("測試期間沒有這台主機的效能資料"));
    }
}
