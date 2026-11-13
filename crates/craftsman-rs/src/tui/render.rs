//! Drawing a frame: the transcript, splash card, subagent rows, footer and
//! screen selection.

use super::app::{App, BlockState, TranscriptItem, TranscriptKind};
use super::input::{layout_input_with_tentative, styled_input_text};
use super::modals::render_modal;
use super::session_bridge::AgentSlot;
use super::terminal::TuiBackend;
use crate::agent::Session;
use crate::config::Config;
use crate::hitl::ApprovalState;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::sync::{Arc, Mutex};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(super) const BANNER: &str = r#" ████ ████   ███  █████ █████  ████ █   █  ███  █   █
█     █   █ █   █ █       █   █     ██ ██ █   █ ██  █
█     ████  █████ ████    █    ███  █ █ █ █████ █ █ █
█     █  █  █   █ █       █       █ █   █ █   █ █  ██
 ████ █   █ █   █ █       █   ████  █   █ █   █ █   █"#;

const HAMMER: [&str; 6] = [
    "  ████████████",
    "██████████████",
    "████████████  ",
    "     ████     ",
    "     ████     ",
    "     ████     ",
];
const SMALL_HAMMER: [&str; 5] = [
    " █████████",
    "██████████",
    "████████  ",
    "   ███    ",
    "   ███    ",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScreenSelection {
    pub(super) start: Position,
    pub(super) end: Position,
}

#[derive(Clone, Debug)]
pub(super) struct ScreenSnapshot {
    area: Rect,
    pub(super) transcript_area: Rect,
    cells: Vec<String>,
}

impl ScreenSnapshot {
    pub(super) fn from_buffer(buffer: &ratatui::buffer::Buffer, transcript_area: Rect) -> Self {
        Self {
            area: buffer.area,
            transcript_area,
            cells: buffer
                .content
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect(),
        }
    }

    fn cell_symbol(&self, point: Position) -> &str {
        if !self.area.contains(point) {
            return "";
        }
        let row = point.y.saturating_sub(self.area.y) as usize;
        let column = point.x.saturating_sub(self.area.x) as usize;
        self.cells
            .get(row * self.area.width as usize + column)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub(super) fn selected_text(&self, selection: ScreenSelection) -> String {
        let Some((start, end)) = ordered_selection(selection, self.transcript_area) else {
            return String::new();
        };
        let mut lines = Vec::new();
        for y in start.y..=end.y {
            let from = if y == start.y {
                start.x
            } else {
                self.transcript_area.x
            };
            let to = if y == end.y {
                end.x
            } else {
                self.transcript_area.right().saturating_sub(1)
            };
            let mut line = String::new();
            for x in from..=to {
                line.push_str(self.cell_symbol(Position { x, y }));
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n").trim().to_string()
    }
}

pub(super) fn clamp_point_to_rect(point: Position, area: Rect) -> Option<Position> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    Some(Position {
        x: point.x.clamp(area.x, area.right().saturating_sub(1)),
        y: point.y.clamp(area.y, area.bottom().saturating_sub(1)),
    })
}

fn ordered_selection(selection: ScreenSelection, area: Rect) -> Option<(Position, Position)> {
    let start = clamp_point_to_rect(selection.start, area)?;
    let end = clamp_point_to_rect(selection.end, area)?;
    let start_index = (start.y as usize) * usize::from(u16::MAX) + start.x as usize;
    let end_index = (end.y as usize) * usize::from(u16::MAX) + end.x as usize;
    if start_index <= end_index {
        Some((start, end))
    } else {
        Some((end, start))
    }
}

#[derive(Clone)]
pub(super) struct TranscriptRenderCache {
    pub(super) revision: u64,
    pub(super) width: u16,
    pub(super) text: Text<'static>,
    pub(super) visual_lines: usize,
    pub(super) block_rows: Vec<(usize, usize)>,
}

fn truncate_visible(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let ellipsis_width = usize::from(max_width > 1);
    let target = max_width.saturating_sub(ellipsis_width);
    let mut output = String::new();
    let mut width = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if width + grapheme_width > target {
            break;
        }
        output.push_str(grapheme);
        width += grapheme_width;
    }
    if ellipsis_width > 0 {
        output.push('…');
    }
    output
}

fn splash_box_line(margin: usize, box_width: usize, content: Vec<Span<'static>>) -> Line<'static> {
    let border = Style::default().fg(Color::Indexed(141));
    let inner_width = box_width.saturating_sub(4);
    let content_width = content
        .iter()
        .map(|span| span.content.as_ref().width())
        .sum::<usize>()
        .min(inner_width);
    let mut spans = vec![Span::raw(" ".repeat(margin)), Span::styled("│ ", border)];
    spans.extend(content);
    spans.push(Span::raw(
        " ".repeat(inner_width.saturating_sub(content_width)),
    ));
    spans.push(Span::styled(" │", border));
    Line::from(spans)
}

fn splash_value_style(label: &str, value: &str) -> Style {
    if value.starts_with('(') || value.contains("not connected") || value.contains("not signed") {
        return Style::default().fg(Color::Yellow);
    }
    match label {
        "VERSION" => Style::default().fg(Color::LightMagenta),
        "AGENT" => Style::default().fg(Color::LightGreen),
        "MODEL" => Style::default().fg(Color::LightBlue),
        "SERVER" | "AUTH" => Style::default().fg(Color::Green),
        "MODE" => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::White),
    }
}

fn render_splash_card(data: &str, width: usize) -> Vec<Line<'static>> {
    if width < 4 {
        return vec![Line::raw(truncate_visible("CRAFTSMAN", width))];
    }
    let available = width;
    let box_width = if available >= 24 {
        available.saturating_sub(2).min(88)
    } else {
        available
    };
    let margin = available.saturating_sub(box_width) / 2;
    let border = Style::default().fg(Color::Indexed(141));
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let title = truncate_visible(" CRAFTSMAN CLI ", box_width.saturating_sub(2));
    let title_width = title.width();
    let rule_width = box_width.saturating_sub(title_width + 2);
    let left_rule = rule_width.min(2);
    let right_rule = rule_width.saturating_sub(left_rule);
    let mut lines = vec![Line::from(vec![
        Span::raw(" ".repeat(margin)),
        Span::styled(format!("╭{}", "─".repeat(left_rule)), border),
        Span::styled(title, title_style),
        Span::styled(format!("{}╮", "─".repeat(right_rule)), border),
    ])];

    let inner_width = box_width.saturating_sub(4);
    let banner_width = BANNER
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    let hammer_width = HAMMER[0].width();
    let full_width = hammer_width + 3 + banner_width;
    let compact = ["", "CRAFTSMAN", "", "Build your own", "WASM gadgets.", ""];
    let compact_width = hammer_width + 3 + compact.iter().map(|s| s.width()).max().unwrap_or(0);
    if inner_width >= compact_width {
        let full = inner_width >= full_width;
        let wordmark: Vec<_> = BANNER
            .lines()
            .chain(std::iter::once("A coding agent that builds its own tools."))
            .collect();
        let art_width = if full { full_width } else { compact_width };
        let left_pad = inner_width.saturating_sub(art_width) / 2;
        let hammer_colors = [141, 135, 111, 214, 208, 202];
        let wordmark_colors = [141, 135, 111, 117, 120, 245];
        for (index, hammer) in HAMMER.iter().enumerate() {
            let caption = if full {
                wordmark[index]
            } else {
                compact[index]
            };
            lines.push(splash_box_line(
                margin,
                box_width,
                vec![
                    Span::raw(" ".repeat(left_pad)),
                    Span::styled(
                        *hammer,
                        Style::default()
                            .fg(Color::Indexed(hammer_colors[index]))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("   "),
                    Span::styled(
                        caption.to_string(),
                        Style::default()
                            .fg(Color::Indexed(wordmark_colors[index]))
                            .add_modifier(Modifier::BOLD),
                    ),
                ],
            ));
        }
    } else {
        if inner_width >= SMALL_HAMMER[0].width() {
            let colors = [141, 135, 111, 214, 208];
            for (index, hammer) in SMALL_HAMMER.iter().enumerate() {
                lines.push(splash_box_line(
                    margin,
                    box_width,
                    vec![
                        Span::raw(" ".repeat((inner_width - hammer.width()) / 2)),
                        Span::styled(
                            *hammer,
                            Style::default()
                                .fg(Color::Indexed(colors[index]))
                                .add_modifier(Modifier::BOLD),
                        ),
                    ],
                ));
            }
        }
        let caption = truncate_visible("CRAFTSMAN", inner_width);
        let left_pad = inner_width.saturating_sub(caption.width()) / 2;
        lines.push(splash_box_line(
            margin,
            box_width,
            vec![
                Span::raw(" ".repeat(left_pad)),
                Span::styled(
                    caption,
                    Style::default()
                        .fg(Color::Indexed(120))
                        .add_modifier(Modifier::BOLD),
                ),
            ],
        ));
    }

    lines.push(splash_box_line(margin, box_width, Vec::new()));
    for row in data.lines() {
        let Some((label, value)) = row.split_once('\t') else {
            continue;
        };
        let label = label.to_ascii_uppercase();
        let label_display = truncate_visible(&format!("{label:<8}"), inner_width);
        let gap = usize::from(inner_width > label_display.width());
        let value_width = inner_width.saturating_sub(label_display.width() + gap);
        let value = truncate_visible(value, value_width);
        lines.push(splash_box_line(
            margin,
            box_width,
            vec![
                Span::styled(
                    label_display,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" ".repeat(gap)),
                Span::styled(value.clone(), splash_value_style(&label, &value)),
            ],
        ));
    }
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(margin)),
        Span::styled(
            format!("╰{}╯", "─".repeat(box_width.saturating_sub(2))),
            border,
        ),
    ]));
    lines
}

