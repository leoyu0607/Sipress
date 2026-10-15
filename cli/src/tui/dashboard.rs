/// ratatui 即時儀表板
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Row, Table},
    Frame, Terminal,
};
use sipress_core::stats::StatsSnapshot;
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

/// TUI 狀態（由 Engine 的 on_progress callback 更新）
#[derive(Default, Clone)]
pub struct TuiState {
    pub snapshot: StatsSnapshot,
    pub progress: f64, // 0.0 ~ 1.0
    pub elapsed_secs: f64,
    pub target_cps: f64,
    pub target_duration: u64,
}

/// 啟動 TUI（在獨立 thread 執行，透過 channel 接收更新）
pub async fn run_tui(state: Arc<Mutex<TuiState>>, mut done_rx: mpsc::Receiver<()>) -> Result<()> {
    // 初始化終端機
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    loop {
        // 繪製
        let st = state.lock().unwrap().clone();
        term.draw(|f| draw(f, &st))?;

        // 每 200ms 刷新一次
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
        }

        // 收到結束訊號
        if done_rx.try_recv().is_ok() {
            // 最後刷新一次
            let st = state.lock().unwrap().clone();
            term.draw(|f| draw(f, &st))?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            break;
        }
    }

    // 恢復終端機
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn draw(f: &mut Frame, st: &TuiState) {
    let area = f.size();

    // 垂直切分：標題 / 進度條 / 主要指標 / 延遲表
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // 標題
            Constraint::Length(3), // 進度條
            Constraint::Length(9), // 指標
            Constraint::Min(7),    // 延遲（預留）
        ])
        .split(area);

    draw_title(f, chunks[0]);
    draw_progress(f, chunks[1], st);
    draw_stats(f, chunks[2], st);
    draw_hint(f, chunks[3]);
}

fn draw_title(f: &mut Frame, area: Rect) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " ⚡ sipress ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("SIP Load Tester", Style::default().fg(Color::White)),
        Span::styled("  [q] 退出", Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
}

fn draw_progress(f: &mut Frame, area: Rect, st: &TuiState) {
    let pct = (st.progress * 100.0) as u16;
    let label = format!(
        "{:.1}s / {}s  ({} CPS 目標)",
        st.elapsed_secs, st.target_duration, st.target_cps
    );
    let gauge = Gauge::default()
        .block(Block::default().title(" 進度 ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Green))
        .percent(pct)
        .label(label);
    f.render_widget(gauge, area);
}

fn draw_stats(f: &mut Frame, area: Rect, st: &TuiState) {
    let s = &st.snapshot;

    // 先儲存 String，避免臨時值在 Row::new 後立即丟棄
    let r0 = [
        fmt_u64(s.calls_initiated),
        String::new(),
        String::new(),
        String::from("接通率 (ASR)"),
        fmt_pct(s.asr),
    ];
    let r1 = [
        fmt_u64(s.calls_answered),
        String::new(),
        String::new(),
        String::from("失敗"),
        fmt_u64(s.calls_failed),
    ];
    let r2 = [
        fmt_u64(s.calls_completed),
        String::new(),
        String::new(),
        String::from("逾時"),
        fmt_u64(s.calls_timeout),
    ];

    let rows = vec![
        Row::new(["發起", r0[0].as_str(), "", r0[3].as_str(), r0[4].as_str()]),
        Row::new(["接通", r1[0].as_str(), "", r1[3].as_str(), r1[4].as_str()]),
        Row::new(["完成", r2[0].as_str(), "", r2[3].as_str(), r2[4].as_str()]),
    ];

    let widths = [
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(2),
        Constraint::Length(14),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .block(Block::default().title(" 即時指標 ").borders(Borders::ALL))
        .header(
            Row::new(vec!["項目", "數量", "", "指標", "值"]).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        );
    f.render_widget(table, area);
}

fn draw_hint(f: &mut Frame, area: Rect) {
    let hint = Paragraph::new("壓測進行中，結果將於完成後顯示於 table/JSON/CSV 輸出")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().title(" 提示 ").borders(Borders::ALL));
    f.render_widget(hint, area);
}

// ── 格式化輔助 ───────────────────────────────────────────────────

fn fmt_u64(n: u64) -> String {
    n.to_string()
}
fn fmt_pct(v: f64) -> String {
    format!("{:.1}%", v)
}
