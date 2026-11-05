//! The REPL's color theme — light-green craftsman accents plus a subtle animated
//! "RGB" shimmer on the input box's top border.
//!
//! Everything emits 256-color indexed codes (`ESC[38;5;Nm`), never 24-bit
//! truecolor: macOS Terminal.app has no truecolor support, and tmux
//! down-converts unpredictably — the indexed palette renders the same
//! everywhere. All functions honor `NO_COLOR` (any value set ⇒ plain text),
//! per https://no-color.org.
//!
//! The shimmer takes no timers: [`top_border`] derives its colors purely from
//! the elapsed-milliseconds value the chrome already has, so it advances only
//! when the chrome repaints anyway (every keystroke + the idle tick).

/// Light green — the primary accent (`#90EE90` ≈ palette 120).
const ACCENT: u8 = 120;
/// Muted green — frames and secondary marks.
const ACCENT_DIM: u8 = 71;
/// Gray — de-emphasized text (status line, hints).
const GRAY: u8 = 245;

/// Soft pastel hue ring for the top-border shimmer: green → teal → blue →
/// violet → pink → back to green. Adjacent entries are near neighbors in hue,
/// so a border sampling consecutive entries reads as a gentle gradient, not
/// color bars.
const RING: [u8; 16] = [
    120, 121, 122, 123, 117, 111, 105, 141, 177, 213, 218, 217, 223, 229, 193, 156,
];

/// One ring step per this many ms — full sweep ≈ 10s. Slow enough to feel
/// like ambient lighting, fast enough to be alive.
const STEP_MS: u128 = 600;

/// Segments the border rule is split into. Few enough to keep the paint
/// string small (the chrome repaints per keystroke), enough for a visible
/// gradient across the width.
const SEGMENTS: usize = 10;

/// Static per-line colors for the banner art: violet at the top fading to the
/// light-green accent at the bottom.
const BANNER_FADE: [u8; 6] = [141, 135, 111, 117, 123, ACCENT];