pub(super) fn render_transcript(items: &[TranscriptItem], width: usize) -> Text<'static> {
    let mut lines = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        if item.kind == TranscriptKind::Splash {
            lines.extend(render_splash_card(&item.text, width));
            continue;
        }
        if let Some(block) = &item.block {
            let marker = if block.collapsed { "▶" } else { "▼" };
            let (status, status_style) = match block.state {
                BlockState::Running => ("●", Style::default().fg(Color::Yellow)),
                BlockState::Done => ("✓", Style::default().fg(Color::Green)),
                BlockState::Error => ("✗", Style::default().fg(Color::Red)),
                BlockState::Waiting => ("!", Style::default().fg(Color::Yellow)),
            };
            let kind_style = if item.kind == TranscriptKind::Task {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::Yellow)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} "), kind_style),
                Span::styled(format!("{status} "), status_style),
                Span::styled(item.text.clone(), kind_style.add_modifier(Modifier::BOLD)),
            ]));
            if !block.collapsed {
                let detail_style = if block.state == BlockState::Error {
                    Style::default().fg(Color::LightRed)
                } else {
                    Style::default().fg(Color::Gray)
                };
                for detail in block.details.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("  │ ", Style::default().fg(Color::DarkGray)),
                        Span::styled(detail.to_string(), detail_style),
                    ]));
                }
            }
            continue;
        }
        let (label, label_style, body_style) = match item.kind {
            TranscriptKind::Splash => unreachable!("handled above"),
            TranscriptKind::Hint => (None, Style::default(), Style::default().fg(Color::DarkGray)),
            TranscriptKind::System => (None, Style::default(), Style::default().fg(Color::White)),
            TranscriptKind::User => (
                Some("YOU"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                Style::default(),
            ),
            TranscriptKind::Assistant => (
                Some("CRAFTSMAN"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                Style::default(),
            ),
            TranscriptKind::Activity => (
                Some("TOOL"),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Gray),
            ),
            TranscriptKind::Task => (
                Some("TASK"),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
                Style::default(),
            ),
            TranscriptKind::Error => (
                Some("ERROR"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::LightRed),
            ),
        };
        if let Some(label) = label {
            lines.push(Line::from(Span::styled(label.to_string(), label_style)));
        }
        let mut code = false;
        for raw in item.text.lines() {
            let trimmed = raw.trim_start();
            if trimmed.starts_with("```") {
                code = !code;
            }
            let style = if code {
                Style::default()
                    .fg(Color::LightCyan)
                    .bg(Color::Rgb(24, 24, 30))
            } else if trimmed.starts_with('#') {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if trimmed.starts_with("> ") {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC)
            } else {
                body_style
            };
            lines.push(Line::from(Span::styled(raw.to_string(), style)));
        }
    }
    Text::from(lines)
}

