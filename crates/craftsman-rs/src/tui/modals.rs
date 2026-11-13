//! Modal dialogs: their state, key handling, rendering and the pickers that
//! open them.

use super::app::App;
use super::commands::display_submission;
use super::session_bridge::ApprovalChoice;
use crate::config::Config;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use std::sync::mpsc as std_mpsc;

pub(super) struct ApprovalModal {
    pub(super) tool: String,
    pub(super) summary: String,
    pub(super) details: String,
    pub(super) selected: usize,
    pub(super) feedback: String,
    pub(super) feedback_mode: bool,
    pub(super) response: std_mpsc::Sender<ApprovalChoice>,
}

pub(super) struct AskModal {
    pub(super) question: String,
    pub(super) answer: String,
    pub(super) response: std_mpsc::Sender<Option<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum AppAction {
    ChooseModel(crate::model_controls::CatalogModel),
    #[cfg(test)]
    SelectModel(String),
    SelectModelEffort {
        model: String,
        effort: Option<crate::model_controls::ReasoningEffort>,
    },
    SelectModelBudget {
        model: String,
        budget: u32,
    },
    SelectVoiceModel(String),
    SelectHarness(String),
    SelectAgent(String),
    SelectMode(crate::config::PermissionMode),
    ResumeSession(String),
    CancelTask(String),
    RemoveQueued(usize),
    SetupDirect {
        base_url: String,
        api_key: String,
        model: String,
    },
    EditProvider(String),
    SaveProviderSetup {
        provider: String,
        base_url: String,
        api_key: String,
        model: String,
    },
}

#[derive(Clone)]
pub(super) struct SelectionItem {
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) enter_action: Option<AppAction>,
    pub(super) delete_action: Option<AppAction>,
}

pub(super) struct SelectionModal {
    pub(super) title: String,
    pub(super) items: Vec<SelectionItem>,
    pub(super) selected: usize,
    pub(super) hint: String,
}

impl SelectionModal {
    fn is_model_catalog(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item.enter_action, Some(AppAction::ChooseModel(_))))
    }

    fn selectable(&self, index: usize) -> bool {
        !self.is_model_catalog()
            || self
                .items
                .get(index)
                .is_some_and(|item| item.enter_action.is_some())
    }

    fn move_selection(&mut self, forward: bool, steps: usize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.min(self.items.len() - 1);
        for _ in 0..steps {
            let next = if forward {
                ((self.selected + 1)..self.items.len()).find(|&index| self.selectable(index))
            } else {
                (0..self.selected)
                    .rev()
                    .find(|&index| self.selectable(index))
            };
            if let Some(next) = next {
                self.selected = next;
            } else {
                break;
            }
        }
    }
}

pub(super) struct EditorModal {
    pub(super) text: String,
}

pub(super) struct SetupModal {
    provider: Option<(String, &'static str)>,
    pub(super) fields: Vec<String>,
    pub(super) field: usize,
}

impl SetupModal {
    pub(super) fn paste(&mut self, text: &str) -> bool {
        if self.provider.is_some() {
            let bytes = text
                .chars()
                .filter(|c| !matches!(c, '\r' | '\n'))
                .try_fold(0usize, |size, c| size.checked_add(c.len_utf8()));
            let bound = [4096, 16384, 512][self.field];
            if bytes
                .and_then(|size| self.fields[self.field].len().checked_add(size))
                .is_none_or(|size| size > bound)
            {
                return false;
            }
        }
        self.fields[self.field].push_str(&text.replace(['\r', '\n'], ""));
        true
    }

    fn new() -> Self {
        Self {
            provider: None,
            fields: vec![String::new(), String::new(), String::new()],
            field: 0,
        }
    }