#[cfg(test)]
thread_local! {
    static COLOR_OFF_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

fn color_off() -> bool {
    #[cfg(test)]
    {
        COLOR_OFF_OVERRIDE
            .with(std::cell::Cell::get)
            .unwrap_or(false)
    }
    #[cfg(not(test))]
    {
        std::env::var_os("NO_COLOR").is_some()
    }
}

fn fg(idx: u8, s: &str) -> String {
    format!("\x1b[38;5;{idx}m{s}\x1b[39m")
}

/// Light green, bold — the primary accent (`❯`, picker selection, model name).
pub fn accent(s: &str) -> String {
    if color_off() {
        return s.to_string();
    }
    format!("\x1b[1;38;5;{ACCENT}m{s}\x1b[22;39m")
}

/// Muted green — box frames, secondary marks.
pub fn accent_dim(s: &str) -> String {
    if color_off() {
        return s.to_string();
    }
    fg(ACCENT_DIM, s)
}

/// Gray — de-emphasized text.
pub fn dim(s: &str) -> String {
    if color_off() {
        return s.to_string();
    }
    fg(GRAY, s)
}

/// The animated top border: `╭` + rule + `╮`, the rule split into
/// [`SEGMENTS`] spans whose colors walk the pastel ring, phase-shifted by
/// elapsed time — a slow shimmer sweeping along the border. Corners take the
/// ring color at their end of the rule. Plain when `NO_COLOR` or too narrow.
pub fn top_border(cols: usize, phase_ms: u128) -> String {
    let w = cols.saturating_sub(2);
    if color_off() || w < SEGMENTS * 2 {
        return format!("╭{}╮", "─".repeat(w));
    }
    let phase = (phase_ms / STEP_MS) as usize;
    let seg_len = w / SEGMENTS;
    let mut out = fg(RING[phase % RING.len()], "╭");
    let mut drawn = 0;
    for seg in 0..SEGMENTS {
        // Last segment absorbs the rounding remainder so the rule always
        // spans exactly `w` columns.
        let len = if seg == SEGMENTS - 1 {
            w - drawn
        } else {
            seg_len
        };
        drawn += len;
        let idx = RING[(phase + seg) % RING.len()];
        out.push_str(&fg(idx, &"─".repeat(len)));
    }
    out.push_str(&fg(RING[(phase + SEGMENTS - 1) % RING.len()], "╮"));
    out
}

/// The static bottom border: `╰` + rule + `╯` in the muted frame green.
pub fn bottom_border(cols: usize) -> String {
    let plain = format!("╰{}╯", "─".repeat(cols.saturating_sub(2)));
    accent_dim(&plain)
}

/// Colorize the fitted status line: the whole line gray, with the model name
/// (text before the first " · ") re-tinted in the accent green. MUST be given
/// the already-width-fitted plain line — the replacement only inserts
/// zero-width escape bytes, so the padding math stays intact.
pub fn status_line(fitted: &str) -> String {
    if color_off() {
        return fitted.to_string();
    }
    match fitted.split_once(" · ") {
        Some((model, _)) if !model.trim().is_empty() => {
            let colored_model = format!("\x1b[38;5;{ACCENT}m{model}\x1b[38;5;{GRAY}m");
            let body = fitted.replacen(model, &colored_model, 1);
            format!("\x1b[38;5;{GRAY}m{body}\x1b[39m")
        }
        _ => fg(GRAY, fitted),
    }
}

/// Colorize the boot banner: each ASCII-art line takes one step of a static
/// violet→green fade; the trailing "(rs)" tag and any non-art line render
/// dim. Art lines are recognized by their block-drawing characters.
pub fn banner(raw: &str) -> String {
    if color_off() {
        return raw.to_string();
    }
    let mut art_line = 0usize;
    raw.lines()
        .map(|line| {
            if line.contains('█') || line.contains('╗') || line.contains('╚') {
                let idx = BANNER_FADE[art_line.min(BANNER_FADE.len() - 1)];
                art_line += 1;
                // Keep the "(rs)" tag on the last art line understated.
                match line.split_once("(rs)") {
                    Some((art, _)) => format!("{}{}", fg(idx, art), dim("(rs)")),
                    None => fg(idx, line),
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip `ESC[...m` SGR sequences (the only kind theme emits).
    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for d in chars.by_ref() {
                    if d == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn top_border_spans_exact_width_for_any_phase() {
        for phase in [0u128, 599, 600, 5_400, 123_456_789] {
            let b = top_border(100, phase);
            let plain = strip(&b);
            assert_eq!(plain.chars().count(), 100, "phase {phase}: {plain:?}");
            assert!(plain.starts_with('╭') && plain.ends_with('╮'));
        }
        // Narrow terminals fall back to plain but still exact.
        let narrow = top_border(10, 0);
        assert_eq!(strip(&narrow).chars().count(), 10);
    }

    #[test]
    fn bottom_border_spans_exact_width() {
        assert_eq!(strip(&bottom_border(80)).chars().count(), 80);
    }

    #[test]
    fn ring_walk_changes_color_over_time() {
        // Two phases one step apart must differ (the shimmer actually moves).
        let a = top_border(100, 0);
        let b = top_border(100, STEP_MS);
        assert_ne!(a, b);
        // And the sweep wraps cleanly after a full ring.
        let wrapped = top_border(100, STEP_MS * RING.len() as u128);
        assert_eq!(a, wrapped);
    }

    #[test]
    fn status_line_keeps_visible_width() {
        let fitted = "gpt-4o · srv:x · tokens:0";
        assert_eq!(strip(&status_line(fitted)), fitted);
    }

    #[test]
    fn banner_colors_art_and_dims_rs_tag() {
        let raw = "\n███╗ ███╗\n╚═╝  ╚═╝  (rs)\n";
        let out = banner(raw);
        assert_eq!(strip(&out), raw.trim_end_matches('\n').to_string() + "");
        // Art got color codes; the (rs) tag got the gray code.
        assert!(out.contains("38;5;141"), "first art line violet: {out:?}");
        assert!(out.contains("38;5;245m(rs)"), "rs tag dim: {out:?}");
    }

    /// All NO_COLOR assertions live in ONE test: env vars are process-global
    /// and cargo runs tests in parallel.
    #[test]
    fn no_color_disables_everything() {
        COLOR_OFF_OVERRIDE.with(|override_| override_.set(Some(true)));
        let plain_border = top_border(50, 1234);
        let plain_accent = accent("x");
        let plain_status = status_line("m · s");
        let plain_banner = banner("███╗\n");
        COLOR_OFF_OVERRIDE.with(|override_| override_.set(None));
        assert!(!plain_border.contains('\x1b'));
        assert_eq!(plain_accent, "x");
        assert_eq!(plain_status, "m · s");
        assert!(!plain_banner.contains('\x1b'));
    }
}
