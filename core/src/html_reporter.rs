/// HTML 報告產生器（淺色主題，含 RTP 聲音品質區塊）
use crate::stats::FinalReport;
use anyhow::Result;
use std::{fs, path::PathBuf};

pub struct HtmlReporter;

impl HtmlReporter {
    pub fn save(
        report: &FinalReport,
        output_dir: &str,
        timestamp: &str,
        server_addr: &str,
    ) -> Result<PathBuf> {
        fs::create_dir_all(output_dir)?;
        let filename = format!("{}_report.html", timestamp);
        let path = PathBuf::from(output_dir).join(&filename);
        fs::write(&path, Self::render(report, timestamp, server_addr))?;
        Ok(path)
    }

    pub fn render(r: &FinalReport, timestamp: &str, server_addr: &str) -> String {
        let asr_cls = kpi_cls(r.asr, 85.0, 70.0);
        let ccr_cls = kpi_cls(r.ccr, 85.0, 70.0);
        let fail_total = r.fail_4xx + r.fail_5xx + r.fail_6xx;

        let asr_arc = ring_svg(r.asr / 100.0, "ASR", &ring_color(r.asr, 85.0, 70.0));
        let ccr_arc = ring_svg(r.ccr / 100.0, "CCR", &ring_color(r.ccr, 85.0, 70.0));
        let pdd_bars = bar_chart(&[
            ("P50", r.pdd_p50_ms),
            ("P95", r.pdd_p95_ms),
            ("P99", r.pdd_p99_ms),
            ("MAX", r.pdd_max_ms),
        ]);
        let setup_bars = bar_chart(&[
            ("P50", r.setup_p50_ms),
            ("P95", r.setup_p95_ms),
            ("P99", r.setup_p99_ms),
            ("MAX", r.setup_max_ms),
        ]);
        let dist = dist_bar(r);
        let timeline_section = timeline_section(r);

        // RTP 區塊
        let rtp_section = match (r.mos, r.loss_rate_pct, r.jitter_ms) {
            (Some(mos), Some(loss), Some(jitter)) => {
                // 判定用含 jitter buffer 200 ms 模擬的 MOS（舊版數據沒有時退回 E-Model MOS）
                let judged = r.mos_jb200.filter(|v| *v > 0.0).unwrap_or(mos);
                let mos_cls = mos_badge_cls(judged);
                let mos_label = mos_label_str(judged);
                let loss_cls = loss_val_cls(loss);
                let jitter_cls = kpi_cls(150.0 - jitter.min(150.0), 100.0, 50.0);
                let sent = r.rtp_sent.unwrap_or(0);
                let recv = r.rtp_recv.unwrap_or(0);
                let ooo = r.rtp_out_of_order.unwrap_or(0);
                format!(
                    r#"<div class="card">
  <div class="section-title">聲音品質 — RTP 分析</div>
  <div style="display:flex;align-items:center;gap:12px;margin-bottom:16px">
    <span style="font-size:13px;color:var(--text-muted)">整體評分（MOS，jitter buffer 200 ms 模擬）</span>
    <div class="mos-badge {mos_cls}">MOS {judged:.2} — {mos_label}</div>
  </div>
  <div class="quality-grid">
    <div class="q-item">
      <div class="q-label">MOS（jitter buffer 200 ms 模擬）</div>
      <div class="q-val {judged_cls}">{judged:.2}<span style="font-size:12px;color:var(--text-muted);margin-left:4px">/ 4.41</span></div>
      <div class="q-sub">判定用；每通電話平均</div>
    </div>
    <div class="q-item">
      <div class="q-label">MOS（E-Model 估算）</div>
      <div class="q-val {mos_cls2}">{mos:.2}<span style="font-size:12px;color:var(--text-muted);margin-left:4px">/ 4.41</span></div>
      <div class="q-sub">ITU-T G.107；每通電話平均</div>
    </div>
    <div class="q-item">
      <div class="q-label">掉包率（整體）</div>
      <div class="q-val {loss_cls}">{loss:.2}<span style="font-size:13px;color:var(--text-muted)"> %</span></div>
      <div class="q-sub">所有通話的掉包數 ÷ 應收封包數</div>
    </div>
    <div class="q-item">
      <div class="q-label">Jitter</div>
      <div class="q-val {jitter_cls}">{jitter:.1}<span style="font-size:13px;color:var(--text-muted)"> ms</span></div>
      <div class="q-sub">RFC 3550；依接收封包數加權</div>
    </div>
  </div>
  <div class="rtp-meta">
    <div>傳送封包 &nbsp;<strong>{sent}</strong></div>
    <div>接收封包 &nbsp;<strong>{recv}</strong></div>
    <div>亂序封包 &nbsp;<strong>{ooo}</strong></div>
  </div>
</div>"#,
                    mos_cls = mos_cls,
                    mos = mos,
                    judged = judged,
                    judged_cls = mos_to_val_cls(judged),
                    mos_label = mos_label,
                    mos_cls2 = mos_to_val_cls(mos),
                    loss_cls = loss_cls,
                    loss = loss,
                    jitter_cls = jitter_cls,
                    jitter = jitter,
                    sent = sent,
                    recv = recv,
                    ooo = ooo,
                )
            }
            _ => r#"<div class="card" style="color:var(--text-muted);font-size:13px">
  <div class="section-title">聲音品質 — RTP 分析</div>
  未啟用 RTP（使用 <code>--rtp</code> 旗標啟用，可選 <code>--audio</code> 指定音檔）
</div>"#
                .to_string(),
        };

        format!(
            r#"<!DOCTYPE html>
<html lang="zh-Hant">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>sipress 壓測報告 — {ts}</title>
<style>
@import url('https://fonts.googleapis.com/css2?family=DM+Mono:wght@400;500&family=Lato:wght@300;400;700&display=swap');
:root{{
  --bg:#f5f6f8;--surface:#fff;--surface2:#f0f2f5;
  --border:#dde1e9;--border2:#c8cdd8;
  --text:#1a2035;--text-muted:#6b7591;--text-light:#9ba3b8;
  --accent:#1a6ef5;--accent-bg:#e8f0fe;
  --green:#1a9e5c;--green-bg:#e8f7f0;
  --yellow:#c47d00;--yellow-bg:#fff8e6;
  --red:#d93025;--red-bg:#fce8e6;
  --purple:#7c4dff;--purple-bg:#ede7ff;
  --mono:'DM Mono',monospace;--sans:'Lato',sans-serif;
  --radius:8px;--shadow:0 1px 4px rgba(0,0,0,.08),0 4px 16px rgba(0,0,0,.04);
}}
*,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
body{{background:var(--bg);color:var(--text);font-family:var(--sans);font-size:14px;line-height:1.6}}
.header{{background:var(--surface);border-bottom:1px solid var(--border);padding:18px 40px;display:flex;align-items:center;gap:16px}}
.logo{{font-family:var(--mono);font-size:18px;font-weight:500;color:var(--accent);display:flex;align-items:center;gap:8px}}
.logo-icon{{width:28px;height:28px;background:var(--accent);border-radius:6px;display:flex;align-items:center;justify-content:center;color:#fff;font-size:14px}}
.header-sep{{color:var(--border2);font-size:18px}}
.header-info{{font-size:13px;color:var(--text-muted)}}
.header-info strong{{color:var(--text);font-weight:700}}
.header-time{{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--text-light);background:var(--surface2);padding:4px 10px;border-radius:20px;border:1px solid var(--border)}}
.main{{max-width:1160px;margin:0 auto;padding:28px 40px 60px;display:grid;gap:20px}}
.section-title{{font-size:11px;font-weight:700;letter-spacing:1.5px;text-transform:uppercase;color:var(--text-muted);margin-bottom:12px;display:flex;align-items:center;gap:8px}}
.section-title::after{{content:'';flex:1;height:1px;background:var(--border)}}
.kpi-grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px}}
.kpi{{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:16px 18px;box-shadow:var(--shadow)}}
.kpi.good{{border-left:3px solid var(--green)}}
.kpi.warn{{border-left:3px solid var(--yellow)}}
.kpi.bad{{border-left:3px solid var(--red)}}
.kpi.info{{border-left:3px solid var(--accent)}}
.kpi-label{{font-size:11px;font-weight:700;letter-spacing:1px;text-transform:uppercase;color:var(--text-muted);margin-bottom:8px}}
.kpi-value{{font-family:var(--mono);font-size:26px;font-weight:500;line-height:1.1}}
.kpi-unit{{font-size:13px;font-weight:400;color:var(--text-muted);margin-left:2px}}
.kpi-sub{{font-size:11px;color:var(--text-light);margin-top:5px}}
.val-good{{color:var(--green)}}.val-warn{{color:var(--yellow)}}.val-bad{{color:var(--red)}}.val-info{{color:var(--accent)}}
.two-col{{display:grid;grid-template-columns:1fr 1fr;gap:16px}}
@media(max-width:720px){{.two-col{{grid-template-columns:1fr}}}}
.card{{background:var(--surface);border:1px solid var(--border);border-radius:var(--radius);padding:20px 22px;box-shadow:var(--shadow)}}
.gauge-row{{display:flex;gap:28px;align-items:center;flex-wrap:wrap}}
.bar-chart{{display:flex;flex-direction:column;gap:9px;margin-top:4px}}
.trend-grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:14px}}
.trend{{border:1px solid var(--border);border-radius:8px;padding:10px 12px}}
.trend-head{{display:flex;justify-content:space-between;font-size:12px;color:var(--text-muted);margin-bottom:6px}}
.trend-head strong{{color:var(--text)}}
.trend svg{{width:100%;height:110px;display:block}}
.trend-axis{{display:flex;justify-content:space-between;font-size:10px;color:var(--text-muted);margin-top:4px}}
.bar-row{{display:flex;align-items:center;gap:8px}}
.bar-tag{{font-family:var(--mono);font-size:10px;color:var(--text-muted);width:28px;text-align:right;flex-shrink:0}}
.bar-track{{flex:1;height:8px;background:var(--surface2);border-radius:4px;overflow:hidden;border:1px solid var(--border)}}
.bar-fill{{height:100%;border-radius:4px}}
.bar-val{{font-family:var(--mono);font-size:11px;color:var(--text);width:64px;text-align:right;flex-shrink:0}}
.dist-bar{{height:24px;border-radius:6px;overflow:hidden;display:flex;margin-top:10px;border:1px solid var(--border)}}
.dist-seg{{height:100%}}
.dist-legend{{display:flex;gap:16px;flex-wrap:wrap;margin-top:10px}}
.dist-item{{display:flex;align-items:center;gap:5px;font-size:12px;color:var(--text-muted)}}
.dist-dot{{width:9px;height:9px;border-radius:2px;flex-shrink:0}}
.mos-badge{{display:inline-flex;align-items:center;gap:6px;padding:4px 12px;border-radius:20px;font-family:var(--mono);font-size:13px;font-weight:500}}
.mos-good{{background:var(--green-bg);color:var(--green)}}
.mos-warn{{background:var(--yellow-bg);color:var(--yellow)}}
.mos-bad{{background:var(--red-bg);color:var(--red)}}
.quality-grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px;margin-top:4px}}
.q-item{{background:var(--surface2);border:1px solid var(--border);border-radius:var(--radius);padding:14px 16px}}
.q-label{{font-size:11px;font-weight:700;letter-spacing:1px;text-transform:uppercase;color:var(--text-muted);margin-bottom:6px}}
.q-val{{font-family:var(--mono);font-size:22px;font-weight:500}}
.q-sub{{font-size:11px;color:var(--text-light);margin-top:4px}}
.rtp-meta{{display:flex;gap:24px;margin-top:14px;font-size:12px;color:var(--text-muted);flex-wrap:wrap}}
.rtp-meta strong{{color:var(--text);font-family:var(--mono)}}
.defs dl{{margin:0;font-size:13px;line-height:1.7}}
.defs dt{{font-weight:600;margin-top:10px}}
.defs dd{{margin:2px 0 0;color:var(--text-muted)}}
.data-table{{width:100%;border-collapse:collapse;font-size:13px}}
.data-table th{{text-align:left;font-size:11px;font-weight:700;letter-spacing:1px;text-transform:uppercase;color:var(--text-muted);padding:8px 14px;border-bottom:2px solid var(--border)}}
.data-table td{{padding:9px 14px;border-bottom:1px solid var(--border)}}
.data-table tbody tr:last-child td{{border-bottom:none}}
.data-table tbody tr:hover td{{background:var(--surface2)}}
.tag{{display:inline-block;padding:2px 8px;border-radius:12px;font-family:var(--mono);font-size:11px;font-weight:500}}
.tag-4{{background:var(--yellow-bg);color:var(--yellow)}}
.tag-5{{background:var(--red-bg);color:var(--red)}}
.tag-6{{background:var(--purple-bg);color:var(--purple)}}
.tag-t{{background:var(--yellow-bg);color:var(--yellow)}}
.footer{{text-align:center;font-size:11px;color:var(--text-light);padding:24px;border-top:1px solid var(--border);margin-top:8px;font-family:var(--mono)}}
</style>
</head>
<body>
<header class="header">
  <div class="logo"><div class="logo-icon">⚡</div>sipress</div>
  <span class="header-sep">/</span>
  <div class="header-info">
    伺服器 <strong>{server}</strong> &nbsp;·&nbsp; 時長 <strong>{dur:.0}s</strong> &nbsp;·&nbsp; 目標 CPS <strong>{cps:.1}</strong>
  </div>
  <div class="header-time">{ts}</div>