    fn labels(&self) -> [&'static str; 3] {
        if self.provider.is_some() {
            ["Base URL", "API key", "API model ID"]
        } else {
            ["Base URL", "API key", "Model"]
        }
    }
}

pub(super) enum Modal {
    Approval(ApprovalModal),
    Ask(AskModal),
    Selection(SelectionModal),
    Setup(SetupModal),
    Editor(EditorModal),
}

impl Modal {
    /// Release a synchronous agent waiter with a refusal, never a grant.
    /// Other dialogs retain their drafts and position in the queue.
    pub(super) fn cancel_prompt(&self) -> bool {
        match self {
            Self::Approval(modal) => {
                let _ = modal.response.send(ApprovalChoice::Deny(String::new()));
                true
            }
            Self::Ask(modal) => {
                let _ = modal.response.send(None);
                true
            }
            _ => false,
        }
    }
}

pub(super) fn handle_modal_key(app: &mut App, key: KeyEvent) {
    let Some(modal) = app.modal.as_mut() else {
        return;
    };
    match modal {
        Modal::Approval(approval) if approval.feedback_mode => match key.code {
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                approval.feedback.push(character)
            }
            KeyCode::Backspace => {
                approval.feedback.pop();
            }
            KeyCode::Enter => {
                if let Some(Modal::Approval(modal)) = app.modal.take() {
                    let _ = modal.response.send(ApprovalChoice::Deny(modal.feedback));
                }
            }
            KeyCode::Esc => approval.feedback_mode = false,
            _ => {}
        },
        Modal::Approval(approval) => match key.code {
            KeyCode::Up => approval.selected = approval.selected.saturating_sub(1),
            KeyCode::Down => approval.selected = (approval.selected + 1).min(3),
            KeyCode::Enter => {
                if approval.selected == 3 {
                    approval.feedback_mode = true;
                } else if let Some(Modal::Approval(modal)) = app.modal.take() {
                    let choice = match modal.selected {
                        0 => ApprovalChoice::Approve,
                        1 => ApprovalChoice::Always,
                        _ => ApprovalChoice::Deny(String::new()),
                    };
                    let _ = modal.response.send(choice);
                }
            }
            KeyCode::Esc => {
                if let Some(Modal::Approval(modal)) = app.modal.take() {
                    let _ = modal.response.send(ApprovalChoice::Deny(String::new()));
                }
            }
            _ => {}
        },
        Modal::Ask(ask) => match key.code {
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                ask.answer.push(character)
            }
            KeyCode::Backspace => {
                ask.answer.pop();
            }
            KeyCode::Enter => {
                if let Some(Modal::Ask(modal)) = app.modal.take() {
                    let _ = modal.response.send(Some(modal.answer));
                }
            }
            KeyCode::Esc => {
                if let Some(Modal::Ask(modal)) = app.modal.take() {
                    let _ = modal.response.send(None);
                }
            }
            _ => {}
        },
        Modal::Selection(selection) => match key.code {
            KeyCode::Up => selection.move_selection(false, 1),
            KeyCode::Down => selection.move_selection(true, 1),
            KeyCode::PageUp => selection.move_selection(false, 10),
            KeyCode::PageDown => selection.move_selection(true, 10),
            KeyCode::Home => selection.move_selection(false, selection.items.len()),
            KeyCode::End => {
                selection.move_selection(true, selection.items.len());
            }
            KeyCode::Enter => {
                let action = selection
                    .items
                    .get(selection.selected)
                    .and_then(|item| item.enter_action.clone());
                if let Some(action) = action {
                    app.pending_action = Some(action);
                    app.modal = None;
                }
            }
            KeyCode::Char('d') => {
                let action = selection
                    .items
                    .get(selection.selected)
                    .and_then(|item| item.delete_action.clone());
                if let Some(action) = action {
                    app.pending_action = Some(action);
                    app.modal = None;
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => app.modal = None,
            _ => {}
        },
        Modal::Editor(editor) => match key.code {
            KeyCode::F(2) => {
                if let Some(Modal::Editor(modal)) = app.modal.take() {
                    app.input = modal.text;
                    app.cursor = app.input.len();
                }
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(Modal::Editor(modal)) = app.modal.take() {
                    app.input = modal.text;
                    app.cursor = app.input.len();
                }
            }
            KeyCode::Enter => editor.text.push('\n'),
            KeyCode::Tab => editor.text.push_str("    "),
            KeyCode::Backspace => {
                editor.text.pop();
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                editor.text.push(character)
            }
            KeyCode::Esc => app.modal = None,
            _ => {}
        },
        Modal::Setup(setup) => match key.code {
            KeyCode::Tab | KeyCode::Down => setup.field = (setup.field + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => setup.field = (setup.field + 2) % 3,
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let bound = [4096, 16384, 512][setup.field];
                if setup.provider.is_none()
                    || setup.fields[setup.field].len() + character.len_utf8() <= bound
                {
                    setup.fields[setup.field].push(character)
                }
            }
            KeyCode::Char('u')
                if setup.provider.is_some() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                setup.fields[setup.field].clear()
            }
            KeyCode::Backspace => {
                setup.fields[setup.field].pop();
            }
            KeyCode::Enter if setup.field < 2 => setup.field += 1,
            KeyCode::Enter => {
                // A local compatible endpoint may have no API key.
                let missing =
                    setup.fields[0].trim().is_empty() || setup.fields[2].trim().is_empty();
                if !missing && let Some(Modal::Setup(modal)) = app.modal.take() {
                    app.pending_action = Some(if let Some((provider, _)) = modal.provider {
                        AppAction::SaveProviderSetup {
                            provider,
                            base_url: modal.fields[0].trim().to_string(),
                            api_key: modal.fields[1].clone(),
                            model: modal.fields[2].trim().to_string(),
                        }
                    } else {
                        AppAction::SetupDirect {
                            base_url: modal.fields[0].trim().to_string(),
                            api_key: modal.fields[1].clone(),
                            model: modal.fields[2].trim().to_string(),
                        }
                    });
                }
            }
            KeyCode::Esc => app.modal = None,
            _ => {}
        },
    }
    app.promote_queued_modal();
    app.dirty = true;
}

