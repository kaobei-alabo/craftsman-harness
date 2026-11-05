//! Inline list picker with arrow-key navigation AND number entry.
//!
//! Renders a numbered list inline (no full-screen takeover), lets the user
//! move with ↑/↓ (or Ctrl-P/Ctrl-N), jump by typing the index digits, and
//! confirm with Enter. Esc / Ctrl-C / Ctrl-G cancels. The list is erased when
//! done, so the transcript stays clean — matching prompt_toolkit's
//! `erase_when_done=True`.
//!
//! Used by the approval gate (`hitl.rs`) and the skills browser
//! (`skills_browser.rs`). It returns the
//! chosen index or `None` when cancelled or when there's no TTY (callers keep
//! their plain-text fallback for that case).
//!
//! Long lists window at [`WINDOW`] rows, scrolling with the selection
//! (windowing keeps tall lists inside the
//! transcript scroll region so the pinned input box + status bar are never
//! overdrawn).

const BOTTOM_OVERHEAD: u16 = 5;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use std::io::{IsTerminal, Write};

/// Max item rows drawn at once; longer lists scroll within this window.
const WINDOW: usize = 10;

const RESET: &str = "\x1b[0m";

/// What a key did to the picker.
#[derive(Debug, PartialEq)]
enum Step {
    Continue,
    Confirm,
    Cancel,
}

/// Pure selection state — keyboard logic without any terminal I/O, so the
/// wrap/jump/backspace behavior is unit-testable headlessly.
struct PickerState {
    idx: usize,
    len: usize,
    /// Digits typed so far ("12" jumps to item 12; arrows clear it).
    buffer: String,
}

impl PickerState {
    fn new(len: usize, start: usize) -> Self {
        PickerState {
            idx: start.min(len.saturating_sub(1)),
            len,
            buffer: String::new(),
        }
    }

    fn step(&mut self, delta: isize) {
        let n = self.len as isize;
        self.idx = ((self.idx as isize + delta).rem_euclid(n)) as usize;
        self.buffer.clear();
    }

    /// Re-apply the digit buffer: jump when it names a valid 1-based index.
    fn jump_from_buffer(&mut self) {
        if let Ok(n) = self.buffer.parse::<usize>() {
            if (1..=self.len).contains(&n) {
                self.idx = n - 1;
            }
        }
    }

    fn apply(&mut self, code: KeyCode, mods: KeyModifiers) -> Step {
        match (mods, code) {
            (_, KeyCode::Up) => self.step(-1),
            (_, KeyCode::Down) => self.step(1),
            (KeyModifiers::CONTROL, KeyCode::Char('p')) => self.step(-1),
            (KeyModifiers::CONTROL, KeyCode::Char('n')) => self.step(1),
            (_, KeyCode::Home) => {
                self.idx = 0;
                self.buffer.clear();
            }
            (_, KeyCode::End) => {
                self.idx = self.len - 1;
                self.buffer.clear();
            }
            (KeyModifiers::NONE, KeyCode::Char(c)) if c.is_ascii_digit() => {
                self.buffer.push(c);
                self.jump_from_buffer();
            }
            (_, KeyCode::Backspace) => {
                self.buffer.pop();
                self.jump_from_buffer();
            }
            (_, KeyCode::Enter) => return Step::Confirm,
            (_, KeyCode::Esc) => return Step::Cancel,
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => return Step::Cancel,
            (KeyModifiers::CONTROL, KeyCode::Char('g')) => return Step::Cancel,
            _ => {}
        }
        Step::Continue
    }
}

