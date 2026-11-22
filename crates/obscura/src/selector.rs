//! The first screen: pick a platform, then hand over to its full TUI.

use std::io::stdout;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use obscura_core::theme;
use obscura_core::tui::TerminalGuard;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;
use ratatui::Terminal;

const TICK: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Discord,
    Instagram,
}

struct Row {
    platform: Platform,
    name: &'static str,
    tagline: &'static str,
    configured: bool,
    hint: &'static str,
}

enum Choice {
    Quit,
    Open(Platform),
}

pub async fn run() -> Result<()> {
    let discord = obscura_discord::config::Config::from_env();
    let instagram = obscura_instagram::config::Config::from_env();

    let rows = [
        Row {
            platform: Platform::Discord,
            name: "Discord",
            tagline: "DMs, group DMs and guild channels",
            configured: !discord.token.trim().is_empty(),
            hint: "set DISCORD_TOKEN in .env",
        },
        Row {
            platform: Platform::Instagram,
            name: "Instagram",
            tagline: "posts, DMs, reactions, likes, saved, comments, archived, reposts",
            configured: !instagram.sessionid.trim().is_empty(),
            hint: "set INSTAGRAM_SESSIONID in .env",
        },
    ];

    let mut selected = rows.iter().position(|row| row.configured).unwrap_or(0);
    let mut notice: Option<String> = None;

    loop {
        let choice = {
            let mut guard = TerminalGuard::enter()?;
            let backend = CrosstermBackend::new(stdout());
            let mut terminal = Terminal::new(backend).context("create terminal")?;
            terminal.clear().ok();
            let choice = pick(&mut terminal, &rows, &mut selected, notice.take())?;
            terminal.show_cursor().ok();
            guard.restore();
            choice
        };

        match choice {
            Choice::Quit => return Ok(()),
            Choice::Open(Platform::Discord) => {
                if let Err(error) = obscura_discord::tui::run(discord.clone()).await {
                    notice = Some(format!("Discord: {error:#}"));
                }
            }
            Choice::Open(Platform::Instagram) => {
                if let Err(error) = obscura_instagram::tui::run(instagram.clone()).await {
                    notice = Some(format!("Instagram: {error:#}"));
                }
            }
        }
    }
}

fn pick(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    rows: &[Row],
    selected: &mut usize,
    notice: Option<String>,
) -> Result<Choice> {
    loop {
        terminal
            .draw(|frame| draw(frame, rows, *selected, notice.as_deref()))
            .context("render the platform picker")?;

        if !event::poll(TICK)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(Choice::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Choice::Quit);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *selected = selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *selected = (*selected + 1).min(rows.len() - 1);
            }
            KeyCode::Char('1') => return Ok(Choice::Open(Platform::Discord)),
            KeyCode::Char('2') => return Ok(Choice::Open(Platform::Instagram)),
            KeyCode::Enter => {
                if let Some(row) = rows.get(*selected) {
                    return Ok(Choice::Open(row.platform));
                }
            }
            _ => {}
        }
    }
}

fn draw(frame: &mut Frame, rows: &[Row], selected: usize, notice: Option<&str>) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme::base()), area);

    let parts = ratatui::layout::Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(area);

    draw_banner(frame, parts[0]);

    let list = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title_style(theme::dim().add_modifier(Modifier::BOLD))
        .title(" choose a platform ")
        .padding(Padding::horizontal(1));
    let inner = list.inner(parts[1]);
    frame.render_widget(list, parts[1]);

    let row_height = 3u16;
    for (index, row) in rows.iter().enumerate() {
        let y = inner.y + index as u16 * row_height;
        if y + row_height > inner.y + inner.height {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, row_height);
        draw_row(frame, rect, row, index + 1, index == selected);
    }

    draw_footer(frame, parts[2], notice);
}

fn draw_banner(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled("obscura", theme::accent().add_modifier(Modifier::BOLD)),
        Span::styled("  mass archive & delete, one terminal", theme::dim()),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::new().padding(Padding::new(1, 1, 1, 0))),
        area,
    );
}

fn draw_row(frame: &mut Frame, area: Rect, row: &Row, number: usize, is_selected: bool) {
    let background = if is_selected {
        theme::selected()
    } else {
        theme::base()
    };
    let accent = if is_selected {
        theme::accent().add_modifier(Modifier::BOLD)
    } else {
        theme::dim()
    };

    let status = if row.configured {
        Span::styled("ready", theme::ok())
    } else {
        Span::styled("not configured", theme::warn())
    };

    let title = Line::from(vec![
        Span::styled(format!("[{number}] "), accent),
        Span::styled(
            row.name.to_string(),
            if is_selected {
                theme::strong()
            } else {
                theme::base()
            },
        ),
        Span::raw("  "),
        status,
    ]);

    let detail = if row.configured {
        row.tagline.to_string()
    } else {
        format!("{}  ·  {}", row.tagline, row.hint)
    };

    let lines = vec![
        title,
        Line::from(Span::styled(detail, theme::dim())),
        Line::from(""),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(background)
            .block(Block::new().padding(Padding::horizontal(1))),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, notice: Option<&str>) {
    let hints = Line::from(vec![
        Span::styled("[↑/↓]", theme::key()),
        Span::styled(" choose  ", theme::dim()),
        Span::styled("[⏎]", theme::key()),
        Span::styled(" open  ", theme::dim()),
        Span::styled("[q]", theme::key()),
        Span::styled(" quit  ", theme::dim()),
        Span::styled("archives: data/<platform>/history", theme::dim()),
    ]);
    let areas = ratatui::layout::Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    frame.render_widget(Paragraph::new(hints), areas[0]);
    if let Some(notice) = notice {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                notice,
                Style::default().fg(theme::ERR),
            )))
            .wrap(Wrap { trim: false }),
            areas[1],
        );
    }
}