</header>
<main class="main">

<div>
  <div class="section-title">核心指標</div>
  <div class="kpi-grid">
    <div class="kpi {asr_cls}"><div class="kpi-label">ASR</div>
      <div class="kpi-value {asr_val}">{asr:.1}<span class="kpi-unit">%</span></div>
      <div class="kpi-sub">Answer Seizure Ratio</div></div>
    <div class="kpi {ccr_cls}"><div class="kpi-label">CCR</div>
      <div class="kpi-value {ccr_val}">{ccr:.1}<span class="kpi-unit">%</span></div>
      <div class="kpi-sub">Call Completion Rate</div></div>
    <div class="kpi info"><div class="kpi-label">實際 CPS</div>
      <div class="kpi-value val-info">{cps:.2}</div>
      <div class="kpi-sub">Calls per Second</div></div>
    <div class="kpi info"><div class="kpi-label">ACD</div>
      <div class="kpi-value val-info">{acd:.1}<span class="kpi-unit">s</span></div>
      <div class="kpi-sub">Avg Call Duration</div></div>
    <div class="kpi info"><div class="kpi-label">總發起</div>
      <div class="kpi-value">{initiated}</div>
      <div class="kpi-sub">Calls Initiated</div></div>
    <div class="kpi {fail_cls}"><div class="kpi-label">失敗合計</div>
      <div class="kpi-value {fail_val}">{fail_total}</div>
      <div class="kpi-sub">{f4}×4xx &nbsp;{f5}×5xx &nbsp;{f6}×6xx</div></div>
  </div>