fn transcript_block_rows(items: &[TranscriptItem], width: u16) -> Vec<(usize, usize)> {
    let mut row = 0usize;
    let mut blocks = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            row += 1;
        }
        if item.block.is_some() {
            blocks.push((index, row));
        }
        let text = render_transcript(std::slice::from_ref(item), width as usize);
        row += Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .line_count(width);
    }
    blocks
}

fn running_subagent_tasks(
    registry: &craftsman_harness::AgentRegistry,
) -> Vec<craftsman_harness::AgentTaskInfo> {
    registry
        .details()
        .into_iter()
        .filter(|task| task.status == craftsman_harness::AgentStatus::Running)
        .collect()
}

pub(super) fn subagent_activity(registry: &craftsman_harness::AgentRegistry) -> Option<String> {
    let active = running_subagent_tasks(registry);
    match active.as_slice() {
        [] => None,
        [task] => Some(format!(
            "{} · {}",
            task.name,
            crate::util::one_line(&task.progress, 48)
        )),
        tasks => Some(format!("{} subagents", tasks.len())),
    }
}

fn render_subagent_progress(
    tasks: &[craftsman_harness::AgentTaskInfo],
    spinner_index: usize,
    width: usize,
    max_rows: usize,
) -> Text<'static> {
    let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][spinner_index % 10];
    let visible = tasks.len().min(max_rows);
    let mut lines = Vec::with_capacity(visible);
    for (index, task) in tasks.iter().take(visible).enumerate() {
        if index + 1 == visible && tasks.len() > visible {
            lines.push(
                Line::from(vec![
                    Span::styled(" … ", Style::default().fg(Color::Magenta)),
                    Span::styled(
                        format!(
                            "{} more delegated agents running",
                            tasks.len() - visible + 1
                        ),
                        Style::default().fg(Color::Gray),
                    ),
                ])
                .style(Style::default().bg(Color::Rgb(20, 18, 28))),
            );
            break;
        }
        let name = truncate_visible(&task.name, 24.min(width.saturating_sub(6)));
        let used = 3 + name.width() + 2;
        let progress = truncate_visible(&task.progress, width.saturating_sub(used));
        lines.push(
            Line::from(vec![
                Span::styled(
                    format!(" {spinner} "),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::styled(
                    name,
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(progress, Style::default().fg(Color::LightCyan)),
            ])
            .style(Style::default().bg(Color::Rgb(20, 18, 28))),
        );
    }
    Text::from(lines)
}

fn highlight_screen_selection(
    buffer: &mut ratatui::buffer::Buffer,
    selection: ScreenSelection,
    transcript_area: Rect,
) {
    let Some((start, end)) = ordered_selection(selection, transcript_area) else {
        return;
    };
    for y in start.y..=end.y {
        let from = if y == start.y {
            start.x
        } else {
            transcript_area.x
        };
        let to = if y == end.y {
            end.x
        } else {
            transcript_area.right().saturating_sub(1)
        };
        for x in from..=to {
            if let Some(cell) = buffer.cell_mut(Position { x, y }) {
                cell.set_style(
                    Style::default()
                        .fg(Color::White)
                        .bg(Color::Rgb(72, 65, 120)),
                );
            }
        }
    }
}

fn voice_level_meter(level: f32) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let active = (level.clamp(0.0, 1.0) * BARS.len() as f32).ceil() as usize;
    (0..6)
        .map(|index| BARS[active.saturating_sub(index + 1).min(BARS.len() - 1)])
        .collect()
}

