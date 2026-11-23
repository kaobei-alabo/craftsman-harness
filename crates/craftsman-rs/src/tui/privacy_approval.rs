//! The private gate's opaque request is transported to the actual receiver.
//! Only a modal installed by that receiver presents it; only a terminal human
//! choice consumes that presentation. No digest or ordinary approval result is
//! converted into a decision, and no policy decision is manufactured here.
use super::app::App;
use super::modals::Modal;
use super::session_bridge::{UiBridge, UiEvent, cancel_prompt, handle_ui_event, scoped_prompt};
use crate::privacy_entry::{
    HumanChoice, PrivacyApprovalDecision, PrivacyApprovalPresentation, PrivacyApprovalPrompt,
    PrivacyApprovalRequest, PrivacyApprovalUi, register_privacy_approval_ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::cell::Cell;
use std::sync::{Arc, mpsc};

const REVIEW_BYTES: usize = 1024 * 1024;
const INPUT_BYTES: usize = crate::privacy_hook::MAX_TEXT_BYTES;

pub(super) fn register(bridge: &UiBridge) -> (Arc<PrivacyApprovalPrompt>, Arc<PrivacyApprovalUi>) {
    let tx = bridge.tx.clone();
    register_privacy_approval_ui(Arc::new(move |request: PrivacyApprovalRequest| {
        let turn = craftsman_harness::TurnCancellation::current();
        if turn.as_ref().is_some_and(|turn| turn.is_cancelled()) {
            return None;
        }
        let (response, receive) = mpsc::channel();
        let _cancel = cancel_prompt(turn.clone(), response.clone(), None);
        tx.send(scoped_prompt(
            UiEvent::PrivacyApproval { request, response },
            &turn,
        ))
        .ok()?;
        let returned = crate::util::block_in_place(|| receive.recv())
            .ok()
            .flatten();
        if turn.as_ref().is_some_and(|turn| turn.is_cancelled()) {
            None
        } else {
            // Return the receiver's original opaque decision. Core validates
            // its request, presentation, callback and current ownership.
            returned
        }
    }))
}

pub(super) fn handle(
    app: &mut App,
    event: UiEvent,
    ui: &Arc<PrivacyApprovalUi>,
    session: Option<&crate::agent::Session>,
) {
    match event {
        UiEvent::ScopedPrompt { cancellation, event } => {
            if cancellation.is_cancelled() {
                // The actual turn is already stopped before any waiter is
                // released. Ordinary prompts keep their existing refusal.
                match *event {
                    UiEvent::PrivacyApproval { response, .. } => { let _ = response.send(None); }
                    event => handle_ui_event(app, UiEvent::ScopedPrompt {
                        cancellation, event: Box::new(event),
                    }, session),
                }
            } else {
                handle(app, *event, ui, session);
            }
        }
        UiEvent::PrivacyApproval { request, response } => {
            match ui.present(&request) {
                Ok(presentation) => match PrivacyApprovalModal::new(presentation, ui.clone(), response) {
                    Ok(modal) => {
                        app.active_response = None;
                        app.enqueue_modal(Modal::PrivacyApproval(modal));
                    }
                    Err(()) => app.push_error("The private approval review exceeds its display bound; this operation was not approved."),
                },
                Err(_) => {
                    let _ = response.send(None);
                    app.push_error("The private approval request is no longer available; this operation was not approved.");
                }
            }
        }
        event => handle_ui_event(app, event, session),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Review,
    Feedback,
    Edit,
}

pub(super) struct PrivacyApprovalModal {
    presentation: Option<PrivacyApprovalPresentation>,
    ui: Arc<PrivacyApprovalUi>,
    response: mpsc::Sender<Option<PrivacyApprovalDecision>>,
    contents: String,
    file_review: bool,
    network: bool,
    scroll: Cell<usize>,
    selected: usize,
    input_mode: InputMode,
    feedback: String,
    edit: String,
    input_refused: bool,
    review_drawn: Cell<bool>,
}
impl std::fmt::Debug for PrivacyApprovalModal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateApprovalModal([private])")
    }
}
impl PrivacyApprovalModal {
    fn new(
        presentation: PrivacyApprovalPresentation,
        ui: Arc<PrivacyApprovalUi>,
        response: mpsc::Sender<Option<PrivacyApprovalDecision>>,
    ) -> Result<Self, ()> {
        let view: crate::privacy_entry::PrivacyApprovalView<'_> = presentation.review();
        // Display the actual derived call's complete argument representation;
        // this is never used as the raw source of a human Edit decision.
        let arguments = match serde_json::to_string_pretty(&view.call.args) {
            Ok(arguments) => arguments,
            Err(_) => {
                let _ = response.send(None);
                return Err(());
            }
        };
        let length = view
            .call
            .name
            .len()
            .checked_add(view.summary.len())
            .and_then(|length| length.checked_add(view.details.len()))
            .and_then(|length| length.checked_add(arguments.len()))
            .and_then(|length| length.checked_add(8));
        if length.is_none_or(|length| length > REVIEW_BYTES) {
            let _ = response.send(None);
            return Err(());
        }
        let contents = format!(
            "{}\n\n{}\n\n{}\n\n{}",
            view.call.name, view.summary, view.details, arguments
        );
        let file_review = view.shown.is_some();
        let network = crate::tools::NETWORK_TOOLS.contains(&view.call.name.as_str());
        Ok(Self {
            presentation: Some(presentation),
            ui,
            response,
            contents,
            file_review,
            network,
            scroll: Cell::new(0),
            selected: 0,
            input_mode: InputMode::Review,
            feedback: String::new(),
            edit: String::new(),
            input_refused: false,
            review_drawn: Cell::new(false),
        })
    }
    pub(super) fn cancel(&self) {
        // Cancellation is an actual absent reply, not a fabricated denial or
        // approval. The core refuses a reply that arrives after closure.
        let _ = self.response.send(None);
    }
    fn answer(mut self, choice: HumanChoice) -> bool {
        let Some(presentation) = self.presentation.take() else {
            return false;
        };
        match self.ui.decide(presentation, choice) {
            Ok(decision) => self.response.send(Some(decision)).is_ok(),
            Err(_) => {
                let _ = self.response.send(None);
                false
            }
        }
    }
    fn input_mut(&mut self) -> Option<&mut String> {
        match self.input_mode {
            InputMode::Review => None,
            InputMode::Feedback => Some(&mut self.feedback),
            InputMode::Edit => Some(&mut self.edit),
        }
    }
    pub(super) fn paste(&mut self, text: &str) {
        let Some(input) = self.input_mut() else {
            return;
        };
        if input
            .len()
            .checked_add(text.len())
            .is_none_or(|size| size > INPUT_BYTES)
        {
            self.input_refused = true;
            return;
        }
        // Preserve the raw human JSON/feedback bytes before core parsing and
        // projection. Whole paste refusal leaves the earlier draft intact.
        input.push_str(text);
        self.input_refused = false;
    }
    pub(super) fn render(&self, frame: &mut Frame<'_>, _area: Rect) {
        let outer = frame.area();
        let area = Rect::new(
            outer.x.saturating_add(1),
            outer.y.saturating_add(1),
            outer.width.saturating_sub(2),
            outer.height.saturating_sub(2),
        );
        frame.render_widget(Clear, area);
        if area.height < 12 || area.width < 24 {
            self.review_drawn.set(false);
            frame.render_widget(Paragraph::new("Enlarge the terminal to review this operation. Escape or Ctrl-C still refuses it."), area);
            return;
        }
        self.review_drawn.set(true);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(7)])
            .split(area);
        let (contents, title) = match self.input_mode {
            InputMode::Review => (
                self.contents.as_str(),
                if self.file_review {
                    " private file review · PgUp/PgDn/Home/End "
                } else {
                    " private operation review · PgUp/PgDn/Home/End "
                },
            ),
            InputMode::Feedback => (
                self.feedback.as_str(),
                " deny with exact feedback · Enter sends · Esc returns ",
            ),
            InputMode::Edit => (
                self.edit.as_str(),
                " raw edited JSON · F2/Ctrl-Enter sends · Esc returns ",
            ),
        };
        let width = usize::from(chunks[0].width.saturating_sub(2).max(1));
        // Escape controls/bidi and literal backslashes before width wrapping,
        // so SafeBackend cannot expand an invisible scalar over the next cell
        // or leave a control-bearing part of the bounded review unreachable.
        let displayed = escaped(contents);
        let lines = super::modals::approval_detail_lines(&displayed, width);
        let height = usize::from(chunks[0].height.saturating_sub(2));
        let start = self.scroll.get().min(lines.len().saturating_sub(height));
        self.scroll.set(start);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(start)
                    .take(height)
                    .map(Line::from)
                    .collect::<Vec<_>>(),
            )
            .block(Block::default().title(title).borders(Borders::ALL)),
            chunks[0],
        );
        let always = if self.network {
            "Allow network this session"
        } else {
            "Allow local changes this session"
        };
        let choices = [
            "Approve once",
            always,
            "Deny",
            "Deny with feedback",
            "Enter edited JSON",
        ];
        let mut state = ListState::default().with_selected(Some(self.selected));
        let title = if self.input_refused {
            " whole input refused: byte bound · earlier draft retained "
        } else {
            " the original bound request decides this operation "
        };
        frame.render_stateful_widget(
            List::new(choices.map(ListItem::new))
                .block(Block::default().title(title).borders(Borders::ALL))
                .highlight_symbol("▶ ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
            chunks[1],
            &mut state,
        );
    }
}