</div>

<div class="two-col">
  <div class="card">
    <div class="section-title">接通率分析</div>
    <div class="gauge-row">
      {asr_arc}
      {ccr_arc}
      <div style="flex:1;display:flex;flex-direction:column;gap:8px;font-size:13px;min-width:100px">
        <div style="display:flex;justify-content:space-between"><span style="color:var(--text-muted)">接通</span><strong style="color:var(--green)">{answered}</strong></div>
        <div style="display:flex;justify-content:space-between"><span style="color:var(--text-muted)">完成</span><strong style="color:var(--accent)">{completed}</strong></div>
        <div style="display:flex;justify-content:space-between"><span style="color:var(--text-muted)">失敗</span><strong style="color:var(--red)">{failed}</strong></div>
        <div style="display:flex;justify-content:space-between"><span style="color:var(--text-muted)">逾時</span><strong style="color:var(--yellow)">{timeout}</strong></div>
      </div>
    </div>
  </div>
  <div class="card">
    <div class="section-title">通話結果分佈</div>
    {dist}
  </div>
</div>

<div class="two-col">
  <div class="card">
    <div class="section-title">PDD — 撥號後延遲</div>
    <div class="bar-chart">{pdd_bars}</div>
  </div>
  <div class="card">
    <div class="section-title">Setup Time — 通話建立</div>
    <div class="bar-chart">{setup_bars}</div>
  </div>