pub(super) fn render_modal(frame: &mut ratatui::Frame<'_>, modal: &Modal) {
    let area = centered_rect(72, 55, frame.area());
    frame.render_widget(Clear, area);
    match modal {
        Modal::Approval(modal) if modal.feedback_mode => {
            let block = Block::default()
                .title(" deny with feedback ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::theme::TEXT));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new(modal.feedback.as_str()).wrap(Wrap { trim: false }),
                inner,
            );
        }
        Modal::Approval(modal) => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(6)])
                .split(area);
            let summary = Paragraph::new(format!(
                "{}\n\n{}\n\n{}",
                modal.tool, modal.summary, modal.details
            ))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(" approval required ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(crate::theme::TEXT)),
            );
            frame.render_widget(summary, chunks[0]);
            let always = if matches!(modal.tool.as_str(), "web_fetch" | "web_search") {
                "Allow all network access this session"
            } else {
                "Allow all local changes this session"
            };
            let choices = ["Approve once", always, "Deny", "Deny with feedback…"];
            let mut state = ListState::default().with_selected(Some(modal.selected));
            frame.render_stateful_widget(
                List::new(choices.map(ListItem::new))
                    .block(Block::default().borders(Borders::ALL))
                    .highlight_symbol("▶ ")
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
                chunks[1],
                &mut state,
            );
        }
        Modal::Ask(modal) => {
            let block = Block::default()
                .title(" question from agent ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::theme::TURQUOISE));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new(format!("{}\n\n> {}", modal.question, modal.answer))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
        Modal::Editor(modal) => {
            let area = centered_rect(92, 82, frame.area());
            frame.render_widget(Clear, area);
            let block = Block::default()
                .title(" Prompt editor · F2/Ctrl-Enter save · Esc cancel ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::theme::TURQUOISE));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(
                Paragraph::new(modal.text.as_str()).wrap(Wrap { trim: false }),
                inner,
            );
        }
        Modal::Setup(modal) => {
            let area = centered_rect(76, 58, frame.area());
            frame.render_widget(Clear, area);
            let block = Block::default()
                .title(" Set up Craftsman ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(crate::theme::TURQUOISE));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(2),
                    Constraint::Length(2),
                    Constraint::Length(2),
                    Constraint::Min(2),
                ])
                .split(inner);
            let tabs = if let Some((provider, protocol)) = &modal.provider {
                format!("{provider} · {protocol}\nCtrl-U clear current field")
            } else {
                "Model connection\nEnter its configured endpoint, API key and model".into()
            };
            frame.render_widget(
                Paragraph::new(tabs).style(Style::default().fg(crate::theme::TURQUOISE)),
                chunks[0],
            );
            for (index, label) in modal.labels().into_iter().enumerate() {
                let secret = index == 1;
                let value = if secret {
                    "•".repeat(modal.fields[index].chars().count())
                } else {
                    modal.fields[index].clone()
                };
                let style = if modal.field == index {
                    Style::default()
                        .fg(crate::theme::TURQUOISE)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                frame.render_widget(
                    Paragraph::new(format!("{label}: {value}")).style(style),
                    chunks[index + 1],
                );
            }
            frame.render_widget(
                Paragraph::new(if modal.provider.is_some() {
                    "Enter save and select · Esc cancel · other providers and models are retained"
                } else {
                    "API key may be empty for a local proxy · Enter finish · Esc cancel"
                })
                .style(Style::default().fg(crate::theme::MUTED)),
                chunks[4],
            );
        }
        Modal::Selection(modal) => {
            let area = centered_rect(82, 72, frame.area());
            frame.render_widget(Clear, area);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(2)])
                .split(area);
            let items = modal.items.iter().map(|item| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        item.label.clone(),
                        Style::default()
                            .fg(crate::theme::TURQUOISE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(if item.detail.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", item.detail)
                    }),
                ]))
            });
            let mut state = ListState::default().with_selected(
                (!modal.items.is_empty()).then_some(modal.selected.min(modal.items.len() - 1)),
            );
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .title(format!(" {} ", modal.title))
                            .borders(Borders::ALL)
                            .border_style(Style::default().fg(crate::theme::TURQUOISE)),
                    )
                    .highlight_symbol("▶ ")
                    .highlight_style(
                        Style::default()
                            .bg(crate::theme::SELECTION)
                            .add_modifier(Modifier::BOLD),
                    ),
                chunks[0],
                &mut state,
            );
            frame.render_widget(
                Paragraph::new(modal.hint.as_str()).style(Style::default().fg(crate::theme::MUTED)),
                chunks[1],
            );
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