pub(super) fn draw(
    terminal: &mut Terminal<TuiBackend>,
    app: &mut App,
    cfg: &Config,
    slot: &AgentSlot,
    approval: &Arc<Mutex<ApprovalState>>,
) -> anyhow::Result<()> {
    let selection = app.screen_selection;
    let mut rendered_transcript_area = Rect::default();
    let mut rendered_max_scroll = 0usize;
    let mut rendered_block_hitboxes = Vec::new();
    let mut rendered_input_area = Rect::default();
    let mut rendered_input_scroll = 0u16;
    let mut rendered_input_max_width = 0usize;
    let (composer_display, composer_cursor, tentative_range, selection_range) =
        app.composer_display();
    let completed = terminal.draw(|frame| {
        let area = frame.area();
        let input_layout = layout_input_with_tentative(
            &composer_display,
            composer_cursor,
            area.width.saturating_sub(2) as usize,
            tentative_range,
            selection_range,
        );
        let visible_input_rows = input_layout.rows.clamp(1, 5);
        let input_height = visible_input_rows as u16 + 2;
        let running_tasks = slot
            .peek()
            .map(|session| running_subagent_tasks(&session.registry))
            .unwrap_or_default();
        let max_task_rows = area.height.saturating_sub(input_height + 1 + 3) as usize;
        let task_height = running_tasks.len().min(max_task_rows) as u16;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(task_height),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ])
            .split(area);
        let transcript_area = chunks[0];
        rendered_transcript_area = transcript_area;
        let tasks_area = chunks[1];
        let input_area = chunks[2];
        let footer_area = chunks[3];
        rendered_input_area = input_area;
        rendered_input_max_width = input_area.width.saturating_sub(2) as usize;

        let cache_stale = app
            .transcript_render_cache
            .as_ref()
            .is_none_or(|cache| {
                cache.revision != app.transcript_revision || cache.width != transcript_area.width
            });
        if cache_stale {
            let text = render_transcript(&app.transcript, transcript_area.width as usize);
            let visual_lines = Paragraph::new(text.clone())
                .wrap(Wrap { trim: false })
                .line_count(transcript_area.width);
            let block_rows = transcript_block_rows(&app.transcript, transcript_area.width);
            app.transcript_render_cache = Some(TranscriptRenderCache {
                revision: app.transcript_revision,
                width: transcript_area.width,
                text,
                visual_lines,
                block_rows,
            });
        }
        let cache = app
            .transcript_render_cache
            .as_ref()
            .expect("transcript render cache initialized");
        let paragraph = Paragraph::new(cache.text.clone())
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::NONE));
        let max_scroll = cache
            .visual_lines
            .saturating_sub(transcript_area.height as usize);
        rendered_max_scroll = max_scroll;
        let scroll_back = app.transcript_scroll.min(max_scroll);
        let scroll = max_scroll
            .saturating_sub(scroll_back)
            .min(u16::MAX as usize) as u16;
        let scroll_top = scroll as usize;
        rendered_block_hitboxes = cache
            .block_rows
            .iter()
            .filter_map(|(index, row)| {
                if *row < scroll_top || *row >= scroll_top + transcript_area.height as usize {
                    return None;
                }
                Some((transcript_area.y + (*row - scroll_top) as u16, *index))
            })
            .collect();
        frame.render_widget(paragraph.scroll((scroll, 0)), transcript_area);

        let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"][app.spinner % 10];
        if task_height > 0 {
            frame.render_widget(
                Paragraph::new(render_subagent_progress(
                    &running_tasks,
                    app.spinner,
                    tasks_area.width as usize,
                    task_height as usize,
                )),
                tasks_area,
            );
        }
        let subagent = slot.peek().and_then(|session| subagent_activity(&session.registry));
        let title = if app.selection_copied {
            " selection copied to clipboard · drag again to replace ".to_string()
        } else if app.voice.active() {
            match app.voice.state {
                crate::voice::VoiceState::Initializing => " voice initializing… · Esc cancel ".to_string(),
                crate::voice::VoiceState::Downloading => {
                    let percent = app
                        .voice
                        .downloaded
                        .saturating_mul(100)
                        .checked_div(app.voice.download_total)
                        .unwrap_or(0);
                    format!(" downloading voice model {percent}% · Esc cancel ")
                }
                crate::voice::VoiceState::Recording => " ● listening · Ctrl+G finish · Esc cancel ".to_string(),
                crate::voice::VoiceState::Finalizing => " transcribing… · Esc cancel ".to_string(),
                _ => " voice ".to_string(),
            }
        } else if app.voice.state == crate::voice::VoiceState::Paused {
            " voice paused · /voice on to retry ".to_string()
        } else if app.running {
            let queued = if app.queued_messages.is_empty() { String::new() } else { format!(" · queued:{}", app.queued_messages.len()) };
            let activity = subagent.map(|value| format!("waiting for {value}")).unwrap_or_else(|| "working".to_string());
            if area.width < 72 {
                format!(" {spinner} working · Ctrl-C stop · Tab queue ")
            } else {
                format!(" {spinner} {activity} · Ctrl-C stop · Enter steer · Tab queue · Ctrl+J newline{queued} ")
            }
        } else if let Some(activity) = subagent {
            format!(" {spinner} background {activity} · message ")
        } else if area.width < 72 {
            format!(" message · mode:{} ", approval.lock().unwrap().mode.as_str())
        } else {
            " message · Ctrl+J newline · Shift-Tab mode ".to_string()
        };
        let input_block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(141, 115, 255)));
        let input_inner = input_block.inner(input_area);
        let input_style = Style::default();
        frame.render_widget(input_block, input_area);
        let input_scroll = input_layout
            .cursor_row
            .saturating_sub(visible_input_rows.saturating_sub(1));
        rendered_input_scroll = input_scroll as u16;
        frame.render_widget(
            Paragraph::new(styled_input_text(&input_layout))
                .style(input_style)
                .scroll((input_scroll as u16, 0)),
            input_inner,
        );

        if app.modal.is_none() {
            let cursor_row = input_layout.cursor_row.saturating_sub(input_scroll) as u16;
            frame.set_cursor_position((
                input_inner.x
                    + (input_layout.cursor_col as u16).min(input_inner.width.saturating_sub(1)),
                input_inner.y + cursor_row.min(input_inner.height.saturating_sub(1)),
            ));
        }

        let mut footer = footer(
            cfg,
            slot.peek(),
            approval,
            slot.is_pending(),
            app.transcript_scroll,
            area.width as usize,
        );
        if app.voice.state == crate::voice::VoiceState::Recording {
            footer = crate::util::one_line(
                &format!("{} · mic:{}", footer.trim_end(), voice_level_meter(app.voice.level)),
                area.width.saturating_sub(1) as usize,
            );
        }
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
            footer_area,
        );

        let matches = app.slash_matches();
        if !matches.is_empty() && app.modal.is_none() {
            let height = (matches.len().min(7) + 2) as u16;
            let popup = Rect::new(
                input_area.x + 1,
                input_area.y.saturating_sub(height),
                input_area.width.saturating_sub(2),
                height,
            );
            frame.render_widget(Clear, popup);
            let items = matches.iter().map(|(command, description)| {
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{command:<12}"), Style::default().fg(Color::Cyan)),
                    Span::raw(description.as_str()),
                ]))
            });
            let mut state = ListState::default().with_selected(Some(
                app.slash_selected.min(matches.len().saturating_sub(1)),
            ));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(" commands "))
                    .highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    )
                    .highlight_symbol("▶ "),
                popup,
                &mut state,
            );
        }

        if let Some(modal) = &app.modal {
            render_modal(frame, modal);
        }
        if let Some(selection) = selection {
            highlight_screen_selection(frame.buffer_mut(), selection, transcript_area);
        }
    })?;
    app.screen_snapshot = Some(ScreenSnapshot::from_buffer(
        completed.buffer,
        rendered_transcript_area,
    ));
    app.block_hitboxes = rendered_block_hitboxes;
    app.transcript_scroll = app.transcript_scroll.min(rendered_max_scroll);
    app.transcript_scroll_target = app.transcript_scroll_target.min(rendered_max_scroll);
    app.input_area_rect = rendered_input_area;
    app.input_scroll = rendered_input_scroll;
    app.input_max_width = rendered_input_max_width;
    Ok(())
}

