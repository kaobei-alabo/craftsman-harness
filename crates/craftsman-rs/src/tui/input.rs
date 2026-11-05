//! Keyboard, mouse and paste handling, voice dictation state and the
//! composer layout.

use super::app::App;
use super::modals::{Modal, handle_modal_key};
use super::terminal::{copy_selected_text, copy_to_clipboard};
use crate::config::Config;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VoiceRequest {
    Start,
    StopAndFinalize,
    CancelSegment,
    Cancel,
    Recreate,
}

#[derive(Clone, Debug)]
pub(super) struct VoiceUiState {
    pub(super) available: bool,
    pub(super) enabled: bool,
    pub(super) state: crate::voice::VoiceState,
    pub(super) level: f32,
    pub(super) tentative: String,
    pub(super) committed: String,
    pub(super) marker: Option<usize>,
    pub(super) downloaded: u64,
    pub(super) download_total: u64,
    pub(super) partials_enabled: Option<bool>,
    pub(super) segment_active: bool,
    pub(super) submit_on_finish: bool,
    pub(super) auto_submit: bool,
    pub(super) min_words_to_submit: usize,
    pub(super) model: String,
}

impl VoiceUiState {
    pub(super) fn new(cfg: &Config) -> Self {
        let config = crate::voice::VoiceConfig::from_config(cfg);
        let compiled = cfg!(all(
            feature = "voice",
            any(target_os = "macos", target_os = "linux")
        ));
        let ssh =
            std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some();
        Self {
            available: compiled && !ssh,
            enabled: config.enabled,
            state: crate::voice::VoiceState::Idle,
            level: 0.0,
            tentative: String::new(),
            committed: String::new(),
            marker: None,
            downloaded: 0,
            download_total: 0,
            partials_enabled: None,
            segment_active: false,
            submit_on_finish: false,
            auto_submit: config.auto_submit,
            min_words_to_submit: config.min_words_to_submit,
            model: config.model,
        }
    }

    pub(super) fn active(&self) -> bool {
        matches!(
            self.state,
            crate::voice::VoiceState::Initializing
                | crate::voice::VoiceState::Downloading
                | crate::voice::VoiceState::Recording
                | crate::voice::VoiceState::Finalizing
        )
    }

    pub(super) fn begin(&mut self, cursor: usize) {
        self.state = crate::voice::VoiceState::Initializing;
        self.level = 0.0;
        self.tentative.clear();
        self.committed.clear();
        self.marker = Some(cursor);
        self.downloaded = 0;
        self.download_total = 0;
        self.segment_active = false;
        self.submit_on_finish = false;
    }

    pub(super) fn finish_segment(&mut self) {
        self.tentative.clear();
        self.committed.clear();
        self.segment_active = false;
        self.level = 0.0;
    }

    pub(super) fn finish(&mut self) {
        self.state = crate::voice::VoiceState::Idle;
        self.level = 0.0;
        self.tentative.clear();
        self.committed.clear();
        self.marker = None;
        self.downloaded = 0;
        self.download_total = 0;
        self.segment_active = false;
        self.submit_on_finish = false;
    }
}