</div>

{rtp_section}

{timeline_section}

<div class="card">
  <div class="section-title">SIP 錯誤碼明細</div>
  <table class="data-table">
    <thead><tr><th>分類</th><th>數量</th><th>佔失敗比</th><th>常見原因</th><th>建議</th></tr></thead>
    <tbody>
      <tr><td><span class="tag tag-4">4xx</span></td><td><strong>{f4}</strong></td><td>{p4:.1}%</td>
        <td style="color:var(--text-muted)">認證失敗、號碼格式錯誤</td>
        <td style="color:var(--text-muted)">確認主叫號碼格式與認證設定</td></tr>
      <tr><td><span class="tag tag-5">5xx</span></td><td><strong>{f5}</strong></td><td>{p5:.1}%</td>
        <td style="color:var(--text-muted)">交換機內部錯誤、資源不足</td>
        <td style="color:var(--text-muted)">降低 CPS 或增加交換機容量</td></tr>
      <tr><td><span class="tag tag-6">6xx</span></td><td><strong>{f6}</strong></td><td>{p6:.1}%</td>
        <td style="color:var(--text-muted)">被叫號碼不存在、拒接</td>
        <td style="color:var(--text-muted)">確認被叫號碼範圍設定</td></tr>
      <tr><td><span class="tag tag-t">逾時</span></td><td><strong>{timeout}</strong></td><td>—</td>
        <td style="color:var(--text-muted)">網路延遲、交換機過載</td>
        <td style="color:var(--text-muted)">調整 invite_timeout 或降低並發數</td></tr>
    </tbody>
  </table>