fn footer(
    cfg: &Config,
    session: Option<&Session>,
    approval: &Arc<Mutex<ApprovalState>>,
    pending: bool,
    transcript_scroll: usize,
    width: usize,
) -> String {
    let model = cfg.model.as_deref().unwrap_or("(no model)");
    let model = crate::model_controls::display_effort(cfg)
        .map_or_else(|| model.to_owned(), |effort| format!("{model} ({effort})"));
    let (input, output) = session
        .map(|session| *session.usage.lock().unwrap())
        .unwrap_or_default();
    let cost = session
        .and_then(Session::cost_usd)
        .map_or_else(|| "n/a".to_string(), |value| format!("${value:.4}"));
    let mode = approval.lock().unwrap().mode.as_str();
    let route = session.map_or("starting", |session| session.route_source.as_str());
    let running = session
        .map(|session| {
            session
                .registry
                .list()
                .into_iter()
                .filter(|(_, status)| *status == craftsman_harness::AgentStatus::Running)
                .count()
        })
        .unwrap_or(0);
    let mut text = if width >= 120 {
        format!(
            " {model} · mode:{mode} · agent:{} · route:{route} · {} · {} · tokens:↑{input} ↓{output} · cost:{cost}",
            cfg.agent,
            server_status(cfg),
            auth_status(cfg)
        )
    } else if width >= 82 {
        format!(
            " {model} · {mode} · route:{route} · tokens:{} · cost:{cost}",
            input + output
        )
    } else if width >= 56 {
        format!(" {model} · {mode} · tokens:{}", input + output)
    } else {
        format!(" {model} · {mode}")
    };
    if pending && width >= 56 {
        text.push_str(" · starting");
    }
    if running > 0 && width >= 56 {
        text.push_str(&format!(" · tasks:{running}"));
    }
    if transcript_scroll > 0 && width >= 82 {
        text.push_str(&format!(" · scroll:-{transcript_scroll}"));
    }
    crate::util::one_line(&text, width.saturating_sub(1).max(1))
}

