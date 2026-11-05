//! Interactive `/skills` browser — tabbed Installed / Available view.
//!
//! Inline (no full-screen takeover), drawn with absolute addressing inside
//! the transcript scroll region — the same pattern as `picker.rs`, so the
//! pinned input box + status bar are never overdrawn. The keyboard logic and
//! row formatting are pure and unit-tested; the raw-mode wiring follows the
//! picker's and is verified live.
//!
//! Keys: ←/→ (or Tab/Shift-Tab) switch tabs · ↑/↓ move · Enter installs or
//! updates the selection · `d` removes (Installed tab) · Esc closes.

use crate::platform_skills::{SkillState, SkillStatus};
const BOTTOM_OVERHEAD: u16 = 5;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::Write;

/// Max item rows drawn at once; longer lists scroll within this window.
const WINDOW: usize = 10;

const RESET: &str = "\x1b[0m";

/// An operation the browser wants performed. The caller owns the actual
/// network/filesystem work (and its async bridging); the browser just renders
/// the result message.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserAction {
    Install(String),
    Update(String),
    Remove(String),
}

/// `(installed_tab, available_tab)`. Installed = anything on disk
/// (installed_version set). Available = anything on the platform
/// (platform_version set). A skill can appear in both.
pub fn split_tabs(statuses: &[SkillStatus]) -> (Vec<SkillStatus>, Vec<SkillStatus>) {
    let installed = statuses
        .iter()
        .filter(|s| s.installed_version.is_some())
        .cloned()
        .collect();
    let available = statuses
        .iter()
        .filter(|s| s.platform_version.is_some())
        .cloned()
        .collect();
    (installed, available)
}

/// One list row (cursor, padded id/name/source,
/// version, state tail).
pub fn format_row(status: &SkillStatus, installed_tab: bool, selected: bool) -> String {
    let lead = if selected { "▶ " } else { "  " };
    let ver = status
        .installed_version
        .as_deref()
        .or(status.platform_version.as_deref())
        .unwrap_or("?");
    let tail = if installed_tab {
        match status.state {
            SkillState::UpdateAvailable => format!(
                "⬆ update → {}",
                status.platform_version.as_deref().unwrap_or("?")
            ),
            SkillState::LocalOnly => "local".to_string(),
            _ => "✓ up to date".to_string(),
        }
    } else if status.installed_version.is_some() {
        "installed".to_string()
    } else {
        "⬇ install".to_string()
    };
    let badge = if status.source.is_empty() {
        String::new()
    } else {
        format!("[{}]", status.source)
    };
    format!(
        "{lead}{:<18} {:<16} {badge:<10} v{ver:<8} {tail}",
        status.id, status.name
    )
}

/// What a key did to the browser.
#[derive(Debug, PartialEq)]
enum Step {
    Continue,
    /// Perform this action, then refresh.
    Act(BrowserAction),
    /// Show this message (no state change).
    Note(&'static str),
    Close,
}

/// Pure selection/tab state — keyboard logic without terminal I/O.
struct BrowserState {
    tab: usize, // 0 = Installed, 1 = Available
    cursor: usize,
}

impl BrowserState {
    fn clamp(&mut self, len: usize) {
        self.cursor = if len == 0 {
            0
        } else {
            self.cursor.min(len - 1)
        };
    }