/// Whether the interactive picker can run at all: BOTH streams must be
/// terminals (raw-mode key reads need stdin, drawing needs stdout). Callers
/// gate on this and fall back to their plain text prompts otherwise — checking
/// only stdout would turn a piped-stdin run's picker into an instant `None`,
/// which an approval gate would misread as "denied".
pub fn picker_available() -> bool {
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

/// Show an inline picker; return the chosen item's index, or `None` when
/// cancelled or when [`picker_available`] is false (callers fall back to
/// their plain prompt). `current` pre-selects an item (highlighted on open,
/// tagged "(current)" in the list).
pub fn pick_from_list(title: &str, labels: &[String], current: Option<usize>) -> Option<usize> {
    if labels.is_empty() || !picker_available() {
        return None;
    }
    let mut state = PickerState::new(labels.len(), current.unwrap_or(0));

    crossterm::terminal::enable_raw_mode().ok()?;
    let result = run_picker(&mut state, title, labels, current);
    let _ = crossterm::terminal::disable_raw_mode();
    result
}

/// The raw-mode session: make room, draw/redraw at a fixed origin, read keys,
/// erase on exit. Split from `pick_from_list` so raw mode is ALWAYS disabled
/// on any early return.
fn run_picker(
    state: &mut PickerState,
    title: &str,
    labels: &[String],
    current: Option<usize>,
) -> Option<usize> {
    let mut out = std::io::stdout();

    // Window the item rows to whichever is smallest: the item count, the
    // standard cap, or what actually fits in the scroll region (title + items
    // + footer must fit, or the draw would address rows over the pinned
    // chrome / off-screen). The region bottom is the row above the input
    // box's top border when the chrome is active; without chrome it's the
    // last screen row.
    let (cols, rows) = crossterm::terminal::size().ok()?;
    let region_bottom0 = region_bottom_row0(rows);
    let region_rows = region_bottom0 as usize + 1;
    let win = labels
        .len()
        .min(WINDOW)
        .min(region_rows.saturating_sub(2).max(1));
    let height = win as u16 + 2;

    // Make room below the cursor so the whole picker fits without scrolling
    // during redraws: scroll the region up by the shortfall (CSI S — honors
    // DECSTBM, so the pinned input box/status rows are never disturbed), then
    // anchor the draw origin so `origin + height - 1` is the last free row.
    let (_, cur0) = crossterm::cursor::position().ok()?;
    let free_below = region_bottom0.saturating_sub(cur0);
    let deficit = (height - 1).saturating_sub(free_below);
    if deficit > 0 {
        let _ = write!(out, "\x1b[{deficit}S");
    }
    let origin0 = cur0.saturating_sub(deficit);

    let _ = write!(out, "\x1b[?25l"); // hide cursor
    loop {
        draw(&mut out, title, labels, state, current, origin0, win, cols);
        let Ok(ev) = crossterm::event::read() else {
            erase(&mut out, origin0, height);
            return None;
        };
        let Event::Key(k) = ev else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        match state.apply(k.code, k.modifiers) {
            Step::Continue => {}
            Step::Confirm => {
                erase(&mut out, origin0, height);
                return Some(state.idx);
            }
            Step::Cancel => {
                erase(&mut out, origin0, height);
                return None;
            }
        }
    }
}

/// The scroll-region bottom (0-based): when the screen is tall enough for the
/// bottom chrome (`BOTTOM_OVERHEAD` rows), the region ends above it;
/// otherwise the whole screen scrolls. The approval gate calls the picker with
/// no access to the screen layout, so the formula lives here (the skills
/// browser keeps its own copy).
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
    title: &str,
    labels: &[String],
    state: &PickerState,
    current: Option<usize>,
    origin0: u16,
    win: usize,
    cols: u16,
) {
    // Keep the selection inside the visible window.
    let skip = if state.idx < win {
        0
    } else {
        (state.idx + 1).saturating_sub(win)
    };
    // Room for a label after "▶ 123. " and before "  (current)"; labels are
    // clamped so a long thread title can't wrap onto (and corrupt) the next
    // absolutely-addressed row. Control chars are flattened for the same
    // reason.
    let max_label = (cols as usize).saturating_sub(20).max(8);

    let mut buf = String::new();
    // Title row.
    buf.push_str(&format!(
        "\x1b[{};1H\x1b[2K\x1b[1m{title}\x1b[0m",
        origin0 + 1
    ));
    // Item rows.
    for (row, i) in (skip..skip + win).enumerate() {
        let label: String = labels[i]
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(max_label)
            .collect();
        let sel = i == state.idx;
        let tail = if current == Some(i) {
            "  (current)"
        } else {
            ""
        };
        let text = format!(
            "{}{:>3}. {label}{tail}",
            if sel { "▶ " } else { "  " },
            i + 1
        );
        let text = if sel {
            crate::theme::accent(&text)
        } else {
            text
        };
        buf.push_str(&format!(
            "\x1b[{};1H\x1b[2K{text}{RESET}",
            origin0 + 2 + row as u16,
        ));
    }
    // Footer row: keys + typed digits + windowed position.
    let typed = if state.buffer.is_empty() {
        String::new()
    } else {
        format!("   typed: {}", state.buffer)
    };
    let pos = if labels.len() > win {
        format!("   {}/{}", state.idx + 1, labels.len())
    } else {
        String::new()
    };
    let footer = crate::theme::dim(&format!(
        "↑/↓ select · number to jump · Enter confirm · Esc cancel{typed}{pos}"
    ));
    buf.push_str(&format!(
        "\x1b[{};1H\x1b[2K{footer}{RESET}",
        origin0 + 1 + win as u16 + 1,
    ));
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}