pub(super) fn server_status(cfg: &Config) -> String {
    let (endpoint, _) = connection_details(cfg);
    match reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
    {
        Some(host) => format!("srv:{host}"),
        None => "srv:not-configured".into(),
    }
}

pub(super) fn auth_status(cfg: &Config) -> String {
    let (_, status) = connection_details(cfg);
    format!("key:{status}")
}

pub(super) fn connection_details(cfg: &Config) -> (String, String) {
    // Configuration status only: rendering never resolves credentials or
    // contacts a provider, including connections that are not selected.
    let (server, auth) = if cfg.providers.is_empty() {
        let server = if cfg.base_url.is_empty() && cfg.api_key.is_empty() {
            "(not configured — run /setup)".to_string()
        } else {
            splash_endpoint(if cfg.base_url.is_empty() {
                "https://api.openai.com/v1"
            } else {
                &cfg.base_url
            })
        };
        (server, splash_key_status(&cfg.api_key))
    } else {
        match cfg
            .model
            .as_deref()
            .and_then(|model| crate::provider_config::selection(cfg, model).ok())
        {
            Some(chosen) => (
                splash_endpoint(crate::provider_config::endpoint(chosen.provider)),
                splash_key_status(&chosen.provider.api_key),
            ),
            None => (
                "(select a model — run /model)".to_string(),
                "(no model connection selected)".to_string(),
            ),
        }
    };
    (server, auth)
}

pub(super) fn startup_splash_data(cfg: &Config) -> String {
    let (server, auth) = connection_details(cfg);
    format!(
        "version\t{}\nagent\t{}\nmodel\t{}\nserver\t{}\nauth\t{}\nmode\t{}",
        env!("CARGO_PKG_VERSION"),
        cfg.agent,
        cfg.model.as_deref().unwrap_or("(none — run /model)"),
        server,
        auth,
        cfg.permission_mode.as_str(),
    )
}