    /// Apply one key against the current tab's rows.
    fn apply(&mut self, code: KeyCode, mods: KeyModifiers, rows: &[SkillStatus]) -> Step {
        match (mods, code) {
            (_, KeyCode::Left | KeyCode::BackTab) => {
                self.tab = (self.tab + 1) % 2;
                self.cursor = 0;
            }
            (_, KeyCode::Right | KeyCode::Tab) => {
                self.tab = (self.tab + 1) % 2;
                self.cursor = 0;
            }
            (_, KeyCode::Up) => self.cursor = self.cursor.saturating_sub(1),
            (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
                self.cursor = self.cursor.saturating_sub(1)
            }
            (_, KeyCode::Down) => {
                self.cursor += 1;
                self.clamp(rows.len());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
                self.cursor += 1;
                self.clamp(rows.len());
            }
            (_, KeyCode::Enter) => {
                let Some(s) = rows.get(self.cursor) else {
                    return Step::Continue;
                };
                if self.tab == 0 {
                    return match s.state {
                        SkillState::UpdateAvailable => {
                            Step::Act(BrowserAction::Update(s.id.clone()))
                        }
                        _ => Step::Note("already up to date"),
                    };
                }
                return if s.installed_version.is_none() {
                    Step::Act(BrowserAction::Install(s.id.clone()))
                } else {
                    Step::Note("already installed")
                };
            }
            (KeyModifiers::NONE, KeyCode::Char('d')) if self.tab == 0 => {
                if let Some(s) = rows.get(self.cursor) {
                    return Step::Act(BrowserAction::Remove(s.id.clone()));
                }
            }
            (_, KeyCode::Esc) => return Step::Close,
            (KeyModifiers::NONE, KeyCode::Char('q')) => return Step::Close,
            (KeyModifiers::CONTROL, KeyCode::Char('c' | 'g')) => return Step::Close,
            _ => {}
        }
        Step::Continue
    }
}

/// Run the browser. `act` performs an action and returns a status message (or
/// an error message); `refresh` recomputes the statuses after a change.
/// Returns whether anything was installed/updated/removed (the caller rebuilds
/// the agent only then — the skills listing is baked into the system prompt).
pub fn run_skills_browser(
    mut statuses: Vec<SkillStatus>,
    connected: bool,
    mut act: impl FnMut(&BrowserAction) -> Result<String, String>,
    mut refresh: impl FnMut() -> Result<Vec<SkillStatus>, String>,
) -> bool {
    if !crate::picker::picker_available() {
        return false;
    }
    if crossterm::terminal::enable_raw_mode().is_err() {
        return false;
    }
    let changed = run_loop(&mut statuses, connected, &mut act, &mut refresh);
    let _ = crossterm::terminal::disable_raw_mode();
    changed
}

/// The raw-mode session, split out so raw mode is ALWAYS disabled on any
/// early return (same shape as `picker::run_picker`).
fn run_loop(
    statuses: &mut Vec<SkillStatus>,
    connected: bool,
    act: &mut impl FnMut(&BrowserAction) -> Result<String, String>,
    refresh: &mut impl FnMut() -> Result<Vec<SkillStatus>, String>,
) -> bool {
    let mut out = std::io::stdout();
    let mut state = BrowserState { tab: 0, cursor: 0 };
    let mut msg = String::new();
    let mut changed = false;

    // Same geometry dance as the picker: window the rows to what fits in the
    // scroll region, make room below the cursor once (CSI S honors DECSTBM,
    // so the pinned chrome is never disturbed), and draw at a fixed origin.
    let Ok((cols, rows)) = crossterm::terminal::size() else {
        return false;
    };
    let region_bottom0 = region_bottom_row0(rows);
    let region_rows = region_bottom0 as usize + 1;
    let win = WINDOW.min(region_rows.saturating_sub(2).max(1));
    let height = win as u16 + 2; // header + items + footer

    let Ok((_, cur0)) = crossterm::cursor::position() else {
        return false;
    };
    let free_below = region_bottom0.saturating_sub(cur0);
    let deficit = (height - 1).saturating_sub(free_below);
    if deficit > 0 {
        let _ = write!(out, "\x1b[{deficit}S");
    }
    let origin0 = cur0.saturating_sub(deficit);

    let _ = write!(out, "\x1b[?25l"); // hide cursor
    loop {
        let (installed, available) = split_tabs(statuses);
        let rows_now = if state.tab == 0 {
            &installed
        } else {
            &available
        };
        state.clamp(rows_now.len());
        draw(
            &mut out, &installed, &available, &state, &msg, connected, origin0, win, cols,
        );

        let Ok(ev) = crossterm::event::read() else {
            break;
        };
        let Event::Key(k) = ev else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        match state.apply(k.code, k.modifiers, rows_now) {
            Step::Continue => msg.clear(),
            Step::Note(n) => msg = n.to_string(),
            Step::Close => break,
            Step::Act(action) => {
                msg = "working…".to_string();
                draw(
                    &mut out, &installed, &available, &state, &msg, connected, origin0, win, cols,
                );
                match act(&action) {
                    Ok(m) => {
                        msg = m;
                        changed = true;
                        match refresh() {
                            Ok(s) => *statuses = s,
                            Err(e) => msg = format!("refresh failed: {e}"),
                        }
                    }
                    Err(e) => msg = format!("error: {e}"),
                }
            }
        }
    }
    erase(&mut out, origin0, height);
    changed
}

/// The scroll-region bottom (0-based) — same formula as `picker.rs` (see the
/// comment there for why it's duplicated rather than shared via a bar handle).
fn region_bottom_row0(rows: u16) -> u16 {
    if rows > BOTTOM_OVERHEAD + 4 {
        rows - BOTTOM_OVERHEAD - 1
    } else {
        rows - 1
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    out: &mut std::io::Stdout,
    installed: &[SkillStatus],
    available: &[SkillStatus],
    state: &BrowserState,
    msg: &str,
    connected: bool,
    origin0: u16,
    win: usize,
    cols: u16,
) {
    let rows = if state.tab == 0 { installed } else { available };
    let max_w = cols as usize;

    // Header: title + tab labels, the active one accented.
    let tabs = [
        format!("[ Installed ({}) ]", installed.len()),
        format!("[ Available ({}) ]", available.len()),
    ];
    let header = format!(
        "\x1b[1mSkills\x1b[0m   {}   {}   {}",
        if state.tab == 0 {
            crate::theme::accent(&tabs[0])
        } else {
            crate::theme::dim(&tabs[0])
        },
        if state.tab == 1 {
            crate::theme::accent(&tabs[1])
        } else {
            crate::theme::dim(&tabs[1])
        },
        crate::theme::dim("← → tab"),
    );
    let mut buf = format!("\x1b[{};1H\x1b[2K{header}{RESET}", origin0 + 1);

    // Item rows (windowed, selection kept visible).
    let skip = if state.cursor < win {
        0
    } else {
        (state.cursor + 1).saturating_sub(win)
    };
    for r in 0..win {
        let row = origin0 + 2 + r as u16;
        buf.push_str(&format!("\x1b[{row};1H\x1b[2K"));
        let i = skip + r;
        if rows.is_empty() && r == 0 {
            let hint = if state.tab == 0 {
                "(nothing installed)"
            } else if !connected {
                "(connect + /login to see platform skills)"
            } else {
                "(no skills available)"
            };
            buf.push_str(&crate::theme::dim(&format!("  {hint}")));
            buf.push_str(RESET);
            continue;
        }
        let Some(s) = rows.get(i) else { continue };
        let sel = i == state.cursor;
        // Flatten control chars and clamp so a row can never wrap onto (and
        // corrupt) the next absolutely-addressed row.
        let text: String = format_row(s, state.tab == 0, sel)
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(max_w)
            .collect();
        let text = if sel {
            crate::theme::accent(&text)
        } else {
            text
        };
        buf.push_str(&format!("{text}{RESET}"));
    }

    // Footer: keys + last action message + windowed position.
    let keys = if state.tab == 0 {
        "↑↓ move · ⏎ update · d remove · ←→ tabs · esc close"
    } else {
        "↑↓ move · ⏎ install · ←→ tabs · esc close"
    };
    let pos = if rows.len() > win {
        format!("   {}/{}", state.cursor + 1, rows.len())
    } else {
        String::new()
    };
    let m = if msg.is_empty() {
        String::new()
    } else {
        format!("   {msg}")
    };
    let footer: String = format!("{keys}{m}{pos}").chars().take(max_w).collect();
    buf.push_str(&format!(
        "\x1b[{};1H\x1b[2K{}{RESET}",
        origin0 + 2 + win as u16,
        crate::theme::dim(&footer)
    ));
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}

/// Clear the browser's rows and park the cursor at the origin (per-line 2K —
/// 0J would wipe the pinned chrome below). Re-shows the cursor.
fn erase(out: &mut std::io::Stdout, origin0: u16, height: u16) {
    let mut buf = String::new();
    for r in 0..height {
        buf.push_str(&format!("\x1b[{};1H\x1b[2K", origin0 + 1 + r));
    }
    buf.push_str(&format!(
        "\x1b[{origin0};1H\x1b[?25h",
        origin0 = origin0 + 1
    ));
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        id: &str,
        installed: Option<&str>,
        platform: Option<&str>,
        state: SkillState,
    ) -> SkillStatus {
        SkillStatus {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            installed_version: installed.map(String::from),
            platform_version: platform.map(String::from),
            state,
            source: "platform".into(),
        }
    }

    fn none() -> KeyModifiers {
        KeyModifiers::NONE
    }

    #[test]
    fn split_tabs_partitions_and_shares() {
        let all = vec![
            status("both", Some("1"), Some("1"), SkillState::UpToDate),
            status("disk-only", Some("1"), None, SkillState::LocalOnly),
            status("platform-only", None, Some("2"), SkillState::NotInstalled),
        ];
        let (inst, avail) = split_tabs(&all);
        let ids = |v: &[SkillStatus]| v.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&inst), ["both", "disk-only"]);
        assert_eq!(ids(&avail), ["both", "platform-only"]);
    }

    #[test]
    fn format_row_tails_reflect_state() {
        let up = status("a", Some("1"), Some("2"), SkillState::UpdateAvailable);
        assert!(format_row(&up, true, false).contains("⬆ update → 2"));
        let ok = status("a", Some("1"), Some("1"), SkillState::UpToDate);
        assert!(format_row(&ok, true, true).starts_with("▶ "));
        assert!(format_row(&ok, true, true).contains("✓ up to date"));
        let local = status("a", Some("1"), None, SkillState::LocalOnly);
        assert!(format_row(&local, true, false).contains("local"));
        let get = status("a", None, Some("3"), SkillState::NotInstalled);
        assert!(format_row(&get, false, false).contains("⬇ install"));
        assert!(format_row(&ok, false, false).contains("installed"));
    }

    #[test]
    fn enter_maps_to_the_right_action_per_tab() {
        let mut st = BrowserState { tab: 0, cursor: 0 };
        let rows = vec![status(
            "u",
            Some("1"),
            Some("2"),
            SkillState::UpdateAvailable,
        )];
        assert_eq!(
            st.apply(KeyCode::Enter, none(), &rows),
            Step::Act(BrowserAction::Update("u".into()))
        );
        let rows = vec![status("ok", Some("1"), Some("1"), SkillState::UpToDate)];
        assert_eq!(
            st.apply(KeyCode::Enter, none(), &rows),
            Step::Note("already up to date")
        );

        st.tab = 1;
        let rows = vec![status("new", None, Some("1"), SkillState::NotInstalled)];
        assert_eq!(
            st.apply(KeyCode::Enter, none(), &rows),
            Step::Act(BrowserAction::Install("new".into()))
        );
        let rows = vec![status("have", Some("1"), Some("1"), SkillState::UpToDate)];
        assert_eq!(
            st.apply(KeyCode::Enter, none(), &rows),
            Step::Note("already installed")
        );
    }

    #[test]
    fn remove_only_on_installed_tab() {
        let rows = vec![status("x", Some("1"), Some("1"), SkillState::UpToDate)];
        let mut st = BrowserState { tab: 0, cursor: 0 };
        assert_eq!(
            st.apply(KeyCode::Char('d'), none(), &rows),
            Step::Act(BrowserAction::Remove("x".into()))
        );
        st.tab = 1;
        assert_eq!(st.apply(KeyCode::Char('d'), none(), &rows), Step::Continue);
    }

    #[test]
    fn tab_switch_resets_cursor_and_wraps() {
        let rows: Vec<SkillStatus> = vec![];
        let mut st = BrowserState { tab: 0, cursor: 3 };
        st.apply(KeyCode::Right, none(), &rows);
        assert_eq!((st.tab, st.cursor), (1, 0));
        st.apply(KeyCode::Right, none(), &rows);
        assert_eq!(st.tab, 0, "two tabs wrap");
        st.apply(KeyCode::Left, none(), &rows);
        assert_eq!(st.tab, 1);
    }

    #[test]
    fn cursor_moves_clamp_and_empty_rows_are_safe() {
        let rows = vec![
            status("a", Some("1"), None, SkillState::LocalOnly),
            status("b", Some("1"), None, SkillState::LocalOnly),
        ];
        let mut st = BrowserState { tab: 0, cursor: 0 };
        st.apply(KeyCode::Up, none(), &rows);
        assert_eq!(st.cursor, 0, "up at top stays");
        st.apply(KeyCode::Down, none(), &rows);
        st.apply(KeyCode::Down, none(), &rows);
        assert_eq!(st.cursor, 1, "down clamps at last");
        // Empty list: Enter and d are no-ops, Esc closes.
        let empty: Vec<SkillStatus> = vec![];
        assert_eq!(st.apply(KeyCode::Enter, none(), &empty), Step::Continue);
        assert_eq!(st.apply(KeyCode::Esc, none(), &empty), Step::Close);
        assert_eq!(st.apply(KeyCode::Char('q'), none(), &empty), Step::Close);
    }
}