pub(super) fn show_setup_modal(app: &mut App) {
    app.modal = Some(Modal::Setup(SetupModal::new()));
    app.dirty = true;
}

pub(super) fn show_provider_setup_modal(
    app: &mut App,
    provider: String,
    protocol: &'static str,
    fields: Vec<String>,
) {
    app.modal = Some(Modal::Setup(SetupModal {
        provider: Some((provider, protocol)),
        fields,
        field: 0,
    }));
    app.dirty = true;
}

pub(super) fn show_voice_model_modal(app: &mut App) {
    let models = crate::voice::model_names().collect::<Vec<_>>();
    let selected = models
        .iter()
        .position(|model| *model == app.voice.model)
        .unwrap_or(0);
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Voice transcription model".to_string(),
        items: models
            .into_iter()
            .map(|model| {
                let current = model == app.voice.model;
                let downloaded = crate::voice::model_downloaded(model);
                SelectionItem {
                    label: model.to_string(),
                    detail: match (current, downloaded) {
                        (true, true) => "current · downloaded".to_string(),
                        (true, false) => "current · downloads on first activation".to_string(),
                        (false, true) => "downloaded".to_string(),
                        (false, false) => "downloads on first activation".to_string(),
                    },
                    enter_action: Some(AppAction::SelectVoiceModel(model.to_string())),
                    delete_action: None,
                }
            })
            .collect(),
        selected,
        hint: "↑/↓ select · Enter choose · Esc close · /voice model <name> also works".to_string(),
    }));
    app.dirty = true;
}