fn splash_key_status(key: &str) -> String {
    if key.is_empty() {
        "(no API key — run /setup)"
    } else if key.contains("${") {
        "API key from environment (not verified)"
    } else {
        "API key configured (not verified)"
    }
    .to_string()
}

fn splash_endpoint(endpoint: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(endpoint) else {
        return "(invalid endpoint — run /setup)".to_string();
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return "(invalid endpoint — run /setup)".to_string();
    }
    // Legacy endpoints may contain credentials. Keep only the public route.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_cells_receive_visible_highlight() {
        let area = Rect::new(0, 0, 10, 2);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        buffer.set_string(0, 0, "abcdef", Style::default());
        highlight_screen_selection(
            &mut buffer,
            ScreenSelection {
                start: Position { x: 1, y: 0 },
                end: Position { x: 3, y: 0 },
            },
            area,
        );
        assert_eq!(buffer[(0, 0)].bg, Color::Reset);
        assert_eq!(buffer[(1, 0)].bg, Color::Rgb(72, 65, 120));
        assert_eq!(buffer[(3, 0)].bg, Color::Rgb(72, 65, 120));
        assert_eq!(buffer[(4, 0)].bg, Color::Reset);
    }

    #[test]
    fn running_agent_rows_track_registry_lifecycle_dynamically() {
        let registry = craftsman_harness::AgentRegistry::default();
        registry.insert("agent#0", craftsman_harness::AgentStatus::Running);
        registry.set_progress("agent#0", "thinking · round 2");
        registry.insert("agent#1", craftsman_harness::AgentStatus::Done);
        let active = running_subagent_tasks(&registry);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "agent#0");
        let rows = render_subagent_progress(&active, 2, 80, 5);
        assert_eq!(rows.lines.len(), 1);
        let plain = rows.lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(plain.contains("agent#0"));
        assert!(plain.contains("thinking · round 2"));

        registry.set_status("agent#0", craftsman_harness::AgentStatus::Done);
        assert!(running_subagent_tasks(&registry).is_empty());
    }

    #[test]
    fn splash_card_has_border_art_and_colored_metadata() {
        let data = startup_splash_data(&Config::default());
        let lines = render_splash_card(&data, 80);
        let plain = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains('╭') && plain.contains('╯'));
        assert!(plain.contains("CRAFTSMAN CLI"));
        assert!(plain.contains("██████████████"));
        assert!(plain.contains(BANNER.lines().next().unwrap()));
        let hammer = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == HAMMER[1])
            .expect("filled hammer head");
        assert_eq!(hammer.style.fg, Some(Color::Indexed(135)));
        assert!(plain.contains("VERSION"));
        let version = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(env!("CARGO_PKG_VERSION")))
            .expect("version span");
        assert_ne!(version.style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn splash_card_compacts_on_narrow_terminals() {
        let data = startup_splash_data(&Config::default());
        let lines = render_splash_card(&data, 40);
        let plain = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains("CRAFTSMAN"));
        assert!(plain.contains("██████████████"));
        assert!(plain.contains("WASM gadgets."));
        assert!(!plain.contains(BANNER.lines().next().unwrap()));
        assert!(lines.iter().all(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum::<usize>()
                <= 40
        }));
    }

    #[test]
    fn splash_card_fits_resizes_and_standard_startup_height() {
        let cfg = Config {
            model: Some("模型/long-model-name-".repeat(12)),
            agent: "long-agent-name-".repeat(12),
            ..Config::default()
        };
        let data = startup_splash_data(&cfg);
        for width in 0..=144 {
            let lines = render_splash_card(&data, width);
            assert!(
                lines.iter().all(|line| line.width() <= width),
                "width {width}"
            );
            if width >= 40 {
                assert_eq!(
                    Paragraph::new(Text::from(lines)).line_count(width as u16),
                    15
                );
            }
        }
    }

    #[test]
    fn splash_uses_selected_provider_without_resolving_any_keys() {
        let mut cfg: Config = serde_json::from_value(serde_json::json!({
            "model": "openai/gpt-test",
            "api_key": "legacy-key-must-not-be-shown",
            "base_url": "https://legacy.example/v1",
            "providers": {
                "openai": {
                    "protocol": "openai_responses",
                    "base_url": "https://gpt.example/v1",
                    "api_key": "selected-key-must-not-be-shown",
                    "models": {"gpt-test": {}}
                },
                "claude": {
                    "protocol": "anthropic_messages",
                    "api_key": "${CRAFTSMAN_SPLASH_UNRESOLVED_TEST_KEY}",
                    "models": {"claude-test": {}}
                }
            }
        }))
        .unwrap();
        let data = startup_splash_data(&cfg);
        assert!(data.contains("server\thttps://gpt.example/v1"));
        assert!(data.contains("auth\tAPI key configured (not verified)"));
        assert!(!data.contains("legacy.example"));
        assert!(!data.contains("must-not-be-shown"));
        assert!(!data.contains("CRAFTSMAN_SPLASH_UNRESOLVED_TEST_KEY"));

        cfg.model = Some("claude/claude-test".into());
        let data = startup_splash_data(&cfg);
        assert!(data.contains("server\thttps://api.anthropic.com/v1"));
        assert!(data.contains("auth\tAPI key from environment (not verified)"));
        assert!(!data.contains("CRAFTSMAN_SPLASH_UNRESOLVED_TEST_KEY"));
        assert!(!data.contains("must-not-be-shown"));
    }

    #[test]
    fn splash_reports_missing_selection_and_key_without_legacy_fallback() {
        let mut cfg: Config = serde_json::from_value(serde_json::json!({
            "api_key": "legacy-key",
            "base_url": "https://legacy.example/v1",
            "providers": {"openai": {"protocol": "openai_responses", "models": {}}}
        }))
        .unwrap();
        for model in [None, Some("missing/gpt-test".to_string())] {
            cfg.model = model;
            let data = startup_splash_data(&cfg);
            assert!(data.contains("auth\t(no model connection selected)"));
            assert!(!data.contains("legacy.example"));
        }
        cfg.model = Some("openai/gpt-test".into());
        let data = startup_splash_data(&cfg);
        assert!(data.contains("server\thttps://api.openai.com/v1"));
        assert!(data.contains("auth\t(no API key — run /setup)"));
        assert!(!data.contains("legacy-key"));
    }

    #[test]
    fn splash_legacy_connection_hides_endpoint_credentials() {
        let cfg = Config {
            model: Some("gpt-test".into()),
            base_url: "https://user:secret@gpt.example/v1?api_key=secret#secret".into(),
            api_key: "key-must-not-be-shown".into(),
            ..Config::default()
        };
        let data = startup_splash_data(&cfg);
        assert!(data.contains("server\thttps://gpt.example/v1\n"));
        assert!(data.contains("auth\tAPI key configured (not verified)"));
        assert!(!data.contains("secret"));
        assert!(!data.contains("user"));
        assert!(!data.contains("must-not-be-shown"));
        let default_endpoint = startup_splash_data(&Config {
            base_url: String::new(),
            ..cfg
        });
        assert!(default_endpoint.contains("server\thttps://api.openai.com/v1"));
        assert_eq!(
            splash_endpoint("data:credential-must-not-be-shown"),
            "(invalid endpoint — run /setup)"
        );
    }

    #[test]
    fn compact_footer_fits_narrow_terminals() {
        let cfg = Config {
            model: Some("very-long-model-name".into()),
            ..Config::default()
        };
        let approval = Arc::new(Mutex::new(ApprovalState::default()));
        let rendered = footer(&cfg, None, &approval, false, 0, 42);
        assert!(rendered.chars().count() <= 41, "{rendered}");
        assert!(rendered.contains("manual"));
    }

    #[test]
    fn wrapped_streamed_text_counts_all_visual_rows() {
        let mut app = App::new(&Config::default());
        app.running = true;
        app.append_delta("word ".repeat(80));
        let paragraph =
            Paragraph::new(render_transcript(&app.transcript, 24)).wrap(Wrap { trim: false });
        let visual = paragraph.line_count(24);
        let explicit = app
            .transcript
            .iter()
            .map(TranscriptItem::rows)
            .sum::<usize>();
        assert!(
            visual > explicit,
            "wrapped height {visual} must exceed logical {explicit}"
        );
    }

    #[test]
    fn startup_and_footer_use_the_model_connection_without_platform() {
        let cfg = Config {
            agent: "research".into(),
            model: Some("model-a".into()),
            base_url: "https://model.example/v1".into(),
            api_key: "synthetic-model-key".into(),
            platform: crate::config::PlatformConfig {
                base_url: "https://retired.example".into(),
                api_token: "retired-token".into(),
            },
            ..Config::default()
        };
        let info = startup_splash_data(&cfg);
        assert!(info.contains("agent\tresearch"));
        assert!(info.contains("https://model.example/v1"));
        assert_eq!(server_status(&cfg), "srv:model.example");
        assert_eq!(auth_status(&cfg), "key:API key configured (not verified)");
        assert!(!info.contains("retired"));
        assert!(!info.contains("synthetic-model-key"));
    }
}