</div>

{definitions}

</main>
<footer class="footer">sipress v{ver} &nbsp;·&nbsp; 報告產生時間 {ts} &nbsp;·&nbsp; 測試時長 {dur:.1}s</footer>
</body></html>"#,
            ts = timestamp,
            ver = env!("CARGO_PKG_VERSION"),
            server = server_addr,
            dur = r.duration_secs,
            cps = r.actual_cps,
            asr = r.asr,
            ccr = r.ccr,
            acd = r.acd_secs,
            initiated = r.calls_initiated,
            answered = r.calls_answered,
            completed = r.calls_completed,
            failed = r.calls_failed,
            timeout = r.calls_timeout,
            fail_total = fail_total,
            f4 = r.fail_4xx,
            f5 = r.fail_5xx,
            f6 = r.fail_6xx,
            asr_cls = asr_cls,
            ccr_cls = ccr_cls,
            asr_val = cls_to_val(asr_cls),
            ccr_val = cls_to_val(ccr_cls),
            fail_cls = if fail_total > 0 { "warn" } else { "good" },
            fail_val = if fail_total > 0 {
                "val-warn"
            } else {
                "val-good"
            },
            asr_arc = asr_arc,
            ccr_arc = ccr_arc,
            dist = dist,
            pdd_bars = pdd_bars,
            setup_bars = setup_bars,
            rtp_section = rtp_section,
            timeline_section = timeline_section,
            definitions = DEFINITIONS,
            p4 = pct(r.fail_4xx, fail_total),
            p5 = pct(r.fail_5xx, fail_total),
            p6 = pct(r.fail_6xx, fail_total),
        )
    }
}

// ── 分時段統計 ──────────────────────────────────────────────────