/// Clear the picker's rows and park the cursor at the draw origin so the
/// caller's next print lands where the picker was. Per-line 2K (not 0J: that
/// would wipe the pinned chrome below). Also re-shows the cursor.
fn erase(out: &mut std::io::Stdout, origin0: u16, height: u16) {
    let mut buf = String::new();
    for r in 0..height {
        buf.push_str(&format!("\x1b[{};1H\x1b[2K", origin0 + 1 + r));
    }
    buf.push_str(&format!("\x1b[{};1H\x1b[?25h", origin0 + 1));
    let _ = out.write_all(buf.as_bytes());
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> KeyModifiers {
        KeyModifiers::NONE
    }

    #[test]
    fn arrows_wrap_both_directions() {
        let mut s = PickerState::new(3, 0);
        assert_eq!(s.apply(KeyCode::Up, none()), Step::Continue);
        assert_eq!(s.idx, 2, "up from 0 wraps to last");
        s.apply(KeyCode::Down, none());
        assert_eq!(s.idx, 0, "down from last wraps to first");
        s.apply(KeyCode::Down, none());
        assert_eq!(s.idx, 1);
    }

    #[test]
    fn ctrl_p_n_move_like_arrows() {
        let mut s = PickerState::new(3, 1);
        s.apply(KeyCode::Char('p'), KeyModifiers::CONTROL);
        assert_eq!(s.idx, 0);
        s.apply(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(s.idx, 1);
    }

    #[test]
    fn digits_jump_multi_digit_and_backspace_edits() {
        let mut s = PickerState::new(15, 0);
        s.apply(KeyCode::Char('1'), none());
        assert_eq!(s.idx, 0, "typing '1' selects item 1");
        s.apply(KeyCode::Char('2'), none());
        assert_eq!(s.idx, 11, "buffer '12' selects item 12");
        s.apply(KeyCode::Backspace, none());
        assert_eq!(s.idx, 0, "back to '1'");
        // Arrow clears the buffer.
        s.apply(KeyCode::Down, none());
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn out_of_range_digits_do_not_move() {
        let mut s = PickerState::new(3, 2);
        s.apply(KeyCode::Char('9'), none());
        assert_eq!(s.idx, 2, "9 > len: selection unchanged");
    }

    #[test]
    fn home_end_enter_escape() {
        let mut s = PickerState::new(5, 2);
        s.apply(KeyCode::Home, none());
        assert_eq!(s.idx, 0);
        s.apply(KeyCode::End, none());
        assert_eq!(s.idx, 4);
        assert_eq!(s.apply(KeyCode::Enter, none()), Step::Confirm);
        assert_eq!(s.apply(KeyCode::Esc, none()), Step::Cancel);
        assert_eq!(
            s.apply(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Step::Cancel
        );
    }

    #[test]
    fn start_index_clamped_to_len() {
        let s = PickerState::new(3, 99);
        assert_eq!(s.idx, 2);
    }
}