pub(super) fn handle_terminal_event(app: &mut App, event: Event) -> bool {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(app, key),
        Event::Paste(text) => {
            match app.modal.as_mut() {
                Some(Modal::Approval(modal)) if modal.feedback_mode => {
                    modal.feedback.push_str(&text)
                }
                Some(Modal::Ask(modal)) => modal.answer.push_str(&text),
                Some(Modal::Selection(_)) => {}
                Some(Modal::Setup(modal)) => {
                    modal.fields[modal.field].push_str(&text.replace(['\r', '\n'], ""));
                }
                Some(Modal::Editor(modal)) => modal.text.push_str(&text.replace('\r', "\n")),
                _ => app.insert(&text.replace('\r', "\n")),
            }
            app.dirty = true;
            false
        }
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(Modal::Selection(modal)) = app.modal.as_mut() {
                        modal.selected = modal.selected.saturating_sub(1);
                        app.dirty = true;
                    } else {
                        app.scroll_up(3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(Modal::Selection(modal)) = app.modal.as_mut() {
                        if !modal.items.is_empty() {
                            modal.selected = (modal.selected + 1).min(modal.items.len() - 1);
                        }
                        app.dirty = true;
                    } else {
                        app.scroll_down(3);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if app.modal.is_none() => {
                    let point = Position {
                        x: mouse.column,
                        y: mouse.row,
                    };
                    if app.input_area_rect.contains(point) {
                        app.clear_screen_selection();
                        app.input_dragging = true;
                        if let Some(offset) = app.grapheme_offset_at_pos(mouse.column, mouse.row) {
                            app.cursor = offset;
                            app.selection_anchor = None;
                            app.dirty = true;
                        }
                    } else {
                        app.input_dragging = false;
                        app.pending_block_click =
                            app.block_hitboxes.iter().find_map(|(row, index)| {
                                (*row == mouse.row).then_some((*index, point))
                            });
                        app.begin_screen_selection(point);
                    }
                }
                MouseEventKind::Drag(MouseButton::Left) if app.modal.is_none() => {
                    let point = Position {
                        x: mouse.column,
                        y: mouse.row,
                    };
                    if app.input_dragging {
                        if let Some(offset) = app.grapheme_offset_at_pos(mouse.column, mouse.row) {
                            if app.selection_anchor.is_none() && offset != app.cursor {
                                app.selection_anchor = Some(app.cursor);
                            }
                            app.cursor = offset;
                            app.dirty = true;
                        }
                    } else {
                        if app
                            .pending_block_click
                            .is_some_and(|(_, start)| start != point)
                        {
                            app.pending_block_click = None;
                        }
                        app.update_screen_selection(point);
                    }
                }
                MouseEventKind::Up(MouseButton::Left) if app.modal.is_none() => {
                    let point = Position {
                        x: mouse.column,
                        y: mouse.row,
                    };
                    if app.input_dragging {
                        app.input_dragging = false;
                        if app
                            .selection_anchor
                            .is_some_and(|anchor| anchor != app.cursor)
                        {
                            if let Some(text) = app.selected_input_text() {
                                match copy_selected_text(&text) {
                                    Ok(()) => app.selection_copied = true,
                                    Err(error) => {
                                        app.push_error(format!(
                                            "Could not copy selected text: {error}"
                                        ));
                                    }
                                }
                            }
                        }
                    } else if let Some((index, start)) = app.pending_block_click.take()
                        && start == point
                    {
                        app.clear_screen_selection();
                        app.toggle_block(index);
                    } else {
                        app.finish_screen_selection(point);
                        let dragged = app
                            .screen_selection
                            .is_some_and(|selection| selection.start != selection.end);
                        if dragged && let Some(text) = app.selected_screen_text() {
                            match copy_selected_text(&text) {
                                Ok(()) => app.selection_copied = true,
                                Err(error) => {
                                    app.push_error(format!(
                                        "Could not copy selected text: {error}"
                                    ));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            false
        }
        Event::Resize(_, _) => {
            app.dirty = true;
            false
        }
        _ => false,
    }
}

pub(super) fn request_voice_start(app: &mut App) {
    if !app.voice.available {
        return;
    }
    if !app.voice.enabled {
        app.push("Voice input is disabled for this session. Run /voice on.");
        return;
    }
    if app.voice.active() {
        return;
    }
    app.voice.begin(app.cursor);
    app.voice_request = Some(VoiceRequest::Start);
    app.dirty = true;
}

fn request_voice_cancel_segment(app: &mut App) {
    app.voice_request = Some(VoiceRequest::CancelSegment);
    app.voice.finish_segment();
    app.dirty = true;
}

pub(super) fn request_voice_cancel(app: &mut App) {
    app.voice_request = Some(VoiceRequest::Cancel);
    app.voice.finish();
    app.dirty = true;
}

pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    if app.modal.is_some() {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::SUPER)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            if app.voice.active() {
                request_voice_cancel(app);
            }
            if app.running {
                // The main loop must stop the real agent BEFORE a prompt
                // reply wakes its blocked tool. Do not release it here.
                app.request_turn_cancellation();
            } else {
                app.cancel_pending_prompts();
            }
            return false;
        }
        handle_modal_key(app, key);
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('g' | 'G'))
    {
        if matches!(
            app.voice.state,
            crate::voice::VoiceState::Recording | crate::voice::VoiceState::Finalizing
        ) {
            app.voice.state = crate::voice::VoiceState::Finalizing;
            app.voice_request = Some(VoiceRequest::StopAndFinalize);
        } else if app.voice.active() {
            request_voice_cancel(app);
        } else {
            request_voice_start(app);
        }
        return false;
    }
    if app.voice.active() && key.code == KeyCode::Esc {
        if app.voice.state == crate::voice::VoiceState::Recording && app.voice.segment_active {
            request_voice_cancel_segment(app);
        } else {
            request_voice_cancel(app);
        }
        return false;
    }
    if app.voice.active()
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        request_voice_cancel(app);
        return false;
    }
    if (key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::SUPER))
        && matches!(key.code, KeyCode::Char('c' | 'C'))
        && app.screen_selection.is_some()
    {
        if let Some(text) = app.selected_screen_text() {
            if let Err(error) = copy_to_clipboard(&text) {
                app.push_error(format!("Could not copy selected text: {error}"));
            } else {
                app.clear_screen_selection();
            }
        } else {
            app.clear_screen_selection();
        }
        return false;
    }
    if (key.modifiers.contains(KeyModifiers::CONTROL)
        || key.modifiers.contains(KeyModifiers::SUPER))
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        app.copy_requested = true;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        if app.running {
            app.request_turn_cancellation();
            return false;
        }
        if app.selection_anchor.is_some() {
            if let Some(text) = app.selected_input_text() {
                match copy_selected_text(&text) {
                    Ok(()) => {
                        app.selection_copied = true;
                        app.selection_anchor = None;
                        app.dirty = true;
                    }
                    Err(error) => {
                        app.push_error(format!("Could not copy selected text: {error}"));
                    }
                }
            } else {
                app.clear_input_selection();
            }
            return false;
        }
        if !app.input.is_empty() {
            app.clear_composer();
            return false;
        }
        if app
            .last_ctrl_c
            .is_some_and(|time| time.elapsed() < Duration::from_millis(1_500))
        {
            return true;
        }
        app.last_ctrl_c = Some(Instant::now());
        app.push("Press Ctrl-C again to quit");
        return false;
    }
    if matches!(key.code, KeyCode::BackTab)
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
    {
        app.mode_cycle_requested = true;
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('d') {
        return app.input.is_empty() && !app.running;
    }
    let matches = app.slash_matches();
    match key.code {
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.selection_anchor = Some(0);
            app.cursor = app.input.len();
            app.dirty = true;
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.selection_anchor = Some(0);
                app.cursor = app.input.len();
            } else {
                app.clear_input_selection();
                app.cursor = 0;
            }
            app.dirty = true;
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                app.begin_input_selection();
                app.cursor = app.input.len();
            } else {
                app.clear_input_selection();
                app.cursor = app.input.len();
            }
            app.dirty = true;
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.delete_previous_word()
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.delete_to_start()
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => app.delete_to_end(),
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(index) = app.transcript.iter().rposition(|item| item.block.is_some()) {
                app.toggle_block(index);
            }
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => app.undo(),
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => app.redo(),
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => app.insert("\n"),
        KeyCode::Char(' ')
            if key.modifiers.is_empty()
                && app.input.is_empty()
                && app.voice.available
                && app.voice.enabled
                && !app.voice.active() =>
        {
            request_voice_start(app);
        }
        KeyCode::Char(character)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            app.insert(&character.to_string());
        }
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),
        KeyCode::Left
            if key.modifiers.contains(KeyModifiers::ALT)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.begin_input_selection();
            app.cursor = app.previous_word_boundary();
            app.dirty = true;
        }
        KeyCode::Right
            if key.modifiers.contains(KeyModifiers::ALT)
                && key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            app.begin_input_selection();
            app.cursor = app.next_word_boundary();
            app.dirty = true;
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
            app.clear_input_selection();
            app.cursor = app.previous_word_boundary();
            app.dirty = true;
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
            app.clear_input_selection();
            app.cursor = app.next_word_boundary();
            app.dirty = true;
        }
        KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.begin_input_selection();
            app.cursor = app.previous_boundary();
            app.dirty = true;
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.begin_input_selection();
            app.cursor = app.next_boundary();
            app.dirty = true;
        }
        KeyCode::Left => {
            app.clear_input_selection();
            app.cursor = app.previous_boundary();
            app.dirty = true;
        }
        KeyCode::Right => {
            app.clear_input_selection();
            app.cursor = app.next_boundary();
            app.dirty = true;
        }
        KeyCode::PageUp => app.scroll_up(12),
        KeyCode::PageDown => app.scroll_down(12),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => app.history_move(true),
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => app.history_move(false),
        KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_up(3),
        KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => app.scroll_down(3),
        KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.begin_input_selection();
            app.cursor = app.line_start();
            app.dirty = true;
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => {
            app.begin_input_selection();
            app.cursor = app.line_end();
            app.dirty = true;
        }
        KeyCode::Home => {
            app.clear_input_selection();
            app.cursor = app.line_start();
            app.dirty = true;
        }
        KeyCode::End => {
            app.clear_input_selection();
            app.cursor = app.line_end();
            app.dirty = true;
        }
        KeyCode::Up if !matches.is_empty() => {
            app.slash_selected = app.slash_selected.saturating_sub(1);
            app.dirty = true;
        }
        KeyCode::Down if !matches.is_empty() => {
            app.slash_selected = (app.slash_selected + 1).min(matches.len() - 1);
            app.dirty = true;
        }
        KeyCode::Up if app.input.contains('\n') => {
            if !app.move_vertical(true) {
                app.history_move(true);
            }
        }
        KeyCode::Down if app.input.contains('\n') => {
            if !app.move_vertical(false) {
                app.history_move(false);
            }
        }
        KeyCode::Up => app.history_move(true),
        KeyCode::Down => app.history_move(false),
        KeyCode::Tab if app.running => {
            if !app.input.trim().is_empty() {
                app.queue_submission_requested = true;
                app.submit_requested = true;
                app.dirty = true;
            }
        }
        KeyCode::Tab if !matches.is_empty() => {
            let selected = &matches[app.slash_selected.min(matches.len() - 1)].0;
            app.input = format!("{selected} ");
            app.cursor = app.input.len();
            app.dirty = true;
        }
        KeyCode::Enter
            if matches!(
                app.voice.state,
                crate::voice::VoiceState::Recording | crate::voice::VoiceState::Finalizing
            ) =>
        {
            app.voice.submit_on_finish = true;
            app.voice.state = crate::voice::VoiceState::Finalizing;
            app.voice_request = Some(VoiceRequest::StopAndFinalize);
            app.dirty = true;
        }
        KeyCode::Enter if app.voice.active() => {
            app.dirty = true;
        }
        KeyCode::Enter if !matches.is_empty() => {
            let selected = &matches[app.slash_selected.min(matches.len() - 1)].0;
            app.input = selected.clone();
            app.cursor = app.input.len();
            app.submit_requested = true;
            app.dirty = true;
        }
        KeyCode::Enter => {
            app.submit_requested = true;
            app.dirty = true;
        }
        KeyCode::Esc => {
            if app.history_index.is_some() || app.history_draft.is_some() {
                app.restore_history_draft();
            } else {
                app.clear_composer();
            }
        }
        _ => {}
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InputLayout {
    rendered: String,
    pub(super) cursor_row: usize,
    pub(super) cursor_col: usize,
    pub(super) rows: usize,
    tentative_render_range: Option<(usize, usize)>,
    selection_render_range: Option<(usize, usize)>,
}