/// 分時段統計區塊：四張趨勢圖（CPS / ASR / PDD p50 / MOS）+ 每段明細表；不足兩段時不顯示
fn timeline_section(r: &FinalReport) -> String {
    let tl = &r.timeline;
    if tl.len() < 2 {
        return String::new();
    }
    let x: Vec<f64> = tl
        .iter()
        .map(|b| (b.start_secs + b.end_secs) / 2.0)
        .collect();
    let charts = [
        trend_svg(
            "CPS（每秒發起）",
            &x,
            &tl.iter().map(|b| Some(b.cps())).collect::<Vec<_>>(),
            "",
            "var(--accent)",
            None,
        ),
        trend_svg(
            "ASR %",
            &x,
            &tl.iter().map(|b| b.asr).collect::<Vec<_>>(),
            "%",
            "var(--green)",
            Some(100.0),
        ),
        trend_svg(
            "PDD p50",
            &x,
            &tl.iter().map(|b| b.pdd_p50_ms).collect::<Vec<_>>(),
            "ms",
            "var(--yellow)",
            None,
        ),
        trend_svg(
            "MOS",
            &x,
            &tl.iter().map(|b| b.mos).collect::<Vec<_>>(),
            "",
            "var(--accent)",
            Some(5.0),
        ),
    ]
    .concat();
    let opt = |v: Option<f64>, prec: usize, unit: &str| {
        v.map_or("—".to_string(), |x| format!("{:.*}{}", prec, x, unit))
    };
    let rows: String = tl
        .iter()
        .map(|b| {
            let codes: Vec<String> = b
                .fail_codes
                .iter()
                .map(|(c, n)| format!("{}×{}", c, n))
                .collect();
            format!(
                "<tr><td>{}–{}</td><td>{}</td><td>{:.2}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td style=\"color:var(--text-muted)\">{}</td></tr>",
                clock(b.start_secs),
                clock(b.end_secs),
                b.initiated,
                b.cps(),
                b.answered,
                opt(b.asr, 1, "%"),
                opt(b.pdd_p50_ms, 0, " ms"),
                opt(b.pdd_p95_ms, 0, " ms"),
                opt(b.mos, 2, ""),
                if codes.is_empty() { "—".to_string() } else { codes.join(" ") },
            )
        })
        .collect();
    format!(
        r#"<div class="card">
  <div class="section-title">分時段統計 — 每 {interval:.0} 秒一段</div>
  <div class="trend-grid">{charts}</div>
  <table class="data-table" style="margin-top:14px">
    <thead><tr><th>時段</th><th>發起</th><th>CPS</th><th>接通</th><th>ASR</th><th>PDD p50</th><th>PDD p95</th><th>MOS</th><th>失敗碼</th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</div>"#,
        interval = tl[0].duration_secs(),
    )
}

/// 單一指標的折線圖（inline SVG）；None 的時段斷開線段，`y_max` 未給時取資料最大值
fn trend_svg(
    title: &str,
    x: &[f64],
    y: &[Option<f64>],
    unit: &str,
    color: &str,
    y_max: Option<f64>,
) -> String {
    const W: f64 = 300.0;
    const H: f64 = 110.0;
    const PAD: f64 = 6.0;
    let x_max = x.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
    let data_max = y.iter().flatten().cloned().fold(0.0_f64, f64::max);
    let top = y_max.unwrap_or(data_max * 1.15).max(1e-9);
    let px = |v: f64| PAD + v / x_max * (W - 2.0 * PAD);
    let py = |v: f64| H - PAD - (v / top).clamp(0.0, 1.0) * (H - 2.0 * PAD);

    let mut lines = String::new();
    let mut dots = String::new();
    let mut seg: Vec<String> = Vec::new();
    let flush = |seg: &mut Vec<String>, lines: &mut String| {
        if seg.len() > 1 {
            lines.push_str(&format!(
                r#"<polyline fill="none" stroke="{color}" stroke-width="2" points="{}"/>"#,
                seg.join(" ")
            ));
        }
        seg.clear();
    };
    for (xi, yi) in x.iter().zip(y) {
        match yi {
            Some(v) => {
                let (cx, cy) = (px(*xi), py(*v));
                seg.push(format!("{:.1},{:.1}", cx, cy));
                dots.push_str(&format!(
                    r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="2.5" fill="{color}"><title>{t} · {v:.2}{unit}</title></circle>"#,
                    t = clock(*xi),
                ));
            }
            None => flush(&mut seg, &mut lines),
        }
    }
    flush(&mut seg, &mut lines);
    let last = y
        .iter()
        .rev()
        .flatten()
        .next()
        .map_or("—".to_string(), |v| format!("{:.1}{}", v, unit));
    format!(
        r#"<div class="trend"><div class="trend-head"><span>{title}</span><strong>{last}</strong></div>
<svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" role="img" aria-label="{title}"><line x1="{PAD}" y1="{base}" x2="{right}" y2="{base}" stroke="var(--border)"/>{lines}{dots}</svg>
<div class="trend-axis"><span>0</span><span>最高 {top:.1}{unit}</span><span>{end}</span></div></div>"#,
        base = H - PAD,
        right = W - PAD,
        top = data_max,
        end = clock(x_max),
    )
}