fn escaped(text: &str) -> String {
    let mut displayed = String::new();
    for character in text.chars() {
        if character == '\\'
            || (character.is_control() && character != '\n')
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            displayed.extend(character.escape_default());
        } else {
            displayed.push(character);
        }
    }
    displayed
}
impl Drop for PrivacyApprovalModal {
    fn drop(&mut self) {
        if self.presentation.is_some() {
            self.cancel();
        }
    }
}

pub(super) fn key(app: &mut App, key: KeyEvent) {
    let Some(Modal::PrivacyApproval(modal)) = app.modal.as_mut() else {
        return;
    };
    let mut choice = None;
    match key.code {
        KeyCode::PageDown => modal.scroll.set(modal.scroll.get().saturating_add(10)),
        KeyCode::PageUp => modal.scroll.set(modal.scroll.get().saturating_sub(10)),
        KeyCode::Home => modal.scroll.set(0),
        KeyCode::End => modal.scroll.set(usize::MAX),
        _ if modal.input_mode != InputMode::Review => match key.code {
            KeyCode::Esc => {
                modal.input_mode = InputMode::Review;
                modal.scroll.set(0);
            }
            KeyCode::Backspace => {
                if let Some(input) = modal.input_mut() {
                    input.pop();
                }
            }
            KeyCode::Enter if modal.input_mode == InputMode::Feedback => {
                choice = Some(HumanChoice::Deny(std::mem::take(&mut modal.feedback)));
            }
            KeyCode::F(2) if modal.input_mode == InputMode::Edit => {
                choice = Some(HumanChoice::Edit(std::mem::take(&mut modal.edit)));
            }
            KeyCode::Enter
                if modal.input_mode == InputMode::Edit
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                choice = Some(HumanChoice::Edit(std::mem::take(&mut modal.edit)));
            }
            KeyCode::Enter if modal.input_mode == InputMode::Edit => modal.paste("\n"),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let mut bytes = [0; 4];
                modal.paste(character.encode_utf8(&mut bytes));
            }
            _ => {}
        },
        KeyCode::Up => modal.selected = modal.selected.saturating_sub(1),
        KeyCode::Down => modal.selected = (modal.selected + 1).min(4),
        KeyCode::Esc => choice = Some(HumanChoice::Deny(String::new())),
        KeyCode::Enter => match modal.selected {
            0 => choice = Some(HumanChoice::Approve),
            1 => choice = Some(HumanChoice::ApproveAlways),
            2 => choice = Some(HumanChoice::Deny(String::new())),
            3 => {
                modal.input_mode = InputMode::Feedback;
                modal.scroll.set(0);
            }
            _ => {
                modal.input_mode = InputMode::Edit;
                modal.scroll.set(0);
            }
        },
        _ => {}
    }
    if let Some(choice) = choice {
        if matches!(
            choice,
            HumanChoice::Approve | HumanChoice::ApproveAlways | HumanChoice::Edit(_)
        ) && !modal.review_drawn.get()
        {
            // An already queued Enter/F2 is not a positive decision on a
            // review that this actual receiver has not drawn. Denial/cancel
            // remains available; no caller readiness flag is consulted.
            if let HumanChoice::Edit(raw) = choice {
                modal.edit = raw;
            }
            app.dirty = true;
            return;
        }
        if let Some(Modal::PrivacyApproval(modal)) = app.modal.take() {
            if !modal.answer(choice) {
                app.push_error("The private approval answer was not accepted; this operation was not approved.");
            }
        }
    }
    app.promote_queued_modal();
    app.dirty = true;
}