/// Soft-wrap composer text by terminal display width while retaining an exact
/// cursor location. Pre-wrapping keeps rendering and cursor math on the same
/// grapheme-aware algorithm instead of relying on a widget's separate wrapper.
#[cfg(test)]
fn layout_input(text: &str, cursor: usize, max_width: usize) -> InputLayout {
    layout_input_with_tentative(text, cursor, max_width, None, None)
}

pub(super) fn layout_input_with_tentative(
    text: &str,
    cursor: usize,
    max_width: usize,
    tentative: Option<(usize, usize)>,
    selection: Option<(usize, usize)>,
) -> InputLayout {
    let max_width = max_width.max(1);
    let mut rendered = String::new();
    let mut row = 0usize;
    let mut col = 0usize;
    let mut pending_wrap = false;
    let mut cursor_position = None;
    let mut tentative_start = None;
    let mut tentative_end = None;
    let mut selection_start = None;
    let mut selection_end = None;

    for (index, grapheme) in text.grapheme_indices(true) {
        if tentative.is_some_and(|(_, end)| index == end) {
            tentative_end = Some(rendered.len());
        }
        if selection.is_some_and(|(_, end)| index == end) {
            selection_end = Some(rendered.len());
        }
        if index == cursor {
            cursor_position = Some(if pending_wrap {
                (row + 1, 0)
            } else {
                (row, col)
            });
        }

        if grapheme == "\n" {
            if tentative.is_some_and(|(start, _)| index == start) {
                tentative_start = Some(rendered.len());
            }
            if selection.is_some_and(|(start, _)| index == start) {
                selection_start = Some(rendered.len());
            }
            rendered.push('\n');
            row += 1;
            col = 0;
            pending_wrap = false;
            continue;
        }

        if pending_wrap {
            rendered.push('\n');
            row += 1;
            col = 0;
        }

        let width = grapheme.width();
        if col > 0 && col.saturating_add(width) > max_width {
            rendered.push('\n');
            row += 1;
            col = 0;
        }
        if tentative.is_some_and(|(start, _)| index == start) {
            tentative_start = Some(rendered.len());
        }
        if selection.is_some_and(|(start, _)| index == start) {
            selection_start = Some(rendered.len());
        }
        rendered.push_str(grapheme);
        col = col.saturating_add(width).min(max_width);
        pending_wrap = col >= max_width;
    }

    if cursor == text.len() {
        cursor_position = Some(if pending_wrap {
            (row + 1, 0)
        } else {
            (row, col)
        });
    }
    if tentative.is_some_and(|(_, end)| end == text.len()) {
        tentative_end = Some(rendered.len());
    }
    if tentative.is_some_and(|(start, _)| start == text.len()) {
        tentative_start = Some(rendered.len());
    }
    if selection.is_some_and(|(_, end)| end == text.len()) {
        selection_end = Some(rendered.len());
    }
    if selection.is_some_and(|(start, _)| start == text.len()) {
        selection_start = Some(rendered.len());
    }
    if selection.is_some() && selection_start.is_some() && selection_end.is_none() {
        selection_end = Some(rendered.len());
    }
    if selection.is_some() && selection_end.is_some() && selection_start.is_none() {
        selection_start = Some(0);
    }
    let (cursor_row, cursor_col) = cursor_position.unwrap_or((row, col));
    let rows = row + 1 + usize::from(pending_wrap);
    InputLayout {
        rendered,
        cursor_row,
        cursor_col,
        rows,
        tentative_render_range: tentative_start.zip(tentative_end),
        selection_render_range: selection_start.zip(selection_end),
    }
}

