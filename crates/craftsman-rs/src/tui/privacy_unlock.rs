//! Owned secret field only. It reads no stdin and is not selected by any modal.
//! Final integration must route the owned non-echoing terminal's input directly,
//! never through composer/history/UiEvent. This is not an echo-mode proof.
use crate::privacy_key::{KeyError, Passphrase, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
};
use zeroize::Zeroizing;

pub struct SecretField {
    text: Zeroizing<String>,
    closed: bool,
}
impl std::fmt::Debug for SecretField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivacySecretField([private])")
    }
}
impl Default for SecretField {
    fn default() -> Self {
        Self {
            text: Zeroizing::new(String::new()),
            closed: false,
        }
    }
}
impl SecretField {
    pub fn accept_key(&mut self, key: KeyEvent) -> Result<Option<Passphrase>> {
        if self.closed {
            return Err(KeyError::Closed);
        }
        match key.code {
            KeyCode::Esc => {
                self.cancel();
                Err(KeyError::Cancelled)
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel();
                Err(KeyError::Cancelled)
            }
            KeyCode::Enter => {
                let owned = std::mem::take(&mut *self.text);
                let phrase = Passphrase::from_owned(Zeroizing::new(owned.into_bytes()))?;
                Ok(Some(phrase))
            }
            KeyCode::Backspace => {
                self.text.pop();
                Ok(None)
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !character.is_control() =>
            {
                if self
                    .text
                    .len()
                    .checked_add(character.len_utf8())
                    .is_none_or(|n| n > 1024)
                {
                    return Err(KeyError::Passphrase);
                }
                self.text.push(character);
                Ok(None)
            }
            _ => Ok(None),
        }
    }
    /// Caller transfers ownership immediately from the terminal paste event;
    /// whole bounded input is admitted or refused without partial appends.
    pub fn accept_owned_paste(&mut self, paste: Zeroizing<String>) -> Result<()> {
        if self.closed {
            return Err(KeyError::Closed);
        }
        if paste.chars().any(char::is_control)
            || self
                .text
                .len()
                .checked_add(paste.len())
                .is_none_or(|n| n > 1024)
        {
            return Err(KeyError::Passphrase);
        }
        self.text.push_str(&paste);
        Ok(())
    }
    pub fn cancel(&mut self) {
        zeroize::Zeroize::zeroize(&mut *self.text);
        self.closed = true;
    }
    pub fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let count = self
            .text
            .chars()
            .count()
            .min(usize::from(area.width.saturating_sub(2)));
        let bullets = "•".repeat(count);
        frame.render_widget(
            Paragraph::new(bullets).block(
                Block::default()
                    .title("Unlock privacy vault")
                    .borders(Borders::ALL),
            ),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::terminal::SafeBackend;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn actual_owned_secret_field_safe_backend_never_draws_or_debugs_secret() {
        let mut field = SecretField::default();
        field
            .accept_owned_paste(Zeroizing::new("owned UI passphrase".to_owned()))
            .unwrap();
        assert!(!format!("{field:?}").contains("owned UI"));
        let mut terminal = Terminal::new(SafeBackend(TestBackend::new(48, 4))).unwrap();
        terminal.draw(|f| field.render(f, f.area())).unwrap();
        let output = terminal
            .backend()
            .0
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(!output.contains("owned UI passphrase"));
        assert!(output.contains('•'));
        assert!(
            field
                .accept_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap()
                .is_some()
        );
        field.cancel();
        assert_eq!(field.text.len(), 0);
        assert!(matches!(
            field.accept_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            Err(KeyError::Closed)
        ));
    }
    #[test]
    fn whole_owned_paste_refusal_keeps_the_prior_secret_without_trimming() {
        let mut field = SecretField::default();
        field
            .accept_owned_paste(Zeroizing::new(" owned prior ".to_owned()))
            .unwrap();
        assert!(
            field
                .accept_owned_paste(Zeroizing::new("x".repeat(1024)))
                .is_err()
        );
        assert_eq!(field.text.as_str(), " owned prior ");
        assert!(
            field
                .accept_owned_paste(Zeroizing::new("bad\ninput".to_owned()))
                .is_err()
        );
        field.cancel();
    }

    #[test]
    fn actual_secret_cancel_is_distinct_from_pending_or_empty_human_answer() {
        let mut field = SecretField::default();
        assert!(
            field
                .accept_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            field.accept_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Err(KeyError::Cancelled)
        ));
        assert_eq!(field.text.len(), 0);
        let mut control = SecretField::default();
        assert!(matches!(
            control.accept_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Err(KeyError::Cancelled)
        ));
    }
}