/// 秒數 → `h:mm:ss`（不足一小時為 `m:ss`）
fn clock(secs: f64) -> String {
    let t = secs.round() as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

// ── 輔助函式 ────────────────────────────────────────────────────

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 {
        0.0
    } else {
        a as f64 / b as f64 * 100.0
    }
}

fn kpi_cls(v: f64, good: f64, warn: f64) -> &'static str {
    if v >= good {
        "good"
    } else if v >= warn {
        "warn"
    } else {
        "bad"
    }
}
fn cls_to_val(cls: &str) -> &'static str {
    match cls {
        "good" => "val-good",
        "warn" => "val-warn",
        _ => "val-bad",
    }
}
fn ring_color(v: f64, good: f64, warn: f64) -> String {
    if v >= good {
        "#1a9e5c".into()
    } else if v >= warn {
        "#c47d00".into()
    } else {
        "#d93025".into()
    }
}

/// 掉包率：0.5% 以內正常、1% 以內注意
fn loss_val_cls(loss_pct: f64) -> &'static str {
    if loss_pct <= 0.5 {
        "val-good"
    } else if loss_pct <= 1.0 {
        "val-warn"
    } else {
        "val-bad"
    }
}

/// 報告最後的「指標說明」（與 METRICS.md 一致）
const DEFINITIONS: &str = r#"<div class="card defs">
  <div class="section-title">指標說明</div>
  <dl>
    <dt>MOS（E-Model 估算）</dt>
    <dd>ITU-T G.107 簡化版，以 G.711 加封包遺失補償計算：R = 93.2 − 掉包損傷 − Jitter 損傷，再換算成 MOS。Jitter 低於 150 ms 不扣分，所以這個值主要反映掉包。完全不掉包時最高 4.41，不會到 5。每通電話算出一個值，報告列的是每通電話的平均。</dd>
    <dt>MOS（jitter buffer 200 ms 模擬）</dt>
    <dd>模擬話機的固定 200 ms 緩衝：封包延遲比基準（最近 30 至 60 秒內最小的延遲）多出 200 ms 以上就來不及播放，視同遺失，再用同一個公式計算。這是判定用的 MOS。另有 50 ms 緩衝的版本，對 Jitter 更敏感。這是近似值，和 VoIPmonitor 等工具的數字不會完全一致。</dd>
    <dt>掉包率（整體）</dt>
    <dd>所有通話的掉包封包數加總 ÷ 應收封包數加總，依 RTP 序號計算（RFC 3550 §A.3）。不是每通電話掉包率的平均，也不是最高值；長的通話占比較大。</dd>
    <dt>Jitter</dt>
    <dd>RFC 3550 §A.8 的到達間隔變異（平滑值），每通電話取結束時的值，再依接收封包數加權平均。</dd>
    <dt>分時段的 MOS</dt>
    <dd>依 <code>--interval</code> 分段的表格與趨勢圖中，MOS 是該段每秒即時 MOS 的平均；即時 MOS 為進行中通話從接通到當下的累計值。</dd>
  </dl>
</div>"#;

fn mos_badge_cls(mos: f64) -> &'static str {
    if mos >= 4.0 {
        "mos-good"
    } else if mos >= 3.0 {
        "mos-warn"
    } else {
        "mos-bad"
    }
}
fn mos_to_val_cls(mos: f64) -> &'static str {
    if mos >= 4.0 {
        "val-good"
    } else if mos >= 3.0 {
        "val-warn"
    } else {
        "val-bad"
    }
}
fn mos_label_str(mos: f64) -> &'static str {
    match mos as u32 {
        5 | 4 => "良好 (Good)",
        3 => "普通 (Fair)",
        2 => "差 (Poor)",
        _ => "劣 (Bad)",
    }
}