pub(super) fn styled_input_text(layout: &InputLayout) -> Text<'static> {
    let ranges: [(Option<(usize, usize)>, Style); 2] = [
        (
            layout.tentative_render_range,
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        (
            layout.selection_render_range,
            Style::default().add_modifier(Modifier::REVERSED),
        ),
    ];
    if ranges.iter().all(|(range, _)| range.is_none()) {
        return Text::raw(layout.rendered.clone());
    }
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in layout.rendered.split('\n') {
        let line_start = offset;
        let line_end = line_start + line.len();
        let segments = build_styled_segments(line, line_start, line_end, &ranges);
        lines.push(Line::from(segments));
        offset = line_end + 1;
    }
    Text::from(lines)
}

fn build_styled_segments(
    line: &str,
    line_start: usize,
    line_end: usize,
    ranges: &[(Option<(usize, usize)>, Style); 2],
) -> Vec<Span<'static>> {
    let clamped: Vec<(usize, usize, Style)> = ranges
        .iter()
        .filter_map(|(range, style)| {
            range.map(|(start, end)| {
                (
                    start.clamp(line_start, line_end) - line_start,
                    end.clamp(line_start, line_end) - line_start,
                    *style,
                )
            })
        })
        .collect();
    let mut boundaries: Vec<usize> = clamped
        .iter()
        .flat_map(|(start, end, _)| [*start, *end])
        .filter(|&pos| pos > 0 && pos < line.len())
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut positions = vec![0];
    positions.extend(boundaries);
    if positions.last() != Some(&line.len()) && !line.is_empty() {
        positions.push(line.len());
    }
    let mut spans = Vec::new();
    for window in positions.windows(2) {
        let seg_start = window[0];
        let seg_end = window[1];
        if seg_start == seg_end {
            continue;
        }
        let seg_mid = line_start + (seg_start + seg_end) / 2;
        let style = clamped
            .iter()
            .rev()
            .find(|(start, end, _)| seg_mid >= line_start + *start && seg_mid < line_start + *end)
            .map(|(_, _, style)| *style)
            .unwrap_or_default();
        let text = line[seg_start..seg_end].to_string();
        if style == Style::default() {
            spans.push(Span::raw(text));
        } else {
            spans.push(Span::styled(text, style));
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::super::app::{BlockState, TranscriptKind};
    use super::super::render::ScreenSnapshot;
    use super::*;
    use ratatui::layout::Rect;

    fn fresh_app() -> App {
        let mut app = App::new(&Config::default());
        app.history.clear();
        app
    }

    #[test]
    fn clicking_block_title_toggles_it_instead_of_starting_selection() {
        let mut app = App::new(&Config::default());
        let index = app.push_collapsible_block(
            TranscriptKind::Task,
            "background agent finished",
            "details",
            BlockState::Done,
            true,
        );
        app.block_hitboxes.push((5, index));
        handle_terminal_event(
            &mut app,
            Event::Mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 4,
                row: 5,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert!(app.transcript[index].block.as_ref().unwrap().collapsed);
        handle_terminal_event(
            &mut app,
            Event::Mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 4,
                row: 5,
                modifiers: KeyModifiers::NONE,
            }),
        );
        assert!(!app.transcript[index].block.as_ref().unwrap().collapsed);
        assert!(app.screen_selection.is_none());
    }

    #[test]
    fn dragging_from_block_title_selects_instead_of_toggling() {
        let area = Rect::new(0, 0, 30, 2);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        buffer.set_string(0, 0, "tool title and output", Style::default());
        let mut app = App::new(&Config::default());
        let index = app.push_collapsible_block(
            TranscriptKind::Activity,
            "tool title",
            "details",
            BlockState::Done,
            true,
        );
        app.screen_snapshot = Some(ScreenSnapshot::from_buffer(&buffer, area));
        app.block_hitboxes.push((0, index));
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            handle_terminal_event(
                &mut app,
                Event::Mouse(crossterm::event::MouseEvent {
                    kind,
                    column: if matches!(kind, MouseEventKind::Down(_)) {
                        0
                    } else {
                        9
                    },
                    row: 0,
                    modifiers: KeyModifiers::NONE,
                }),
            );
        }
        assert!(app.transcript[index].block.as_ref().unwrap().collapsed);
        assert_eq!(app.selected_screen_text().as_deref(), Some("tool title"));
        assert!(app.selection_copied);
    }

    #[test]
    fn ctrl_c_requests_cancel_while_running() {
        let mut app = App::new(&Config::default());
        app.running = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.cancel_requested);
    }

    #[test]
    fn ctrl_g_and_empty_space_request_voice_start() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.enabled = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.voice_request, Some(VoiceRequest::Start));
        assert_eq!(app.voice.marker, Some(0));
        assert!(app.voice.active());

        request_voice_cancel(&mut app);
        app.voice_request = None;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(app.voice_request, Some(VoiceRequest::Start));
        assert!(app.input.is_empty());
    }

    #[test]
    fn escape_cancels_current_segment_then_closes_continuous_voice() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.enabled = true;
        app.voice.begin(0);
        app.voice.state = crate::voice::VoiceState::Recording;
        app.voice.segment_active = true;
        app.insert_voice_text("kept");
        app.voice.committed = "kept".into();
        app.voice.tentative = " discarded".into();
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.input, "kept");
        assert!(app.voice.tentative.is_empty());
        assert!(app.voice.active());
        assert_eq!(app.voice_request, Some(VoiceRequest::CancelSegment));

        app.voice_request = None;
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.voice_request, Some(VoiceRequest::Cancel));
        assert!(!app.voice.active());
    }

    #[test]
    fn exact_builtin_name_outranks_longer_names() {
        let mut app = App::new(&Config::default());
        app.input = "/mode".into();
        app.cursor = app.input.len();
        let matches = app.slash_matches();
        assert_eq!(matches[0].0, "/mode");
        assert_eq!(matches[1].0, "/model");
        // Enter on the fully typed name submits it as typed.
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert_eq!(app.input, "/mode");
        assert!(app.submit_requested);
    }

    #[test]
    fn extension_commands_join_the_slash_menu_and_win_exact_matches() {
        let mut app = App::new(&Config::default());
        app.extension_commands = vec![
            ("/test".into(), "Run the tests (plugin dev)".into()),
            (
                "/review".into(),
                "Review the diff (plugin review-flow)".into(),
            ),
        ];
        // "test" is also inside the built-in `/copy` description ("copy the
        // latest answer"); that used to rewrite a typed `/test` into `/copy`.
        app.input = "/test".into();
        app.cursor = app.input.len();
        let matches = app.slash_matches();
        assert_eq!(matches[0].0, "/test");
        assert!(matches.iter().any(|(name, _)| name == "/copy"));
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert_eq!(app.input, "/test");
        assert!(app.submit_requested);

        // Partial names complete to plugin commands with Tab.
        let mut app = App::new(&Config::default());
        app.extension_commands = vec![("/review".into(), "Review the diff (plugin x)".into())];
        app.input = "/rev".into();
        app.cursor = app.input.len();
        assert_eq!(app.slash_matches()[0].0, "/review");
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        ));
        assert_eq!(app.input, "/review ");
        assert!(!app.submit_requested);
    }

    #[test]
    fn a_plugin_command_that_prefixes_a_builtin_is_not_rewritten() {
        let mut app = App::new(&Config::default());
        // "/mod" is a prefix of the built-ins "/mode" and "/model", which come
        // first in the menu unless an exact name outranks them.
        app.extension_commands = vec![("/mod".into(), "Do the thing (plugin x)".into())];
        app.input = "/mod".into();
        app.cursor = app.input.len();
        assert_eq!(app.slash_matches()[0].0, "/mod");
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert_eq!(app.input, "/mod");
        assert!(app.submit_requested);
    }

    #[test]
    fn enter_accepts_selected_incomplete_slash_command() {
        let mut app = App::new(&Config::default());
        app.input = "/mo".into();
        app.cursor = app.input.len();
        assert!(!handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert_eq!(app.input, "/model");
        assert!(app.submit_requested);
    }

    #[test]
    fn composer_soft_wraps_by_display_width_and_tracks_cursor() {
        let layout = layout_input("abcdefg", 7, 4);
        assert_eq!(layout.rendered, "abcd\nefg");
        assert_eq!(
            (layout.cursor_row, layout.cursor_col, layout.rows),
            (1, 3, 2)
        );

        let middle = layout_input("abcdefg", 3, 4);
        assert_eq!((middle.cursor_row, middle.cursor_col), (0, 3));

        let unicode = layout_input("ab👍cd", "ab👍cd".len(), 4);
        assert_eq!(unicode.rendered, "ab👍\ncd");
        assert_eq!((unicode.cursor_row, unicode.cursor_col), (1, 2));

        let hard_break = layout_input("ab\ncd", 3, 4);
        assert_eq!(hard_break.rendered, "ab\ncd");
        assert_eq!((hard_break.cursor_row, hard_break.cursor_col), (1, 0));
    }

    #[test]
    fn ctrl_j_inserts_newline_without_submitting() {
        let mut app = App::new(&Config::default());
        app.input = "first".into();
        app.cursor = app.input.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.input, "first\n");
        assert!(!app.submit_requested);
    }

    #[test]
    fn running_composer_enter_steers_and_tab_queues() {
        let mut app = App::new(&Config::default());
        app.running = true;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.take_submission(), Some(("ab".into(), false)));

        app.input = "next turn".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.take_submission(), Some(("next turn".into(), true)));
    }

    #[test]
    fn running_composer_accepts_paste_and_follow_up_queue_is_fifo() {
        let mut app = App::new(&Config::default());
        app.running = true;
        handle_terminal_event(&mut app, Event::Paste("pasted".into()));
        assert_eq!(app.input, "pasted");

        app.queue_follow_up("one".into());
        app.queue_follow_up("two".into());
        assert_eq!(app.pop_queued_follow_up().as_deref(), Some("one"));
        assert_eq!(app.pop_queued_follow_up().as_deref(), Some("two"));
        assert!(app.pop_queued_follow_up().is_none());
    }

    #[test]
    fn plain_arrows_control_prompt_history_and_restore_current_draft() {
        let mut app = fresh_app();
        app.history.push("previous prompt".into());
        app.input = "current draft".into();
        app.cursor = app.input.len();

        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "previous prompt");
        assert_eq!(app.history_index, Some(0));

        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "current draft");
        assert!(app.history_index.is_none());
        assert_eq!(app.transcript_scroll_target, 0);
    }

    #[test]
    fn control_arrows_navigate_prompt_history() {
        let mut app = fresh_app();
        app.history.push("previous prompt".into());

        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
        assert_eq!(app.input, "previous prompt");
        assert_eq!(app.history_index, Some(0));

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL),
        );
        assert!(app.input.is_empty());
        assert!(app.history_index.is_none());
    }

    #[test]
    fn page_keys_scroll_transcript_without_touching_history() {
        let mut app = App::new(&Config::default());
        app.push(
            (0..30)
                .map(|index| format!("line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        handle_key(&mut app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.transcript_scroll_target, 12);
        assert_eq!(app.transcript_scroll, 12);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        assert_eq!(app.transcript_scroll_target, 0);
        assert_eq!(app.transcript_scroll, 0);
        assert!(app.history_index.is_none());
    }

    #[test]
    fn shift_right_extends_input_selection() {
        let mut app = App::new(&Config::default());
        app.insert("hello");
        app.cursor = 2;
        app.selection_anchor = None;
        handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.selection_anchor, Some(2));
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn shift_left_extends_input_selection_backward() {
        let mut app = App::new(&Config::default());
        app.insert("hello");
        app.cursor = 4;
        app.selection_anchor = None;
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(app.selection_anchor, Some(4));
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn plain_left_clears_input_selection() {
        let mut app = App::new(&Config::default());
        app.insert("hello");
        app.cursor = 3;
        app.selection_anchor = Some(1);
        handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.selection_anchor.is_none());
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn shift_home_selects_to_line_start() {
        let mut app = App::new(&Config::default());
        app.insert("hello\nworld");
        app.cursor = "hello\nwor".len();
        app.selection_anchor = None;
        handle_key(&mut app, KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
        assert!(app.selection_anchor.is_some());
        assert_eq!(app.cursor, "hello\n".len());
    }

    #[test]
    fn ctrl_c_copies_input_selection_and_clears_anchor() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 5;
        app.selection_anchor = Some(0);
        assert!(!app.running);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.selection_copied);
        assert!(app.selection_anchor.is_none());
        assert_eq!(app.input, "hello world");
    }

    #[test]
    fn ctrl_c_clears_input_when_no_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello".into();
        app.cursor = 5;
        app.selection_anchor = None;
        assert!(!app.running);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn ctrl_c_with_empty_input_selection_clears_anchor_but_keeps_input() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 5;
        app.selection_anchor = Some(5);
        assert!(!app.running);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(app.selection_anchor.is_none());
        assert!(!app.selection_copied);
        assert_eq!(app.input, "hello world");
    }

    #[test]
    fn typing_replaces_input_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 8;
        app.selection_anchor = Some(2);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE),
        );
        assert_eq!(app.input, "heXrld");
        assert!(app.selection_anchor.is_none());
    }

    #[test]
    fn backspace_deletes_input_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 8;
        app.selection_anchor = Some(2);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(app.input, "herld");
        assert!(app.selection_anchor.is_none());
    }

    #[test]
    fn delete_key_deletes_input_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 2;
        app.selection_anchor = Some(8);
        handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(app.input, "herld");
        assert!(app.selection_anchor.is_none());
    }

    #[test]
    fn super_a_selects_all_input() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 5;
        app.selection_anchor = None;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SUPER),
        );
        assert_eq!(app.selection_anchor, Some(0));
        assert_eq!(app.cursor, "hello world".len());
    }

    #[test]
    fn ctrl_shift_a_selects_all_input() {
        let mut app = App::new(&Config::default());
        app.input = "hello".into();
        app.cursor = 3;
        app.selection_anchor = None;
        handle_key(
            &mut app,
            KeyEvent::new(
                KeyCode::Char('a'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        );
        assert_eq!(app.selection_anchor, Some(0));
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn history_move_clears_input_selection() {
        let mut app = fresh_app();
        app.input = "current".into();
        app.cursor = 3;
        app.selection_anchor = Some(1);
        app.history.push("previous".into());
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL));
        assert!(app.selection_anchor.is_none());
        assert_eq!(app.input, "previous");
    }

    #[test]
    fn layout_input_with_selection_tracks_render_range() {
        let layout = layout_input_with_tentative("hello world", 5, 80, None, Some((2, 8)));
        assert_eq!(layout.selection_render_range, Some((2, 8)));
    }

    #[test]
    fn layout_input_selection_clamps_to_text_end() {
        let layout = layout_input_with_tentative("hi", 1, 80, None, Some((0, 10)));
        assert_eq!(layout.selection_render_range, Some((0, 2)));
    }

    #[test]
    fn down_at_live_position_is_noop() {
        let mut app = fresh_app();
        app.history.push("previous".into());
        app.input = "my draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "my draft");
        assert!(app.history_index.is_none());
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn up_then_down_twice_second_down_is_noop() {
        let mut app = fresh_app();
        app.history.push("old".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "old");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "draft");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "draft");
        assert!(app.history_index.is_none());
    }

    #[test]
    fn edit_recalled_entry_then_esc_restores_original_draft() {
        let mut app = fresh_app();
        app.history.push("old".into());
        app.input = "original draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "old");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(app.input, "oldx");
        assert!(app.history_index.is_none());
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.input, "original draft");
        assert!(app.history_index.is_none());
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn edit_recalled_entry_then_down_keeps_edit() {
        let mut app = fresh_app();
        app.history.push("old".into());
        app.input = "original draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(app.input, "oldx");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "oldx");
    }

    #[test]
    fn esc_at_live_position_with_no_draft_clears_input() {
        let mut app = fresh_app();
        app.input = "hello".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.input.is_empty());
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn esc_while_viewing_history_restores_draft() {
        let mut app = fresh_app();
        app.history.push("old".into());
        app.input = "my draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "old");
        assert_eq!(app.history_index, Some(0));
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.input, "my draft");
        assert!(app.history_index.is_none());
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn down_at_live_with_empty_history_is_noop() {
        let mut app = fresh_app();
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "draft");
    }

    #[test]
    fn ctrl_down_at_live_is_noop() {
        let mut app = fresh_app();
        app.history.push("old".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL),
        );
        assert_eq!(app.input, "draft");
        assert!(app.history_index.is_none());
    }

    #[test]
    fn up_down_cycle_preserves_draft_across_multiple_rounds() {
        let mut app = fresh_app();
        app.history.push("first".into());
        app.history.push("second".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "second");
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "first");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "second");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "draft");
        handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.input, "draft");
    }

    #[test]
    fn slash_command_in_history_does_not_show_menu() {
        let mut app = fresh_app();
        app.history.push("normal prompt".into());
        app.history.push("/setup".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "/setup");
        assert!(app.history_index.is_some());
        assert!(app.slash_matches().is_empty());
    }

    #[test]
    fn up_through_slash_command_continues_to_older_entries() {
        let mut app = fresh_app();
        app.history.push("oldest".into());
        app.history.push("/setup".into());
        app.history.push("newer".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "newer");
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "/setup");
        assert!(app.slash_matches().is_empty());
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "oldest");
    }

    #[test]
    fn esc_after_recalling_slash_command_restores_draft() {
        let mut app = fresh_app();
        app.history.push("/setup".into());
        app.input = "my draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "/setup");
        assert!(app.history_index.is_some());
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.input, "my draft");
        assert!(app.history_index.is_none());
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn typing_after_recalling_slash_command_shows_menu_again() {
        let mut app = fresh_app();
        app.history.push("/setup".into());
        app.input = "draft".into();
        app.cursor = app.input.len();
        handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.input, "/setup");
        assert!(app.slash_matches().is_empty());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert!(app.history_index.is_none());
        assert_eq!(app.input, "/setup ");
        assert!(app.slash_matches().is_empty());
    }
}
