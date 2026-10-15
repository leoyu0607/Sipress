//! 端到端對照的 HTML 報告（與 [`crate::html_reporter`] 相同的淺色主題）
use crate::correlate::{CallPair, CorrelationReport, DelayStats, MatchKind};
use crate::trace::{CallOutcome, CallTrace};
use anyhow::Result;
use std::fmt::Write as _;
use std::{fs, path::PathBuf};

/// 明細表最多列出的通數（避免數十萬通時報告過大）
const MAX_ROWS: usize = 1000;

pub struct CorrelationHtml;

impl CorrelationHtml {
    pub fn save(
        r: &CorrelationReport,
        output_dir: &str,
        timestamp: &str,
        server_addr: &str,
    ) -> Result<PathBuf> {
        fs::create_dir_all(output_dir)?;
        let path = PathBuf::from(output_dir).join(format!("{}_e2e_report.html", timestamp));
        fs::write(&path, Self::render(r, timestamp, server_addr))?;
        Ok(path)
    }

    pub fn render(r: &CorrelationReport, timestamp: &str, server_addr: &str) -> String {
        // 時間以第一通民眾端 INVITE 為 0（兩端時鐘不同步時，座席端時間僅供參考）
        let t0 = r
            .pairs
            .iter()
            .map(|p| p.caller.invite_at_ms)
            .min()
            .unwrap_or(0);
        let mut out = String::with_capacity(64 * 1024);
        let _ = write!(
            out,
            r#"<!DOCTYPE html>
<html lang="zh-Hant">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>sipress 端到端對照報告 — {ts}</title>
<style>{css}</style>
</head>
<body>
<header class="header">
  <div class="logo">sipress</div>
  <span class="sep">/</span>
  <div class="info">端到端對照 &nbsp;·&nbsp; 伺服器 <strong>{server}</strong></div>
  <div class="time">{ts}</div>
</header>
<main class="main">
"#,
            ts = esc(timestamp),
            server = esc(server_addr),
            css = CSS,
        );

        // ── 核心指標 ──
        let miss_cls = if r.caller_answered == 0 {
            "info"
        } else if r.miss_rate_pct <= 1.0 {
            "good"
        } else if r.miss_rate_pct <= 5.0 {
            "warn"
        } else {
            "bad"
        };
        let _ = write!(
            out,
            r#"<section>
<div class="title">核心指標</div>
<div class="kpis">
  {k1}{k2}{k3}{k4}{k5}{k6}
</div>
</section>
"#,
            k1 = kpi(
                "info",
                "民眾端發起",
                &r.caller_calls.to_string(),
                "",
                &format!("接通 {}", r.caller_answered)
            ),
            k2 = kpi(
                "info",
                "座席接起",
                &r.agent_answered.to_string(),
                "",
                &format!("座席收到來電 {} 通", r.delivered)
            ),
            k3 = kpi(
                miss_cls,
                "漏接率",
                &format!("{:.1}", r.miss_rate_pct),
                "%",
                &format!(
                    "漏接 {} 通（其中 {} 通沒有座席收到）",
                    r.missed, r.not_delivered
                )
            ),
            k4 = kpi(
                "info",
                "分配延遲 P50",
                &fmt_ms(r.distribution.p50_ms),
                "ms",
                &format!("P95 {} ms", fmt_ms(r.distribution.p95_ms))
            ),
            k5 = kpi(
                if r.rerouted > 0 { "warn" } else { "info" },
                "ACD 改派",
                &r.rerouted.to_string(),
                "通",
                &format!("多送出 {} 個 INVITE", r.extra_offers)
            ),
            k6 = kpi(
                if r.ambiguous > 0 { "warn" } else { "good" },
                "配對",
                &(r.matched_by_cin + r.matched_by_number).to_string(),
                "通",
                &format!(
                    "P-CIN-CallId {}・主叫號碼 {}",
                    r.matched_by_cin, r.matched_by_number
                )
            ),
        );

        // ── 提醒 ──
        let mut notes = Vec::new();
        if r.caller_calls > 0 && r.caller_cin_pct < 100.0 {
            notes.push(format!(
                "民眾端只有 {:.0}% 的通話從 SIPX 回應取得 P-CIN-CallId（座席端 {:.0}%），其餘以主叫號碼 + {} 秒時間窗配對。",
                r.caller_cin_pct, r.agent_cin_pct, r.options.window_ms / 1000
            ));
        }
        if r.ambiguous > 0 {
            notes.push(format!(
                "{} 通以主叫號碼配對時時間窗內有多通同號碼的候選，配對可能錯誤。民眾端改用「每通不同主叫號碼」（CLI <code>--from-seq</code>）即可一對一配對。",
                r.ambiguous
            ));
        }
        if !r.orphans.is_empty() {
            notes.push(format!(
                "座席端有 {} 通來電找不到對應的民眾端通話（非本次壓測的來電、主叫號碼被改寫，或排隊超過時間窗）。",
                r.orphans.len()
            ));
        }
        if r.pairs
            .iter()
            .any(|p| p.distribution_ms().is_some_and(|d| d < 0))
        {
            notes.push(
                "部分分配延遲為負值：兩端時鐘不同步（不同機器請先校時），延遲數值僅供參考。".into(),
            );
        }
        if !notes.is_empty() {
            out.push_str("<section class=\"notes\">");
            for n in notes {
                let _ = write!(out, "<div class=\"note\">{}</div>", n);
            }
            out.push_str("</section>\n");
        }

        // ── 延遲與音質 ──
        let _ = write!(
            out,
            r#"<section class="two">
<div class="card">
<div class="title">排隊與分配延遲</div>
<table class="tbl">
<thead><tr><th></th><th>通數</th><th>平均</th><th>P50</th><th>P95</th><th>MAX</th></tr></thead>
<tbody>{d1}{d2}</tbody>
</table>
<div class="hint">分配延遲：民眾端送出 INVITE → 第一個座席收到 INVITE。座席接起：民眾端送出 INVITE → 座席回 200 OK（含響鈴與改派）。</div>
</div>
<div class="card">
<div class="title">雙向音質（MOS）</div>
{mos}
</div>
</section>
"#,
            d1 = delay_row("分配延遲", &r.distribution),
            d2 = delay_row("座席接起", &r.agent_answer),
            mos = mos_block(r),
        );

        // ── 漏接明細 ──
        let missed: Vec<&CallPair> = r
            .pairs
            .iter()
            .filter(|p| p.caller.outcome == CallOutcome::Answered && p.answered_offer().is_none())
            .collect();
        if !missed.is_empty() {
            let _ = write!(
                out,
                r#"<section class="card"><div class="title">漏接通話（{n}）</div>
<table class="tbl"><thead><tr><th>時間</th><th>主叫號碼</th><th>P-CIN-CallId</th><th>座席收到</th></tr></thead><tbody>"#,
                n = missed.len()
            );
            for p in missed.iter().take(MAX_ROWS) {
                let offers = if p.offers.is_empty() {
                    "<span class=\"tag bad\">無</span>".to_string()
                } else {
                    p.offers
                        .iter()
                        .map(offer_label)
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                let _ = write!(
                    out,
                    "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>",
                    rel(p.caller.invite_at_ms, t0),
                    esc(&p.caller.caller_number),
                    esc(p.caller.cin_call_id.as_deref().unwrap_or("—")),
                    offers,
                );
            }
            out.push_str("</tbody></table>");
            more_rows(&mut out, missed.len());
            out.push_str("</section>\n");
        }

        // ── 全部通話 ──
        let _ = write!(
            out,
            r#"<section class="card"><div class="title">通話明細（{n}）</div>
<table class="tbl"><thead><tr><th>時間</th><th>主叫號碼</th><th>民眾端</th><th>P-CIN-CallId</th><th>配對</th><th>座席</th><th>分配</th><th>接起</th><th>MOS 座席→民眾</th><th>MOS 民眾→座席</th></tr></thead><tbody>"#,
            n = r.pairs.len()
        );
        for p in r.pairs.iter().take(MAX_ROWS) {
            let matched = match (p.matched_by, p.ambiguous) {
                (Some(MatchKind::CinCallId), _) => "<span class=\"tag good\">CIN</span>",
                (Some(MatchKind::CallerNumber), false) => "<span class=\"tag info\">號碼</span>",
                (Some(MatchKind::CallerNumber), true) => "<span class=\"tag warn\">號碼?</span>",
                (None, _) => "—",
            };
            let _ = write!(
                out,
                "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td><td class=\"mono\">{}</td><td>{}</td><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td></tr>",
                rel(p.caller.invite_at_ms, t0),
                esc(&p.caller.caller_number),
                outcome_label(&p.caller),
                esc(p.caller.cin_call_id.as_deref().unwrap_or("—")),
                matched,
                if p.offers.is_empty() { "—".into() } else { p.offers.iter().map(offer_label).collect::<Vec<_>>().join(" ") },
                p.distribution_ms().map_or("—".into(), |v| format!("{} ms", v)),
                p.agent_answer_ms().map_or("—".into(), |v| format!("{} ms", v)),
                fmt_mos(p.caller.mos),
                fmt_mos(p.answered_offer().and_then(|o| o.mos)),
            );
        }
        out.push_str("</tbody></table>");
        more_rows(&mut out, r.pairs.len());
        out.push_str("</section>\n");

        // ── 未配對的座席來電 ──
        if !r.orphans.is_empty() {
            let _ = write!(
                out,
                r#"<section class="card"><div class="title">未配對的座席來電（{n}）</div>
<table class="tbl"><thead><tr><th>時間</th><th>座席</th><th>主叫號碼</th><th>P-CIN-CallId</th><th>結果</th></tr></thead><tbody>"#,
                n = r.orphans.len()
            );
            for o in r.orphans.iter().take(MAX_ROWS) {
                let _ = write!(
                    out,
                    "<tr><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td class=\"mono\">{}</td><td>{}</td></tr>",
                    rel(o.invite_at_ms, t0),
                    esc(o.agent.as_deref().unwrap_or("—")),
                    esc(&o.caller_number),
                    esc(o.cin_call_id.as_deref().unwrap_or("—")),
                    outcome_label(o),
                );
            }
            out.push_str("</tbody></table>");
            more_rows(&mut out, r.orphans.len());
            out.push_str("</section>\n");
        }

        out.push_str("<footer class=\"footer\">sipress · 端到端對照報告</footer>\n</main>\n</body>\n</html>\n");
        out
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

fn kpi(cls: &str, label: &str, value: &str, unit: &str, sub: &str) -> String {
    format!(
        r#"<div class="kpi {cls}"><div class="kpi-label">{label}</div><div class="kpi-value">{value}<span class="unit">{unit}</span></div><div class="kpi-sub">{sub}</div></div>"#
    )
}

fn fmt_ms(v: f64) -> String {
    format!("{:.0}", v)
}

fn fmt_mos(v: Option<f64>) -> String {
    v.map_or("—".into(), |m| format!("{:.2}", m))
}

/// 相對第一通的秒數
fn rel(ms: u64, t0: u64) -> String {
    format!("T{:+.3}s", (ms as i64 - t0 as i64) as f64 / 1000.0)
}

fn delay_row(label: &str, d: &DelayStats) -> String {
    if d.count == 0 {
        return format!(
            "<tr><td>{}</td><td class=\"mono\">0</td><td colspan=\"4\">—</td></tr>",
            label
        );
    }
    format!(
        "<tr><td>{}</td><td class=\"mono\">{}</td><td class=\"mono\">{} ms</td><td class=\"mono\">{} ms</td><td class=\"mono\">{} ms</td><td class=\"mono\">{} ms</td></tr>",
        label, d.count, fmt_ms(d.mean_ms), fmt_ms(d.p50_ms), fmt_ms(d.p95_ms), fmt_ms(d.max_ms)
    )
}

fn mos_block(r: &CorrelationReport) -> String {
    if r.mos.pairs == 0 {
        return "<div class=\"hint\">沒有兩端都量到 MOS 的通話（需兩端都啟用 RTP，且座席有接起）。</div>".into();
    }
    format!(
        r#"<table class="tbl"><thead><tr><th>方向</th><th>平均</th><th>最低</th></tr></thead><tbody>
<tr><td>座席 → 民眾（民眾端接收）</td><td class="mono">{}</td><td class="mono">{}</td></tr>
<tr><td>民眾 → 座席（座席端接收）</td><td class="mono">{}</td><td class="mono">{}</td></tr>
</tbody></table><div class="hint">共 {} 通兩端都有 MOS。</div>"#,
        fmt_mos(r.mos.caller_avg),
        fmt_mos(r.mos.caller_min),
        fmt_mos(r.mos.agent_avg),
        fmt_mos(r.mos.agent_min),
        r.mos.pairs
    )
}

fn outcome_label(t: &CallTrace) -> String {
    let code = t
        .response_code
        .map(|c| format!(" {}", c))
        .unwrap_or_default();
    match t.outcome {
        CallOutcome::Answered => "<span class=\"tag good\">接通</span>".into(),
        CallOutcome::Failed => format!("<span class=\"tag bad\">失敗{}</span>", code),
        CallOutcome::Timeout => "<span class=\"tag warn\">逾時</span>".into(),
        CallOutcome::Pending => "<span class=\"tag\">未完成</span>".into(),
    }
}

/// 座席分機 + 結果（接起 / 回應碼）
fn offer_label(o: &CallTrace) -> String {
    let ext = esc(o.agent.as_deref().unwrap_or("?"));
    match (o.outcome, o.response_code) {
        (CallOutcome::Answered, _) => format!("<span class=\"tag good\">{}</span>", ext),
        (_, Some(code)) => format!("<span class=\"tag bad\">{} {}</span>", ext, code),
        _ => format!("<span class=\"tag\">{}</span>", ext),
    }
}

fn more_rows(out: &mut String, total: usize) {
    if total > MAX_ROWS {
        let _ = write!(
            out,
            "<div class=\"hint\">只列出前 {} 通，共 {} 通（完整資料請用 JSON 輸出）。</div>",
            MAX_ROWS, total
        );
    }
}

const CSS: &str = r#"
:root{--bg:#f5f6f8;--surface:#fff;--surface2:#f0f2f5;--border:#dde1e9;--text:#1a2035;--muted:#6b7591;--light:#9ba3b8;
--accent:#1a6ef5;--green:#1a9e5c;--green-bg:#e8f7f0;--yellow:#c47d00;--yellow-bg:#fff8e6;--red:#d93025;--red-bg:#fce8e6;--blue-bg:#e8f0fe;
--mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;--sans:system-ui,-apple-system,"Segoe UI","Noto Sans TC",sans-serif}
*{box-sizing:border-box;margin:0;padding:0}
body{background:var(--bg);color:var(--text);font-family:var(--sans);font-size:14px;line-height:1.6}
.header{background:var(--surface);border-bottom:1px solid var(--border);padding:16px 32px;display:flex;align-items:center;gap:12px;flex-wrap:wrap}
.logo{font-family:var(--mono);font-size:18px;color:var(--accent);font-weight:600}
.sep{color:var(--border)}
.info{color:var(--muted);font-size:13px}.info strong{color:var(--text)}
.time{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--light);background:var(--surface2);padding:3px 10px;border-radius:20px;border:1px solid var(--border)}
.main{max-width:1200px;margin:0 auto;padding:24px 32px 48px;display:grid;gap:18px}
.title{font-size:11px;font-weight:700;letter-spacing:1.5px;color:var(--muted);margin-bottom:10px}
.kpis{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px}
.kpi{background:var(--surface);border:1px solid var(--border);border-left-width:3px;border-radius:8px;padding:14px 16px}
.kpi.good{border-left-color:var(--green)}.kpi.warn{border-left-color:var(--yellow)}.kpi.bad{border-left-color:var(--red)}.kpi.info{border-left-color:var(--accent)}
.kpi-label{font-size:11px;font-weight:700;color:var(--muted);margin-bottom:6px}
.kpi-value{font-family:var(--mono);font-size:26px;line-height:1.1}
.unit{font-size:13px;color:var(--muted);margin-left:3px}
.kpi-sub{font-size:11px;color:var(--light);margin-top:4px}
.notes{display:grid;gap:8px}
.note{background:var(--yellow-bg);border:1px solid #f0d9a8;color:#6b4a00;border-radius:8px;padding:10px 14px;font-size:13px}
.note code{font-family:var(--mono);background:#fff3d6;padding:0 4px;border-radius:3px}
.two{display:grid;grid-template-columns:1fr 1fr;gap:16px}
@media(max-width:760px){.two{grid-template-columns:1fr}.main{padding:16px}}
.card{background:var(--surface);border:1px solid var(--border);border-radius:8px;padding:16px 18px;overflow-x:auto}
.tbl{width:100%;border-collapse:collapse;font-size:13px}
.tbl th{text-align:left;font-size:11px;color:var(--muted);padding:6px 10px;border-bottom:2px solid var(--border);white-space:nowrap}
.tbl td{padding:6px 10px;border-bottom:1px solid var(--border);white-space:nowrap}
.mono{font-family:var(--mono);font-size:12px}
.hint{font-size:12px;color:var(--muted);margin-top:8px}
.tag{display:inline-block;padding:1px 8px;border-radius:10px;font-family:var(--mono);font-size:11px;background:var(--surface2);color:var(--muted)}
.tag.good{background:var(--green-bg);color:var(--green)}.tag.bad{background:var(--red-bg);color:var(--red)}
.tag.warn{background:var(--yellow-bg);color:var(--yellow)}.tag.info{background:var(--blue-bg);color:var(--accent)}
.footer{text-align:center;font-size:11px;color:var(--light);padding:16px;font-family:var(--mono)}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::correlate::{correlate, CorrelateOptions};

    fn t(
        id: &str,
        cin: Option<&str>,
        num: &str,
        at: u64,
        outcome: CallOutcome,
        agent: Option<&str>,
    ) -> CallTrace {
        CallTrace {
            sip_call_id: id.into(),
            cin_call_id: cin.map(Into::into),
            caller_number: num.into(),
            agent: agent.map(Into::into),
            invite_at_ms: at,
            answered_at_ms: (outcome == CallOutcome::Answered).then_some(at + 200),
            outcome,
            response_code: Some(if outcome == CallOutcome::Answered {
                200
            } else {
                486
            }),
            ..Default::default()
        }
    }

    #[test]
    fn renders_summary_tables_and_escapes_values() {
        let callers = [
            t(
                "c1",
                Some("C<1>"),
                "0912",
                1_000,
                CallOutcome::Answered,
                None,
            ),
            t("c2", None, "<script>", 2_000, CallOutcome::Answered, None),
        ];
        let agents = [
            t(
                "a1",
                Some("C<1>"),
                "0912",
                1_150,
                CallOutcome::Answered,
                Some("1001"),
            ),
            t("a9", None, "0999", 5_000, CallOutcome::Failed, Some("1002")),
        ];
        let r = correlate(&callers, &agents, CorrelateOptions::default());
        let html = CorrelationHtml::render(&r, "20261015_120000", "10.0.0.1:5060");
        assert!(html.contains("端到端對照"));
        assert!(html.contains("漏接通話（1）"));
        assert!(html.contains("未配對的座席來電（1）"));
        assert!(html.contains("150 ms"));
        assert!(html.contains("T+1.000s"));
        assert!(!html.contains("<script>"), "外部來的字串必須跳脫");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("C&lt;1&gt;"));
        assert!(html.contains("P-CIN-CallId"), "民眾端缺 CIN 時應提醒");
    }

    #[test]
    fn empty_report_renders() {
        let r = correlate(&[], &[], CorrelateOptions::default());
        let html = CorrelationHtml::render(&r, "ts", "s");
        assert!(html.contains("通話明細（0）"));
        assert!(html.ends_with("</html>\n"));
    }
}