/// SVG 環形圖（110×110）
fn ring_svg(ratio: f64, label: &str, color: &str) -> String {
    use std::f64::consts::PI;
    let r = 42.0_f64;
    let cx = 55.0;
    let cy = 55.0;
    let sw = 9.0;
    let a = ratio.clamp(0.0, 0.9999) * 2.0 * PI;
    let ex = cx + r * a.sin();
    let ey = cy - r * a.cos();
    let large = if a > PI { 1 } else { 0 };
    // 注意：使用 r##"..."## 避免 SVG hex 顏色中的 "#..." 提前終止 r#"..."# raw string
    format!(
        r##"<div style="display:flex;flex-direction:column;align-items:center;gap:4px">
<svg width="110" height="110" viewBox="0 0 110 110">
  <circle cx="{cx}" cy="{cy}" r="{r}" fill="none" stroke="#dde1e9" stroke-width="{sw}"/>
  <path d="M {cx} {sy} A {r} {r} 0 {large} 1 {ex:.2} {ey:.2}"
    fill="none" stroke="{color}" stroke-width="{sw}" stroke-linecap="round"/>
  <text x="{cx}" y="51" text-anchor="middle" font-family="DM Mono,monospace"
    font-size="15" font-weight="500" fill="#1a2035">{pct:.0}%</text>
  <text x="{cx}" y="66" text-anchor="middle" font-family="Lato,sans-serif"
    font-size="10" fill="#9ba3b8">{label}</text>
</svg></div>"##,
        cx = cx,
        cy = cy,
        r = r,
        sw = sw,
        sy = cy - r,
        ex = ex,
        ey = ey,
        large = large,
        color = color,
        pct = ratio * 100.0,
        label = label,
    )
}

/// 延遲長條圖
fn bar_chart(data: &[(&str, f64)]) -> String {
    let max = data
        .iter()
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let colors = ["#1a6ef5", "#c47d00", "#d93025", "#7b0000"];
    let mut out = String::new();
    for (i, (lbl, val)) in data.iter().enumerate() {
        let w = (val / max * 100.0) as u32;
        out.push_str(&format!(
            r#"<div class="bar-row">
  <span class="bar-tag">{lbl}</span>
  <div class="bar-track"><div class="bar-fill" style="width:{w}%;background:{c}"></div></div>
  <span class="bar-val">{val:.1} ms</span>
</div>"#,
            lbl = lbl,
            w = w,
            c = colors[i.min(3)],
            val = val
        ));
    }
    out
}

/// 通話分佈堆疊條
fn dist_bar(r: &FinalReport) -> String {
    let total = r.calls_initiated as f64;
    if total == 0.0 {
        return "<p style='color:var(--text-muted);font-size:13px'>無資料</p>".into();
    }
    let segs = [
        (r.calls_completed, "#1a9e5c", "完成"),
        (
            r.calls_answered.saturating_sub(r.calls_completed),
            "#1a6ef5",
            "接通中",
        ),
        (r.calls_failed, "#d93025", "失敗"),
        (r.calls_timeout, "#c47d00", "逾時"),
    ];
    let mut bar = r#"<div class="dist-bar">"#.to_string();
    let mut leg = r#"<div class="dist-legend">"#.to_string();
    for (cnt, col, lbl) in &segs {
        let w = (*cnt as f64 / total * 100.0) as u32;
        if w > 0 {
            bar.push_str(&format!(
                r#"<div class="dist-seg" style="width:{w}%;background:{c}"></div>"#,
                w = w,
                c = col
            ));
        }
        leg.push_str(&format!(
            r#"<div class="dist-item"><span class="dist-dot" style="background:{c}"></span>{lbl}&nbsp;<strong style="color:var(--text)">{cnt}</strong>&nbsp;({p:.1}%)</div>"#,
            c=col, lbl=lbl, cnt=cnt, p=*cnt as f64/total*100.0));
    }
    format!("{}</div>{}</div>", bar, leg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{DetailedStats, LiveStats};

    #[test]
    fn rtp_section_shows_judged_mos_and_precise_loss() {
        let mut r = FinalReport::build(
            &LiveStats::default().snapshot(),
            &DetailedStats::default(),
            60.0,
            None,
        );
        r.mos = Some(4.3);
        r.mos_jb200 = Some(3.9);
        r.loss_rate_pct = Some(0.46);
        r.jitter_ms = Some(12.0);
        let html = HtmlReporter::render(&r, "20261028_050000", "127.0.0.1:5060");
        assert!(
            html.contains("MOS 3.90"),
            "整體評分用 jitter buffer 200 ms 的 MOS"
        );
        assert!(html.contains("0.46<span"), "掉包率顯示到小數 2 位");
        assert!(html.contains("指標說明"));
    }
}
