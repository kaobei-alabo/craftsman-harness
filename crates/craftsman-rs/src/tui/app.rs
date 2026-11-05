//! The application state (`App`), the transcript model and the main loop
//! (`run`) of the interactive terminal client.

use super::commands::{
    SLASH_COMMANDS, SubmissionRoute, action_status, display_submission, handle_app_action,
    handle_slash, live_command, submission_route,
};
use super::input::{VoiceRequest, VoiceUiState, handle_terminal_event};
use super::modals::{AppAction, Modal, show_setup_modal};
use super::render::{
    BANNER, ScreenSelection, ScreenSnapshot, TranscriptRenderCache, clamp_point_to_rect, draw,
    startup_splash_data, subagent_activity,
};
use super::session_bridge::{
    AgentSlot, UiBridge, UiEvent, apply_turn_cancellation, drain_notifications,
    handle_ui_event_batch, hydrate_transcript,
};
use super::terminal::{SafeBackend, TerminalGuard, copy_to_clipboard};
use crate::agent::{self, Session};
use crate::config::Config;
use crate::hitl::ApprovalState;
use craftsman_harness::TurnResult;
use crossterm::event::EventStream;
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::{Block, Borders};
use std::collections::{HashMap, VecDeque};
use std::io::{IsTerminal, stdout};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TranscriptKind {
    Splash,
    Hint,
    System,
    User,
    Assistant,
    Activity,
    Task,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockState {
    Running,
    Done,
    Error,
    Waiting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CollapsibleBlock {
    pub(super) details: String,
    pub(super) collapsed: bool,
    pub(super) state: BlockState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TranscriptItem {
    pub(super) kind: TranscriptKind,
    pub(super) text: String,
    pub(super) block: Option<CollapsibleBlock>,
}

impl TranscriptItem {
    fn new(kind: TranscriptKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            block: None,
        }
    }

    fn collapsible(
        kind: TranscriptKind,
        title: impl Into<String>,
        details: impl Into<String>,
        state: BlockState,
        collapsed: bool,
    ) -> Self {
        Self {
            kind,
            text: title.into(),
            block: Some(CollapsibleBlock {
                details: details.into(),
                collapsed,
                state,
            }),
        }
    }

    pub(super) fn rows(&self) -> usize {
        match (&self.kind, &self.block) {
            (TranscriptKind::Splash, _) => BANNER.lines().count() + self.text.lines().count() + 4,
            (_, Some(block)) if !block.collapsed => {
                self.text.lines().count().max(1) + block.details.lines().count().max(1) + 2
            }
            _ => self.text.lines().count().max(1) + 1,
        }
    }
}

type ComposerDisplay = (
    String,
    usize,
    Option<(usize, usize)>,
    Option<(usize, usize)>,
);

pub(super) struct App {
    pub(super) transcript: Vec<TranscriptItem>,
    pub(super) input: String,
    pub(super) cursor: usize,
    pub(super) selection_anchor: Option<usize>,
    pub(super) history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) history_draft: Option<String>,
    pub(super) slash_selected: usize,
    /// Plugin commands offered in the slash menu as `("/name", "description")`.
    /// Re-read whenever a new "/…" line is started (see the main loop), so
    /// plugin edits show up without a restart.
    pub(super) extension_commands: Vec<(String, String)>,
    slash_active: bool,
    pub(super) running: bool,
    pub(super) active_response: Option<usize>,
    pub(super) modal: Option<Modal>,
    pub(super) modal_queue: VecDeque<Modal>,
    pub(super) dirty: bool,
    pub(super) last_ctrl_c: Option<Instant>,
    pub(super) spinner: usize,
    pub(super) submit_requested: bool,
    pub(super) queue_submission_requested: bool,
    pub(super) queued_messages: VecDeque<String>,
    pub(super) pending_action: Option<AppAction>,
    pub(super) transcript_scroll: usize,
    pub(super) transcript_scroll_target: usize,
    pub(super) cancel_requested: bool,
    pub(super) mode_cycle_requested: bool,
    pub(super) copy_requested: bool,
    pub(super) discard_stream_deltas: bool,
    undo_stack: Vec<(String, usize)>,
    redo_stack: Vec<(String, usize)>,
    pub(super) screen_snapshot: Option<ScreenSnapshot>,
    pub(super) screen_selection: Option<ScreenSelection>,
    selection_dragging: bool,
    pub(super) pending_block_click: Option<(usize, Position)>,
    pub(super) selection_copied: bool,
    pub(super) input_area_rect: Rect,
    pub(super) input_scroll: u16,
    pub(super) input_max_width: usize,
    pub(super) input_dragging: bool,
    pub(super) transcript_revision: u64,
    pub(super) transcript_render_cache: Option<TranscriptRenderCache>,
    pub(super) tool_blocks: HashMap<String, usize>,
    pub(super) block_hitboxes: Vec<(u16, usize)>,
    pub(super) voice: VoiceUiState,
    pub(super) voice_request: Option<VoiceRequest>,
}

impl App {
    pub(super) fn new(cfg: &Config) -> Self {
        let voice = VoiceUiState::new(cfg);
        let hint = if voice.available && voice.enabled {
            "Drag across chat text to copy · Ctrl-C copies again · Ctrl+G voice · ↑/↓ prompt/history · trackpad scrolls chat"
        } else {
            "Drag across chat text to copy · Ctrl-C copies again · ↑/↓ prompt/history · trackpad scrolls chat"
        };
        Self {
            transcript: vec![
                TranscriptItem::new(TranscriptKind::Splash, startup_splash_data(cfg)),
                TranscriptItem::new(TranscriptKind::Hint, hint),
            ],
            input: String::new(),
            cursor: 0,
            selection_anchor: None,
            history: crate::history::load(),
            history_index: None,
            history_draft: None,
            slash_selected: 0,
            extension_commands: Vec::new(),
            slash_active: false,
            running: false,
            active_response: None,
            modal: None,
            modal_queue: VecDeque::new(),
            dirty: true,
            last_ctrl_c: None,
            spinner: 0,
            submit_requested: false,
            queue_submission_requested: false,
            queued_messages: VecDeque::new(),
            pending_action: None,
            transcript_scroll: 0,
            transcript_scroll_target: 0,
            cancel_requested: false,
            mode_cycle_requested: false,
            copy_requested: false,
            discard_stream_deltas: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            screen_snapshot: None,
            screen_selection: None,
            selection_dragging: false,
            pending_block_click: None,
            selection_copied: false,
            input_area_rect: Rect::default(),
            input_scroll: 0,
            input_max_width: 0,
            input_dragging: false,
            transcript_revision: 0,
            transcript_render_cache: None,
            tool_blocks: HashMap::new(),
            block_hitboxes: Vec::new(),
            voice,
            voice_request: None,
        }
    }

    fn invalidate_transcript_render(&mut self) {
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript_render_cache = None;
    }

    pub(super) fn clear_screen_selection(&mut self) {
        self.pending_block_click = None;
        self.selection_copied = false;
        if self.screen_selection.take().is_some() || self.selection_dragging {
            self.selection_dragging = false;
            self.dirty = true;
        }
        self.clear_input_selection();
    }

    pub(super) fn clear_input_selection(&mut self) {
        if self.selection_anchor.take().is_some() {
            self.dirty = true;
        }
    }

    fn input_selection_range(&self) -> Option<(usize, usize)> {
        self.selection_anchor.map(|anchor| {
            if anchor <= self.cursor {
                (anchor, self.cursor)
            } else {
                (self.cursor, anchor)
            }
        })
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.input_selection_range() {
            self.record_undo();
            self.adjust_voice_marker(start, end, 0);
            self.input.replace_range(start..end, "");
            self.cursor = start;
            self.selection_anchor = None;
            self.history_index = None;
            self.slash_selected = 0;
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_input_selection(&mut self) {
        if self.selection_anchor.is_none() {
            self.selection_anchor = Some(self.cursor);
        }
    }

    pub(super) fn selected_input_text(&self) -> Option<String> {
        let (start, end) = self.input_selection_range()?;
        let text = self.input[start..end].to_string();
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn grapheme_offset_at_pos(&self, col: u16, row: u16) -> Option<usize> {
        let input_inner = self.input_block_inner();
        let local_row = row.saturating_sub(input_inner.y) as usize + self.input_scroll as usize;
        let local_col = col.saturating_sub(input_inner.x) as usize;
        let max_width = self.input_max_width.max(1);
        let mut current_row = 0usize;
        let mut current_col = 0usize;
        let mut pending_wrap = false;
        for (index, grapheme) in self.input.grapheme_indices(true) {
            if grapheme == "\n" {
                if current_row == local_row && local_col <= current_col {
                    return Some(index);
                }
                current_row += 1;
                current_col = 0;
                pending_wrap = false;
                continue;
            }
            if pending_wrap {
                if current_row == local_row && local_col <= current_col {
                    return Some(index);
                }
                current_row += 1;
                current_col = 0;
            }
            let width = grapheme.width();
            if current_col > 0 && current_col.saturating_add(width) > max_width {
                if current_row == local_row && local_col <= current_col {
                    return Some(index);
                }
                current_row += 1;
                current_col = 0;
            }
            if current_row == local_row && local_col <= current_col {
                return Some(index);
            }
            if current_row > local_row {
                return Some(index);
            }
            current_col = current_col.saturating_add(width).min(max_width);
            if current_row == local_row && local_col <= current_col {
                return Some(index + grapheme.len());
            }
            pending_wrap = current_col >= max_width;
        }
        if current_row < local_row {
            return Some(self.input.len());
        }
        if current_row == local_row && local_col >= current_col {
            return Some(self.input.len());
        }
        None
    }

    fn input_block_inner(&self) -> Rect {
        Block::default()
            .borders(Borders::ALL)
            .inner(self.input_area_rect)
    }

    pub(super) fn begin_screen_selection(&mut self, point: Position) {
        let Some(snapshot) = &self.screen_snapshot else {
            return;
        };
        if !snapshot.transcript_area.contains(point) {
            self.clear_screen_selection();
            return;
        }
        self.screen_selection = Some(ScreenSelection {
            start: point,
            end: point,
        });
        self.selection_copied = false;
        self.selection_dragging = true;
        self.dirty = true;
    }

    pub(super) fn update_screen_selection(&mut self, point: Position) {
        if !self.selection_dragging {
            return;
        }
        let Some(snapshot) = &self.screen_snapshot else {
            return;
        };
        let Some(end) = clamp_point_to_rect(point, snapshot.transcript_area) else {
            return;
        };
        if let Some(selection) = self.screen_selection.as_mut() {
            selection.end = end;
            self.dirty = true;
        }
    }

    pub(super) fn finish_screen_selection(&mut self, point: Position) {
        self.update_screen_selection(point);
        self.selection_dragging = false;
        self.dirty = true;
    }

    pub(super) fn selected_screen_text(&self) -> Option<String> {
        let snapshot = self.screen_snapshot.as_ref()?;
        let selection = self.screen_selection?;
        let text = snapshot.selected_text(selection);
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn enqueue_modal(&mut self, modal: Modal) {
        // A prompt may already be in the event channel when Ctrl-C arrives.
        // Keep releasing waiters until this turn finishes, including after the
        // one-shot cancel_requested flag has been consumed by the main loop.
        if self.discard_stream_deltas && modal.cancel_prompt() {
            return;
        }
        self.clear_screen_selection();
        if self.modal.is_none() {
            self.modal = Some(modal);
        } else {
            self.modal_queue.push_back(modal);
        }
        self.dirty = true;
    }

    pub(super) fn promote_queued_modal(&mut self) {
        if self.modal.is_none() {
            self.modal = self.modal_queue.pop_front();
        }
    }

    pub(super) fn request_turn_cancellation(&mut self) {
        self.cancel_requested = true;
        if !self.discard_stream_deltas {
            self.discard_stream_deltas = true;
            self.push_kind(TranscriptKind::Activity, "Stopping the active turn…");
        }
    }

    pub(super) fn cancel_pending_prompts(&mut self) {
        if self.modal.as_ref().is_some_and(Modal::cancel_prompt) {
            self.modal = None;
        }
        self.modal_queue.retain(|modal| !modal.cancel_prompt());
        self.promote_queued_modal();
        self.dirty = true;
    }

    pub(super) fn push_collapsible_block(
        &mut self,
        kind: TranscriptKind,
        title: impl Into<String>,
        details: impl Into<String>,
        state: BlockState,
        collapsed: bool,
    ) -> usize {
        self.clear_screen_selection();
        self.invalidate_transcript_render();
        let index = self.transcript.len();
        self.transcript.push(TranscriptItem::collapsible(
            kind, title, details, state, collapsed,
        ));
        self.dirty = true;
        index
    }

    pub(super) fn start_tool_block(&mut self, call_id: String, summary: String, details: String) {
        self.active_response = None;
        let index = self.push_collapsible_block(
            TranscriptKind::Activity,
            summary,
            details,
            BlockState::Running,
            true,
        );
        self.tool_blocks.insert(call_id, index);
    }

    pub(super) fn finish_tool_block(&mut self, call_id: &str, content: String, is_error: bool) {
        let content = truncate_block_details(&content, 30_000);
        let Some(index) = self.tool_blocks.remove(call_id) else {
            self.push_collapsible_block(
                TranscriptKind::Activity,
                "tool result",
                content,
                if is_error {
                    BlockState::Error
                } else {
                    BlockState::Done
                },
                !is_error,
            );
            return;
        };
        if let Some(item) = self.transcript.get_mut(index) {
            if let Some(block) = item.block.as_mut() {
                block.details = if block.details.trim().is_empty() {
                    content
                } else {
                    format!("{}\n\nResult:\n{}", block.details, content)
                };
                block.state = if is_error {
                    BlockState::Error
                } else {
                    BlockState::Done
                };
                block.collapsed = !is_error;
            }
        }
        self.invalidate_transcript_render();
        self.dirty = true;
    }

    pub(super) fn toggle_block(&mut self, index: usize) -> bool {
        let Some(block) = self
            .transcript
            .get_mut(index)
            .and_then(|item| item.block.as_mut())
        else {
            return false;
        };
        block.collapsed = !block.collapsed;
        self.invalidate_transcript_render();
        self.dirty = true;
        true
    }

    pub(super) fn push_kind(&mut self, kind: TranscriptKind, text: impl Into<String>) {
        self.clear_screen_selection();
        self.invalidate_transcript_render();
        let item = TranscriptItem::new(kind, text);
        if self.transcript_scroll > 0 || self.transcript_scroll_target > 0 {
            let added = item.rows();
            self.transcript_scroll = self.transcript_scroll.saturating_add(added);
            self.transcript_scroll_target = self.transcript_scroll_target.saturating_add(added);
        }
        self.transcript.push(item);
        self.dirty = true;
    }

    pub(super) fn push(&mut self, text: impl Into<String>) {
        self.push_kind(TranscriptKind::System, text);
    }

    fn push_user(&mut self, text: impl Into<String>) {
        self.push_kind(TranscriptKind::User, text);
    }

    pub(super) fn push_error(&mut self, text: impl Into<String>) {
        self.push_kind(TranscriptKind::Error, text);
    }

    pub(super) fn scroll_up(&mut self, rows: usize) {
        self.clear_screen_selection();
        self.transcript_scroll_target = self.transcript_scroll_target.saturating_add(rows);
        self.transcript_scroll = self.transcript_scroll_target;
        self.dirty = true;
    }

    pub(super) fn scroll_down(&mut self, rows: usize) {
        self.clear_screen_selection();
        self.transcript_scroll_target = self.transcript_scroll_target.saturating_sub(rows);
        self.transcript_scroll = self.transcript_scroll_target;
        self.dirty = true;
    }

    pub(super) fn follow_latest(&mut self) {
        self.clear_screen_selection();
        self.transcript_scroll = 0;
        self.transcript_scroll_target = 0;
        self.dirty = true;
    }

    /// Slash-menu rows for the composer text: built-in commands, then plugin
    /// commands. An exact name comes first, then names that start with the
    /// query, then names or descriptions that merely contain it. Putting the
    /// exact match first means Enter on a fully typed command submits that
    /// command instead of a different one that happens to contain the text.
    pub(super) fn slash_matches(&self) -> Vec<(String, String)> {
        if self.history_index.is_some() {
            return Vec::new();
        }
        if !self.input.starts_with('/') || self.input.contains(char::is_whitespace) {
            return Vec::new();
        }
        let query = self.input.trim_start_matches('/').to_ascii_lowercase();
        let builtin = SLASH_COMMANDS
            .iter()
            .filter(|(command, _)| *command != "/voice" || self.voice.available)
            .map(|(command, description)| (command.to_string(), description.to_string()));
        let mut matches = builtin
            .chain(self.extension_commands.iter().cloned())
            .filter(|(command, description)| {
                query.is_empty()
                    || command.trim_start_matches('/').contains(&query)
                    || description.to_ascii_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|(command, _)| {
            let name = command.trim_start_matches('/');
            if name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else {
                2
            }
        });
        matches
    }

    fn record_undo(&mut self) {
        self.undo_stack.push((self.input.clone(), self.cursor));
        if self.undo_stack.len() > 200 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(super) fn undo(&mut self) {
        let Some((input, cursor)) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push((self.input.clone(), self.cursor));
        self.input = input;
        self.cursor = cursor.min(self.input.len());
        self.selection_anchor = None;
        if let Some(marker) = self.voice.marker.as_mut() {
            *marker = (*marker).min(self.input.len());
        }
        self.dirty = true;
    }

    pub(super) fn redo(&mut self) {
        let Some((input, cursor)) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push((self.input.clone(), self.cursor));
        self.input = input;
        self.cursor = cursor.min(self.input.len());
        self.selection_anchor = None;
        if let Some(marker) = self.voice.marker.as_mut() {
            *marker = (*marker).min(self.input.len());
        }
        self.dirty = true;
    }

    pub(super) fn previous_word_boundary(&self) -> usize {
        let mut index = self.cursor;
        while index > 0 {
            let previous = self.input[..index]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(offset, _)| offset);
            let grapheme = &self.input[previous..index];
            if !grapheme.chars().all(char::is_whitespace) {
                break;
            }
            index = previous;
        }
        while index > 0 {
            let previous = self.input[..index]
                .grapheme_indices(true)
                .next_back()
                .map_or(0, |(offset, _)| offset);
            let grapheme = &self.input[previous..index];
            if grapheme.chars().all(char::is_whitespace) {
                break;
            }
            index = previous;
        }
        index
    }

    pub(super) fn next_word_boundary(&self) -> usize {
        let mut index = self.cursor;
        while index < self.input.len() {
            let next = self.input[index..]
                .grapheme_indices(true)
                .nth(1)
                .map_or(self.input.len(), |(offset, _)| index + offset);
            let grapheme = &self.input[index..next];
            index = next;
            if grapheme.chars().all(char::is_whitespace) {
                break;
            }
        }
        while index < self.input.len() {
            let next = self.input[index..]
                .grapheme_indices(true)
                .nth(1)
                .map_or(self.input.len(), |(offset, _)| index + offset);
            let grapheme = &self.input[index..next];
            if !grapheme.chars().all(char::is_whitespace) {
                break;
            }
            index = next;
        }
        index
    }

    pub(super) fn line_start(&self) -> usize {
        self.input[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    pub(super) fn line_end(&self) -> usize {
        self.input[self.cursor..]
            .find('\n')
            .map_or(self.input.len(), |offset| self.cursor + offset)
    }

    fn adjust_voice_marker(&mut self, start: usize, end: usize, replacement_len: usize) {
        let Some(marker) = self.voice.marker else {
            return;
        };
        self.voice.marker = Some(if marker < start {
            marker
        } else if marker <= end {
            start.saturating_add(replacement_len)
        } else {
            marker
                .saturating_sub(end.saturating_sub(start))
                .saturating_add(replacement_len)
        });
    }

    pub(super) fn insert(&mut self, text: &str) {
        self.delete_selection();
        self.record_undo();
        let start = self.cursor;
        self.adjust_voice_marker(start, start, text.len());
        self.input.insert_str(start, text);
        self.cursor += text.len();
        self.history_index = None;
        self.slash_selected = 0;
        self.dirty = true;
    }

    pub(super) fn insert_voice_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.selection_anchor = None;
        let marker = self
            .voice
            .marker
            .unwrap_or(self.cursor)
            .min(self.input.len());
        self.record_undo();
        self.input.insert_str(marker, text);
        if self.cursor >= marker {
            self.cursor += text.len();
        }
        self.voice.marker = Some(marker + text.len());
        self.history_index = None;
        self.slash_selected = 0;
        self.dirty = true;
    }

    pub(super) fn ensure_voice_leading_separator(&mut self, text: &str) {
        if !self.voice.committed.is_empty() || text.is_empty() {
            return;
        }
        let marker = self
            .voice
            .marker
            .unwrap_or(self.cursor)
            .min(self.input.len());
        let previous = self.input[..marker].chars().next_back();
        let first = text.chars().find(|ch| !ch.is_whitespace());
        if previous.is_some_and(|ch| !ch.is_whitespace())
            && first.is_some_and(|ch| ch.is_alphanumeric())
        {
            self.insert_voice_text(" ");
        }
    }

    pub(super) fn ensure_voice_trailing_separator(&mut self) {
        let Some(marker) = self.voice.marker else {
            return;
        };
        let marker = marker.min(self.input.len());
        if marker == 0 || marker == self.input.len() {
            return;
        }
        let previous = self.input[..marker].chars().next_back();
        let next = self.input[marker..].chars().next();
        if previous.is_some_and(|ch| !ch.is_whitespace())
            && next.is_some_and(|ch| ch.is_alphanumeric())
        {
            self.insert_voice_text(" ");
        }
    }

    pub(super) fn composer_display(&self) -> ComposerDisplay {
        let selection = self.input_selection_range();
        let Some(marker) = self
            .voice
            .marker
            .filter(|_| !self.voice.tentative.is_empty())
        else {
            return (self.input.clone(), self.cursor, None, selection);
        };
        let marker = marker.min(self.input.len());
        let mut display = String::with_capacity(self.input.len() + self.voice.tentative.len());
        display.push_str(&self.input[..marker]);
        let tentative_start = display.len();
        display.push_str(&self.voice.tentative);
        let tentative_end = display.len();
        display.push_str(&self.input[marker..]);
        let cursor = if self.cursor > marker {
            self.cursor + self.voice.tentative.len()
        } else {
            self.cursor
        };
        let mapped_selection = selection.map(|(start, end)| {
            let map = |offset: usize| {
                if offset <= marker {
                    offset
                } else {
                    offset + self.voice.tentative.len()
                }
            };
            (map(start), map(end))
        });
        (
            display,
            cursor,
            Some((tentative_start, tentative_end)),
            mapped_selection,
        )
    }

    pub(super) fn previous_boundary(&self) -> usize {
        self.input[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    pub(super) fn next_boundary(&self) -> usize {
        self.input[self.cursor..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(self.input.len(), |(index, _)| self.cursor + index)
    }

    pub(super) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.record_undo();
        let previous = self.previous_boundary();
        self.adjust_voice_marker(previous, self.cursor, 0);
        self.input.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.dirty = true;
    }

    pub(super) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == self.input.len() {
            return;
        }
        self.record_undo();
        let next = self.next_boundary();
        self.adjust_voice_marker(self.cursor, next, 0);
        self.input.replace_range(self.cursor..next, "");
        self.dirty = true;
    }

    pub(super) fn delete_previous_word(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.record_undo();
        let start = self.previous_word_boundary();
        self.adjust_voice_marker(start, self.cursor, 0);
        self.input.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.dirty = true;
    }

    pub(super) fn delete_to_start(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.record_undo();
        self.adjust_voice_marker(0, self.cursor, 0);
        self.input.replace_range(..self.cursor, "");
        self.cursor = 0;
        self.dirty = true;
    }

    pub(super) fn delete_to_end(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == self.input.len() {
            return;
        }
        self.record_undo();
        self.adjust_voice_marker(self.cursor, self.input.len(), 0);
        self.input.truncate(self.cursor);
        self.dirty = true;
    }

    pub(super) fn move_vertical(&mut self, up: bool) -> bool {
        let start = self.line_start();
        let end = self.line_end();
        let column = self.input[start..self.cursor].graphemes(true).count();
        let (target_start, target_end) = if up {
            if start == 0 {
                return false;
            }
            let target_end = start - 1;
            let target_start = self.input[..target_end]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            (target_start, target_end)
        } else {
            if end == self.input.len() {
                return false;
            }
            let target_start = end + 1;
            let target_end = self.input[target_start..]
                .find('\n')
                .map_or(self.input.len(), |offset| target_start + offset);
            (target_start, target_end)
        };
        self.cursor = self.input[target_start..target_end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset);
        self.dirty = true;
        true
    }

    pub(super) fn history_move(&mut self, older: bool) {
        if self.voice.active() || self.history.is_empty() {
            return;
        }
        // Down at the live position is a no-op: there is nothing newer
        // than "current." Without this guard, the restore branch below
        // overwrites `input` with "" when no draft was ever saved.
        if !older && self.history_index.is_none() {
            return;
        }
        self.selection_anchor = None;
        // Save the live draft on first Up so it survives the round trip.
        // The `history_draft.is_none()` guard avoids overwriting a draft
        // that a previous Up already stashed (e.g. Up → Down → Up again).
        if older && self.history_index.is_none() && self.history_draft.is_none() {
            self.history_draft = Some(self.input.clone());
        }
        let next = match (self.history_index, older) {
            (None, true) => self.history.len().checked_sub(1),
            (Some(index), true) => Some(index.saturating_sub(1)),
            (Some(index), false) if index + 1 < self.history.len() => Some(index + 1),
            (Some(_), false) => None,
            (None, false) => None, // unreachable: guarded above
        };
        self.history_index = next;
        // Use `.clone()` (not `.take()`) so the draft survives multiple
        // returns to live — Esc is the only consumer that takes it.
        self.input = match next {
            Some(index) => self.history.get(index).cloned().unwrap_or_default(),
            None => self.history_draft.clone().unwrap_or_default(),
        };
        self.cursor = self.input.len();
        self.dirty = true;
    }

    /// Restore the original pre-navigation draft, discarding any edits
    /// made to a recalled history entry. Called by the Esc handler when
    /// the user is in (or has returned from) history navigation.
    pub(super) fn restore_history_draft(&mut self) {
        self.history_index = None;
        self.input = self.history_draft.take().unwrap_or_default();
        self.cursor = self.input.len();
        self.selection_anchor = None;
        self.slash_selected = 0;
        self.dirty = true;
    }

    pub(super) fn clear_composer(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        self.history_index = None;
        self.history_draft = None;
        self.slash_selected = 0;
        self.voice.marker = None;
        self.voice.tentative.clear();
        self.voice.committed.clear();
        self.dirty = true;
    }

    pub(super) fn take_submission(&mut self) -> Option<(String, bool)> {
        if !self.submit_requested {
            return None;
        }
        self.submit_requested = false;
        let queue_for_next_turn = std::mem::take(&mut self.queue_submission_requested);
        let line = self.input.trim().to_string();
        if line.is_empty() {
            return None;
        }
        {
            self.history.push(line.clone());
            if self.history.len() > 2_000 {
                self.history.remove(0);
            }
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::history::append(&line, &cwd);
        }
        self.input.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        self.history_index = None;
        self.history_draft = None;
        self.slash_selected = 0;
        self.voice.marker = None;
        self.voice.tentative.clear();
        self.voice.committed.clear();
        self.dirty = true;
        Some((line, queue_for_next_turn))
    }

    pub(super) fn queue_follow_up(&mut self, line: String) {
        let display = display_submission(&line);
        self.active_response = None;
        self.queued_messages.push_back(line);
        self.push(format!(
            "⇥ queued for next turn ({}): {display}",
            self.queued_messages.len()
        ));
    }

    pub(super) fn pop_queued_follow_up(&mut self) -> Option<String> {
        let next = self.queued_messages.pop_front();
        if next.is_some() {
            self.dirty = true;
        }
        next
    }

    pub(super) fn append_delta(&mut self, delta: String) {
        if self.discard_stream_deltas || !self.running {
            return;
        }
        self.clear_screen_selection();
        self.invalidate_transcript_render();
        let index = match self.active_response {
            Some(index) => index,
            None => {
                self.transcript.push(TranscriptItem::new(
                    TranscriptKind::Assistant,
                    String::new(),
                ));
                let index = self.transcript.len() - 1;
                self.active_response = Some(index);
                index
            }
        };
        if self.transcript_scroll > 0 || self.transcript_scroll_target > 0 {
            let added = delta.matches('\n').count();
            self.transcript_scroll = self.transcript_scroll.saturating_add(added);
            self.transcript_scroll_target = self.transcript_scroll_target.saturating_add(added);
        }
        self.transcript[index].text.push_str(&delta);
        self.dirty = true;
    }

    pub(super) fn activity(&mut self, line: String) {
        self.active_response = None;
        self.push_kind(TranscriptKind::Activity, line);
    }

    pub(super) fn finish_turn(
        &mut self,
        result: Result<TurnResult, String>,
        session: Option<&Session>,
    ) {
        self.active_response = None;
        self.running = false;
        self.cancel_requested = false;
        self.discard_stream_deltas = false;
        match result {
            Ok(TurnResult::Done(_)) => {}
            Ok(TurnResult::Suspended(suspension)) => {
                self.push(format!("[turn suspended: {}]", suspension.request.summary));
            }
            Err(error) => {
                if let Some(session) = session {
                    agent::rollback_dangling_user(&session.agent);
                }
                self.push_error(format!("Request failed: {error}"));
            }
        }
        self.dirty = true;
    }
}

fn shell_quote_argument(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
    }
}

fn exit_message(resume_thread: Option<&str>) -> String {
    match resume_thread {
        Some(thread_id) => format!(
            "\nSession saved. Resume it with:\n  craftsman --session {}\n",
            shell_quote_argument(thread_id)
        ),
        None => "\nStart Craftsman again with:\n  craftsman\n".to_string(),
    }
}

pub async fn run(
    mut cfg: Config,
    auto_approve: bool,
    mouse_capture: bool,
    force_setup: bool,
    resume_thread: Option<String>,
) -> anyhow::Result<()> {
    if !stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("interactive mode requires a terminal");
    }

    if cfg.has_legacy_credentials() {
        // One-time migration notice (printed once per launch, not per turn).
        eprintln!(
            "note: login-era fields found in config.json; they are ignored now. Set \
             platform.api_token (or $CRAFTSMAN_PLATFORM_TOKEN) and point base_url at the \
             platform's OpenAI-compatible endpoint."
        );
    }
    if auto_approve || cfg.auto_approve {
        cfg.permission_mode = crate::config::PermissionMode::BypassPermissions;
    }
    let approval = Arc::new(Mutex::new(
        ApprovalState {
            mode: if auto_approve || cfg.auto_approve {
                crate::config::PermissionMode::BypassPermissions
            } else {
                cfg.permission_mode
            },
            auto_approve: auto_approve || cfg.auto_approve,
            auto_approve_network: auto_approve || cfg.auto_approve || cfg.auto_approve_web(),
            shell_allow_list: cfg.shell_allow_list.clone(),
        }
        .with_env_allow_list(),
    ));

    let guard = TerminalGuard::enter(mouse_capture)?;
    let mut terminal = Terminal::new(SafeBackend(CrosstermBackend::new(stdout())))?;
    terminal.clear()?;

    let agent_seed_error = crate::agents::ensure_agent(&cfg.agent).err();

    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let bridge = UiBridge { tx: ui_tx };
    let (voice_event_tx, mut voice_event_rx) = mpsc::unbounded_channel();
    let voice_ui_tx = bridge.tx.clone();
    tokio::spawn(async move {
        while let Some(event) = voice_event_rx.recv().await {
            if voice_ui_tx.send(UiEvent::Voice(event)).is_err() {
                break;
            }
        }
    });
    let mut voice_controller: Option<crate::voice::VoiceController> = None;
    let mut slot = AgentSlot::spawn(&cfg, approval.clone(), bridge.clone());
    let mut app = App::new(&cfg);
    if let Some(error) = agent_seed_error {
        app.push_error(format!(
            "Could not initialize agent profile '{}': {error}",
            cfg.agent
        ));
    }
    if force_setup {
        show_setup_modal(&mut app);
    } else if cfg.model.is_none() && cfg.platform.base_url.is_empty() && cfg.base_url.is_empty() {
        app.push_kind(
            TranscriptKind::Hint,
            "Setup required · run /setup to connect a platform or direct model endpoint",
        );
    }
    if let Some(thread_id) = resume_thread {
        app.push(format!("Resuming session {thread_id}…"));
        draw(&mut terminal, &mut app, &cfg, &slot, &approval)?;
        match slot.get().await.as_mut() {
            Some(session) => match session.load_thread(&thread_id).await {
                Ok(true) => {
                    let state = session.agent.state.lock().unwrap().clone();
                    hydrate_transcript(&mut app, &state);
                    app.push(format!("Resumed session {thread_id}"));
                    if let Some(suspension) = session.agent.pending_suspension() {
                        app.push(format!(
                            "Session is awaiting approval: {}",
                            suspension.request.summary
                        ));
                    }
                }
                Ok(false) => app.push_error(format!("No saved session found for {thread_id}")),
                Err(error) => {
                    app.push_error(format!("Could not resume session {thread_id}: {error}"))
                }
            },
            None => app.push_error("Cannot resume a session until a model is configured"),
        }
        app.dirty = true;
    }

    let mut events = EventStream::new();
    let mut frame_tick = tokio::time::interval(Duration::from_millis(16));
    frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = frame_tick.tick() => {
                let subagent_running = slot
                    .peek()
                    .and_then(|session| subagent_activity(&session.registry))
                    .is_some();
                if app.running || subagent_running {
                    app.spinner = app.spinner.wrapping_add(1);
                    app.dirty = true;
                }
                if app.dirty {
                    draw(&mut terminal, &mut app, &cfg, &slot, &approval)?;
                    app.dirty = false;
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if handle_terminal_event(&mut app, event) {
                            break;
                        }
                    }
                    Some(Err(error)) => app.push(format!("terminal input error: {error}")),
                    None => break,
                }
            }
            Some(event) = ui_rx.recv() => {
                handle_ui_event_batch(&mut app, event, &mut ui_rx, slot.peek());
            }
        }

        // The slash menu offers declarative extension commands. Re-read them when a new
        // "/…" line is started so gadget/plugin edits show up without a restart.
        let slash_now = app.input.starts_with('/');
        if slash_now && !app.slash_active {
            app.extension_commands = crate::extensions::menu_entries_in_cwd(&cfg);
            app.dirty = true;
        }
        app.slash_active = slash_now;

        if let Some(request) = app.voice_request.take() {
            match request {
                VoiceRequest::Start => {
                    if voice_controller.is_none() {
                        voice_controller = Some(crate::voice::VoiceController::spawn(
                            crate::voice::VoiceConfig::from_config(&cfg),
                            voice_event_tx.clone(),
                        ));
                    }
                    if let Some(controller) = &voice_controller {
                        controller.start();
                    }
                }
                VoiceRequest::StopAndFinalize => {
                    if let Some(controller) = &voice_controller {
                        controller.stop_and_finalize();
                    }
                }
                VoiceRequest::CancelSegment => {
                    if let Some(controller) = &voice_controller {
                        controller.cancel_segment();
                    }
                }
                VoiceRequest::Cancel => {
                    if let Some(controller) = &voice_controller {
                        controller.cancel();
                    }
                }
                VoiceRequest::Recreate => {
                    if let Some(controller) = voice_controller.take() {
                        controller.shutdown().await;
                    }
                    app.voice.state = crate::voice::VoiceState::Idle;
                    app.voice.partials_enabled = None;
                    app.dirty = true;
                }
            }
        }

        apply_turn_cancellation(&mut app, slot.peek().map(|session| &session.agent));

        if std::mem::take(&mut app.mode_cycle_requested) {
            let current = approval.lock().unwrap().mode;
            let index = crate::config::PermissionMode::ALL
                .iter()
                .position(|mode| *mode == current)
                .unwrap_or(0);
            let next = crate::config::PermissionMode::ALL
                [(index + 1) % crate::config::PermissionMode::ALL.len()];
            cfg.permission_mode = next;
            cfg.auto_approve = false;
            {
                let mut state = approval.lock().unwrap();
                state.mode = next;
                state.auto_approve = false;
                state.auto_approve_network = cfg.auto_approve_web();
            }
            match cfg.save() {
                Ok(()) => app.push(format!(
                    "Permission mode: {} — {}",
                    next.as_str(),
                    next.description()
                )),
                Err(error) => app.push_error(format!(
                    "Permission mode changed for this run, but could not save it: {error}"
                )),
            }
        }

        if std::mem::take(&mut app.copy_requested) {
            match latest_assistant_text(&app.transcript) {
                Some(text) => match copy_to_clipboard(text) {
                    Ok(()) => {
                        app.push("Copied the latest Craftsman response to the terminal clipboard.")
                    }
                    Err(error) => app.push_error(format!("Could not copy response: {error}")),
                },
                None => app.push("No Craftsman response is available to copy yet."),
            }
        }

        let action_ready = app.modal.is_none()
            && app.pending_action.as_ref().is_some_and(|action| {
                !app.running
                    || matches!(
                        action,
                        AppAction::SelectMode(_)
                            | AppAction::SelectVoiceModel(_)
                            | AppAction::RemoveQueued(_)
                            | AppAction::CancelTask(_)
                    )
            });
        if action_ready && let Some(action) = app.pending_action.take() {
            if let Some(status) = action_status(&action) {
                app.push(status);
            }
            draw(&mut terminal, &mut app, &cfg, &slot, &approval)?;
            app.dirty = false;
            handle_app_action(action, &mut cfg, &mut slot, &approval, &mut app).await;
            app.dirty = true;
            continue;
        }

        let submission = if app.modal.is_none() {
            app.take_submission()
                .map(|(line, queue_for_next)| (line, queue_for_next, false))
                .or_else(|| {
                    (!app.running)
                        .then(|| app.pop_queued_follow_up())
                        .flatten()
                        .map(|line| (line, false, true))
                })
        } else {
            None
        };

        if let Some((line, queue_for_next, _from_queue)) = submission {
            app.follow_latest();
            if app.running {
                if line.starts_with('/') && live_command(&line) {
                    app.push_user(display_submission(&line));
                    let rebuild =
                        handle_slash(&line, &mut cfg, &mut slot, &approval, &mut app).await;
                    if rebuild {
                        slot.rebuild(&cfg, approval.clone());
                    }
                } else if queue_for_next || line.starts_with('/') || line.starts_with('!') {
                    app.queue_follow_up(line);
                } else if slot
                    .peek()
                    .is_some_and(|session| session.agent.steer(line.clone()))
                {
                    app.active_response = None;
                    app.push(format!(
                        "↪ steering current turn: {}",
                        display_submission(&line)
                    ));
                } else {
                    // The turn may have completed between the key event and
                    // submission handling. Preserve the input as a follow-up.
                    app.queue_follow_up(line);
                }
                app.dirty = true;
                continue;
            }

            app.push_user(display_submission(&line));
            if line == "/quit" || line == "/exit" {
                break;
            }
            // A declarative extension slash command expands into a plain prompt for the
            // model. `submission_route` sends it straight to the turn: extension
            // text is never re-read as an inline `!shell` command or a
            // built-in `/command` — only the user's own typing may be. (While
            // a turn is running the typed command is queued above like any
            // other slash command and lands here when it is popped.)
            let extension_prompt = crate::extensions::expand_command_in_cwd(&line, &cfg);
            let route = submission_route(&line, extension_prompt.as_deref());
            draw(&mut terminal, &mut app, &cfg, &slot, &approval)?;
            app.dirty = false;
            match route {
                SubmissionRoute::Shell(command) => {
                    let output = crate::tools::run_shell_escape(
                        command.trim(),
                        &crate::shell::ShellSettings::from_config(&cfg),
                    )
                    .await;
                    app.push(output);
                }
                SubmissionRoute::Slash => {
                    let rebuild =
                        handle_slash(&line, &mut cfg, &mut slot, &approval, &mut app).await;
                    if rebuild {
                        slot.rebuild(&cfg, approval.clone());
                    }
                }
                SubmissionRoute::Prompt => {
                    if slot.is_pending() {
                        app.push("(finishing startup…)".to_string());
                        draw(&mut terminal, &mut app, &cfg, &slot, &approval)?;
                    }
                    let Some(session) = slot.get().await.as_ref() else {
                        app.push("No model configured. Run /setup or set base_url in ~/.craftsman/config.json, then /model.".to_string());
                        continue;
                    };
                    app.running = true;
                    app.discard_stream_deltas = false;
                    app.active_response = None;
                    let agent = session.agent.clone();
                    let tx = bridge.tx.clone();
                    let prompt = extension_prompt.unwrap_or(line);
                    tokio::spawn(async move {
                        let result = agent
                            .run_turn(prompt)
                            .await
                            .map_err(|error| error.to_string());
                        let _ = tx.send(UiEvent::TurnFinished(result));
                    });
                }
            }
            app.dirty = true;
        }

        // Drain continuously, including while the parent is waiting inside a
        // foreground task or a background child is still running. The agent has
        // its own notification subscription, so this display drain cannot steal
        // completion feedback from model context.
        if let Some(session) = slot.peek_mut() {
            drain_notifications(session, &mut app);
        }
    }

    let resume_id = slot.peek().and_then(|session| {
        let has_messages = !session.agent.state.lock().unwrap().messages.is_empty();
        (has_messages && session.checkpointer.is_some()).then(|| session.thread_id.clone())
    });
    if let Some(controller) = voice_controller.take() {
        controller.shutdown().await;
    }
    if let Some(session) = slot.peek() {
        session.lsp.shutdown().await;
    }
    terminal.show_cursor()?;
    drop(terminal);
    drop(guard);
    print!("{}", exit_message(resume_id.as_deref()));
    Ok(())
}

pub(super) fn truncate_block_details(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    format!(
        "{}\n… [block details truncated]",
        text.chars().take(max_chars).collect::<String>()
    )
}

fn latest_assistant_text(items: &[TranscriptItem]) -> Option<&str> {
    items
        .iter()
        .rev()
        .find(|item| item.kind == TranscriptKind::Assistant && !item.text.trim().is_empty())
        .map(|item| item.text.as_str())
}

#[cfg(test)]
mod tests {
    use super::super::render::render_transcript;
    use super::*;
    use ratatui::style::Style;
    use ratatui::text::Text;

    fn plain_text(text: &Text<'_>) -> String {
        text.lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn tool_lifecycle_updates_one_collapsible_block() {
        let mut app = App::new(&Config::default());
        app.start_tool_block(
            "call-1".into(),
            "execute › cargo test".into(),
            "Command: cargo test".into(),
        );
        assert_eq!(
            app.transcript.last().unwrap().block.as_ref().unwrap().state,
            BlockState::Running
        );
        app.finish_tool_block("call-1", "FULL TOOL RESULT".into(), false);
        let index = app.transcript.len() - 1;
        let block = app.transcript[index].block.as_ref().unwrap();
        assert_eq!(block.state, BlockState::Done);
        assert!(block.collapsed);
        assert!(!plain_text(&render_transcript(&app.transcript, 80)).contains("FULL TOOL RESULT"));

        assert!(app.toggle_block(index));
        assert!(plain_text(&render_transcript(&app.transcript, 80)).contains("FULL TOOL RESULT"));
    }

    #[test]
    fn failed_tool_block_expands_automatically() {
        let mut app = App::new(&Config::default());
        app.start_tool_block(
            "call-2".into(),
            "execute › bad".into(),
            "bad command".into(),
        );
        app.finish_tool_block("call-2", "permission denied".into(), true);
        let block = app.transcript.last().unwrap().block.as_ref().unwrap();
        assert_eq!(block.state, BlockState::Error);
        assert!(!block.collapsed);
    }

    #[test]
    fn click_drag_selection_extracts_visible_transcript_text() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        buffer.set_string(0, 0, "hello world", Style::default());
        buffer.set_string(0, 1, "second line", Style::default());
        let mut app = App::new(&Config::default());
        app.screen_snapshot = Some(ScreenSnapshot::from_buffer(&buffer, Rect::new(0, 0, 20, 2)));

        app.begin_screen_selection(Position { x: 0, y: 0 });
        app.finish_screen_selection(Position { x: 4, y: 0 });
        assert_eq!(app.selected_screen_text().as_deref(), Some("hello"));
    }

    #[test]
    fn exit_message_contains_one_command_resume_path() {
        assert_eq!(
            exit_message(Some("1234-abcd")),
            "\nSession saved. Resume it with:\n  craftsman --session 1234-abcd\n"
        );
        assert!(exit_message(None).contains("\n  craftsman\n"));
        assert_eq!(
            shell_quote_argument("thread with spaces"),
            "\"thread with spaces\""
        );
    }

    #[test]
    fn cancelled_or_idle_turn_discards_late_stream_deltas() {
        let mut app = App::new(&Config::default());
        app.running = true;
        app.discard_stream_deltas = true;
        app.append_delta("stale".into());
        assert!(
            !app.transcript
                .iter()
                .any(|item| item.kind == TranscriptKind::Assistant)
        );
        app.discard_stream_deltas = false;
        app.running = false;
        app.append_delta("late".into());
        assert!(
            !app.transcript
                .iter()
                .any(|item| item.kind == TranscriptKind::Assistant)
        );
    }

    #[test]
    fn slash_menu_ignores_extension_commands_that_do_not_match() {
        let mut app = App::new(&Config::default());
        app.extension_commands = vec![("/review".into(), "Review the diff (plugin x)".into())];
        app.input = "/zzz".into();
        assert!(app.slash_matches().is_empty());
        // Arguments end the menu: the line is submitted as typed.
        app.input = "/review focus".into();
        assert!(app.slash_matches().is_empty());
    }

    #[test]
    fn composer_supports_word_delete_and_undo() {
        let mut app = App::new(&Config::default());
        app.insert("hello world");
        app.delete_previous_word();
        assert_eq!(app.input, "hello ");
        app.undo();
        assert_eq!(app.input, "hello world");
    }

    #[test]
    fn input_cursor_moves_by_grapheme() {
        let mut app = App::new(&Config::default());
        app.insert("a👍b");
        app.cursor = app.previous_boundary();
        assert_eq!(&app.input[..app.cursor], "a👍");
        app.backspace();
        assert_eq!(app.input, "ab");
    }

    #[test]
    fn slash_matches_filter_commands() {
        let mut app = App::new(&Config::default());
        app.input = "/mo".into();
        app.cursor = app.input.len();
        let matches = app.slash_matches();
        assert_eq!(matches[0].0, "/model");
    }

    #[test]
    fn mouse_sized_scroll_is_immediate_and_does_not_invalidate_render_cache() {
        let mut app = App::new(&Config::default());
        app.transcript_render_cache = Some(TranscriptRenderCache {
            revision: app.transcript_revision,
            width: 80,
            text: Text::raw("cached"),
            visual_lines: 1,
            block_rows: Vec::new(),
        });
        app.scroll_up(3);
        assert_eq!(app.transcript_scroll, 3);
        assert_eq!(app.transcript_scroll_target, 3);
        assert!(app.transcript_render_cache.is_some());
        app.scroll_down(2);
        assert_eq!(app.transcript_scroll, 1);
        assert_eq!(app.transcript_scroll_target, 1);
    }

    #[test]
    fn transcript_content_changes_invalidate_render_cache() {
        let mut app = App::new(&Config::default());
        app.transcript_render_cache = Some(TranscriptRenderCache {
            revision: app.transcript_revision,
            width: 80,
            text: Text::raw("cached"),
            visual_lines: 1,
            block_rows: Vec::new(),
        });
        app.push("new content");
        assert!(app.transcript_render_cache.is_none());
        assert_eq!(app.transcript_revision, 1);
    }

    #[test]
    fn input_selection_range_orders_anchor_and_cursor() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 5;
        app.selection_anchor = Some(2);
        assert_eq!(app.input_selection_range(), Some((2, 5)));

        app.cursor = 0;
        app.selection_anchor = Some(8);
        assert_eq!(app.input_selection_range(), Some((0, 8)));

        app.selection_anchor = None;
        assert_eq!(app.input_selection_range(), None);
    }

    #[test]
    fn delete_selection_removes_selected_range() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 8;
        app.selection_anchor = Some(2);
        assert!(app.delete_selection());
        assert_eq!(app.input, "herld");
        assert_eq!(app.cursor, 2);
        assert!(app.selection_anchor.is_none());
    }

    #[test]
    fn delete_selection_returns_false_without_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello".into();
        app.cursor = 3;
        app.selection_anchor = None;
        assert!(!app.delete_selection());
        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn grapheme_offset_at_pos_maps_row_col_to_byte_offset() {
        let mut app = App::new(&Config::default());
        app.input = "hello world".into();
        app.cursor = 0;
        app.input_area_rect = Rect::new(0, 10, 40, 3);
        app.input_max_width = 38;
        app.input_scroll = 0;
        let inner = app.input_block_inner();
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y), Some(0));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 5, inner.y), Some(5));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 11, inner.y), Some(11));
    }

    #[test]
    fn grapheme_offset_at_pos_handles_wrapped_lines() {
        let mut app = App::new(&Config::default());
        app.input = "abcdefghij".into();
        app.cursor = 0;
        app.input_area_rect = Rect::new(0, 10, 6, 5);
        app.input_max_width = 4;
        app.input_scroll = 0;
        let inner = app.input_block_inner();
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y), Some(0));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 3, inner.y), Some(3));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 4, inner.y), Some(4));
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y + 1), Some(4));
        assert_eq!(
            app.grapheme_offset_at_pos(inner.x + 1, inner.y + 1),
            Some(5)
        );
        assert_eq!(
            app.grapheme_offset_at_pos(inner.x + 4, inner.y + 1),
            Some(8)
        );
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y + 2), Some(8));
        assert_eq!(
            app.grapheme_offset_at_pos(inner.x + 2, inner.y + 2),
            Some(10)
        );
    }

    #[test]
    fn grapheme_offset_at_pos_handles_hard_newlines() {
        let mut app = App::new(&Config::default());
        app.input = "ab\ncd".into();
        app.cursor = 0;
        app.input_area_rect = Rect::new(0, 10, 20, 5);
        app.input_max_width = 18;
        app.input_scroll = 0;
        let inner = app.input_block_inner();
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y), Some(0));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 2, inner.y), Some(2));
        assert_eq!(app.grapheme_offset_at_pos(inner.x + 3, inner.y), Some(3));
        assert_eq!(app.grapheme_offset_at_pos(inner.x, inner.y + 1), Some(3));
        assert_eq!(
            app.grapheme_offset_at_pos(inner.x + 2, inner.y + 1),
            Some(5)
        );
    }

    #[test]
    fn clear_composer_clears_input_selection() {
        let mut app = App::new(&Config::default());
        app.input = "hello".into();
        app.cursor = 3;
        app.selection_anchor = Some(1);
        app.clear_composer();
        assert!(app.selection_anchor.is_none());
        assert!(app.input.is_empty());
    }

    #[test]
    fn undo_clears_input_selection() {
        let mut app = App::new(&Config::default());
        app.insert("hello");
        app.cursor = 3;
        app.selection_anchor = Some(1);
        app.undo();
        assert!(app.selection_anchor.is_none());
    }
}
