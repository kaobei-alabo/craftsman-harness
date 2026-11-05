//! Terminal ownership: raw-mode guard, the control-character-safe backend
//! (`SafeBackend`) and the clipboard.

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use std::io::{self, Stdout, stdout};

pub(super) struct TerminalGuard {
    mouse: bool,
}

impl TerminalGuard {
    pub(super) fn enter(mouse: bool) -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let result = if mouse {
            execute!(
                stdout(),
                EnterAlternateScreen,
                EnableBracketedPaste,
                EnableMouseCapture
            )
        } else {
            execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste)
        };
        if let Err(error) = result {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self { mouse })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse {
            let _ = execute!(
                stdout(),
                DisableMouseCapture,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
        } else {
            let _ = execute!(stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        }
    }
}

/// The terminal backend: Crossterm behind [`SafeBackend`].
pub(super) type TuiBackend = SafeBackend<CrosstermBackend<Stdout>>;

/// Neutralizes control characters in anything the UI draws.
///
/// Model output, tool results, resumed transcripts, `/help` text and plugin
/// descriptions all end up as cell symbols, and `Paragraph` does not filter
/// control characters. A symbol holding ESC, BEL or a C1 control would reach
/// the terminal verbatim: title changes, OSC 52 clipboard writes, cursor
/// movement, and fake approval prompts. Cleaning here, at the one place every
/// frame passes through, covers every render path. (The OSC 52 copy helper
/// writes to stdout itself and is not affected.)
pub(super) struct SafeBackend<B: Backend>(pub(super) B);

/// Characters that must never reach the terminal from drawn text: C0, DEL and
/// C1 controls, plus the bidirectional overrides and isolates that can make
/// text read differently from how it is stored.
fn unsafe_for_terminal(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A visible stand-in for an unsafe character: the Unicode control picture
/// for C0 controls and DEL (`ESC` becomes `␛`), a space for TAB, and U+FFFD
/// for the rest.
fn visible_replacement(c: char) -> char {
    match c {
        '\t' => ' ',
        '\u{0}'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if unsafe_for_terminal(c) => '\u{fffd}',
        c => c,
    }
}

/// `Some(cleaned)` when the symbol holds an unsafe character, else `None`.
fn clean_symbol(symbol: &str) -> Option<String> {
    symbol
        .chars()
        .any(unsafe_for_terminal)
        .then(|| symbol.chars().map(visible_replacement).collect())
}

impl<B: Backend> Backend for SafeBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells: Vec<(u16, u16, &Cell)> = content.collect();
        let cleaned: Vec<Option<Cell>> = cells
            .iter()
            .map(|(_, _, cell)| {
                clean_symbol(cell.symbol()).map(|symbol| {
                    let mut fixed = (*cell).clone();
                    fixed.set_symbol(&symbol);
                    fixed
                })
            })
            .collect();
        self.0.draw(
            cells
                .iter()
                .zip(&cleaned)
                .map(|(&(x, y, cell), fixed)| (x, y, fixed.as_ref().unwrap_or(cell))),
        )
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    // The `scrolling-regions` feature is always enabled for ratatui in this
    // crate's Cargo.toml, which makes these two required trait methods.
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.0.scroll_region_up(region, amount)
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.0.scroll_region_down(region, amount)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
pub(super) fn copy_selected_text(_text: &str) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(test))]
pub(super) fn copy_selected_text(text: &str) -> anyhow::Result<()> {
    copy_to_clipboard(text)
}

pub(super) fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(text.as_bytes())?;
        }
        if child.wait()?.success() {
            return Ok(());
        }
    }
    copy_osc52(text)
}

fn copy_osc52(text: &str) -> anyhow::Result<()> {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = stdout();
    write!(out, "\x1b]52;c;{encoded}\x07")?;
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::widgets::{Paragraph, Wrap};

    #[test]
    fn clean_symbol_replaces_unsafe_characters_and_keeps_safe_text() {
        assert_eq!(clean_symbol("\u{1b}").as_deref(), Some("␛"));
        assert_eq!(clean_symbol("\u{7}").as_deref(), Some("␇"));
        assert_eq!(clean_symbol("\u{7f}").as_deref(), Some("␡"));
        assert_eq!(clean_symbol("\t").as_deref(), Some(" "));
        assert_eq!(clean_symbol("\u{9b}").as_deref(), Some("\u{fffd}"));
        assert_eq!(clean_symbol("\u{202e}").as_deref(), Some("\u{fffd}"));
        assert_eq!(clean_symbol("\u{2067}").as_deref(), Some("\u{fffd}"));
        // Ordinary text, wide characters, emoji, combining marks, RTL letters
        // and joiners are untouched.
        for safe in ["a", " ", "é", "漢", "🙂", "e\u{301}", "א", "\u{200d}"] {
            assert_eq!(clean_symbol(safe), None, "{safe:?}");
        }
    }

    #[test]
    fn safe_backend_never_passes_control_characters_to_the_terminal() {
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(SafeBackend(TestBackend::new(48, 6))).unwrap();
        terminal
            .draw(|frame| {
                // Title change, colour, CSI via a C1 byte, a bidi override,
                // a tab and DEL: the shapes an attacker-controlled string takes.
                let hostile = "A\u{1b}]0;PWNED\u{7}B\u{1b}[31mRED\u{9b}2JC\u{202e}rtl\tend\u{7f}";
                frame.render_widget(
                    Paragraph::new(hostile).wrap(Wrap { trim: false }),
                    frame.area(),
                );
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .0
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!text.chars().any(unsafe_for_terminal), "{text:?}");
        // The surrounding text still renders.
        for word in ["PWNED", "RED", "rtl", "end"] {
            assert!(text.contains(word), "{word} missing from {text:?}");
        }
    }
}