pub(super) fn show_mode_modal(cfg: &Config, app: &mut App) {
    let modes = crate::config::PermissionMode::ALL;
    let selected = modes
        .iter()
        .position(|mode| *mode == cfg.permission_mode)
        .unwrap_or(0);
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Permission mode".to_string(),
        items: modes
            .into_iter()
            .map(|mode| SelectionItem {
                label: mode.as_str().to_string(),
                detail: if mode == cfg.permission_mode {
                    format!("current · {}", mode.description())
                } else {
                    mode.description().to_string()
                },
                enter_action: Some(AppAction::SelectMode(mode)),
                delete_action: None,
            })
            .collect(),
        selected,
        hint: "↑/↓ select · Enter apply · Shift-Tab cycles · Esc close".to_string(),
    }));
    app.dirty = true;
}

pub(super) fn show_queue_modal(app: &mut App) {
    if app.queued_messages.is_empty() {
        app.push("(no queued follow-ups)");
        return;
    }
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Queued follow-ups".to_string(),
        items: app
            .queued_messages
            .iter()
            .enumerate()
            .map(|(index, message)| SelectionItem {
                label: format!(
                    "{}. {}",
                    index + 1,
                    crate::util::one_line(&display_submission(message), 64)
                ),
                detail: String::new(),
                enter_action: None,
                delete_action: Some(AppAction::RemoveQueued(index)),
            })
            .collect(),
        selected: 0,
        hint: "↑/↓ select · d remove · Esc close".to_string(),
    }));
    app.dirty = true;
}

pub(super) fn show_tasks_modal(app: &mut App, tasks: Vec<craftsman_harness::AgentTaskInfo>) {
    if tasks.is_empty() {
        app.push("(no subagent tasks)");
        return;
    }
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Subagent tasks".to_string(),
        items: tasks
            .into_iter()
            .map(|task| {
                let running = task.status == craftsman_harness::AgentStatus::Running;
                let name = task.name;
                SelectionItem {
                    label: name.clone(),
                    detail: format!("{:?} · {}", task.status, task.progress),
                    enter_action: None,
                    delete_action: running.then_some(AppAction::CancelTask(name)),
                }
            })
            .collect(),
        selected: 0,
        hint: "↑/↓ select · d cancel running task · /tasks message <name> <text>".to_string(),
    }));
    app.dirty = true;
}

pub(super) fn show_skills_modal(app: &mut App, skills: Vec<crate::skills::Skill>) {
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Local skills".into(),
        items: skills
            .into_iter()
            .map(|skill| SelectionItem {
                label: skill.name,
                detail: format!(
                    "{} · {}",
                    crate::util::one_line(&skill.description, 70),
                    skill.path.display()
                ),
                enter_action: None,
                delete_action: None,
            })
            .collect(),
        selected: 0,
        hint: "↑/↓ browse · Esc close · read SKILL.md to use a skill".into(),
    }));
    app.dirty = true;
}

pub(super) fn show_harness_modal(cfg: &Config, app: &mut App) {
    let current = crate::harness_profiles::configured_harness_preset(cfg);
    let presets = crate::harness_profiles::harness_presets();
    let selected = presets
        .iter()
        .position(|preset| preset.id == current.id)
        .unwrap_or_default();
    app.modal = Some(Modal::Selection(SelectionModal {
        title: "Select harness profile".to_string(),
        items: presets
            .iter()
            .map(|preset| SelectionItem {
                label: preset.label.to_string(),
                detail: if preset.id == current.id {
                    format!("current · {}", preset.description)
                } else {
                    preset.description.to_string()
                },
                enter_action: Some(AppAction::SelectHarness(preset.id.to_string())),
                delete_action: None,
            })
            .collect(),
        selected,
        hint: "↑/↓ select · Enter apply and rebuild · Esc close · /harness status inspects"
            .to_string(),
    }));
    app.dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_skills_menu_has_no_remote_or_destructive_actions() {
        let mut app = App::new(&Config::default());
        show_skills_modal(
            &mut app,
            vec![crate::skills::Skill {
                name: "Local skill".into(),
                description: "Read local instructions".into(),
                path: std::path::PathBuf::from("workspace/.craftsman/skills/local/SKILL.md"),
            }],
        );
        let Some(Modal::Selection(picker)) = &app.modal else {
            panic!("missing skills menu")
        };
        assert_eq!(picker.title, "Local skills");
        assert!(
            picker
                .items
                .iter()
                .all(|item| item.enter_action.is_none() && item.delete_action.is_none())
        );
        assert!(picker.items[0].detail.contains("SKILL.md"));
    }

    #[test]
    fn setup_is_a_model_connection_and_does_not_offer_a_platform_tab() {
        let mut app = App::new(&Config::default());
        show_setup_modal(&mut app);
        let Some(Modal::Setup(setup)) = &mut app.modal else {
            panic!("missing setup")
        };
        assert_eq!(setup.labels(), ["Base URL", "API key", "Model"]);
        setup.fields = vec![
            "http://127.0.0.1:4000/v1".into(),
            String::new(),
            "local-model".into(),
        ];
        setup.field = 2;
        handle_modal_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        handle_modal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(&app.pending_action, Some(AppAction::SetupDirect{base_url,api_key,model}) if base_url=="http://127.0.0.1:4000/v1" && api_key.is_empty() && model=="local-model")
        );
    }

    #[test]
    fn selection_enter_emits_action() {
        let mut app = App::new(&Config::default());
        app.modal = Some(Modal::Selection(SelectionModal {
            title: "Select model".into(),
            items: vec![SelectionItem {
                label: "model-a".into(),
                detail: String::new(),
                enter_action: Some(AppAction::SelectModel("model-a".into())),
                delete_action: None,
            }],
            selected: 0,
            hint: String::new(),
        }));
        handle_modal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.pending_action,
            Some(AppAction::SelectModel("model-a".into()))
        );
        assert!(app.modal.is_none());
    }

    #[cfg(all(feature = "voice", any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn voice_model_command_opens_selectable_model_picker() {
        let mut app = App::new(&Config::default());
        app.voice.available = true;
        app.voice.model = "small-q5_1".into();
        show_voice_model_modal(&mut app);
        let Some(Modal::Selection(modal)) = app.modal.as_ref() else {
            panic!("expected voice model selection modal");
        };
        assert_eq!(modal.title, "Voice transcription model");
        assert_eq!(
            modal
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            ["large-v3-turbo-q5_0", "small-q5_1", "base-q5_1"]
        );
        assert_eq!(modal.selected, 1);
        assert_eq!(
            modal.items[2].enter_action,
            Some(AppAction::SelectVoiceModel("base-q5_1".into()))
        );
    }

    #[test]
    fn harness_modal_lists_selectable_example_profiles() {
        let cfg = Config::default();
        let mut app = App::new(&cfg);
        show_harness_modal(&cfg, &mut app);
        let Some(Modal::Selection(modal)) = app.modal.as_ref() else {
            panic!("expected harness selection modal");
        };
        assert_eq!(modal.title, "Select harness profile");
        assert_eq!(
            modal
                .items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            [
                "Automatic",
                "General",
                "OpenAI / Codex",
                "Claude",
                "NVIDIA Nemotron",
            ]
        );
        assert_eq!(modal.selected, 0);
        assert_eq!(
            modal.items[3].enter_action,
            Some(AppAction::SelectHarness("claude".into()))
        );
    }
}
