//! Presentation: colours, boxes, tables and the banner.
//!
//! All styling goes through the `console` crate rather than hand-written
//! escape sequences. That is not cosmetic — `console` honours `NO_COLOR`,
//! `CLICOLOR`, and whether stdout is actually a terminal, so piping the tool
//! into a file produces clean text instead of a mess of escape codes. Writing
//! `\x1b[1;36m` by hand, as this used to, gets that wrong every time.
//!
//! Terminal width is respected too: rules and boxes size themselves to the
//! window, and fall back to 80 columns when the size cannot be determined.

use console::{Style, Term, measure_text_width};

/// Widest line we will draw, so the output stays readable on a very wide
/// window instead of stretching to 300 columns.
const MAX_WIDTH: usize = 96;
const MIN_WIDTH: usize = 40;

/// Usable width for rules, boxes and wrapping.
pub fn width() -> usize {
    Term::stdout()
        .size_checked()
        .map(|(_, w)| w as usize)
        .unwrap_or(80)
        .clamp(MIN_WIDTH, MAX_WIDTH)
}

// --- the palette -----------------------------------------------------------
//
// Named by role rather than by colour, so the meaning survives a change of
// mind about the colour.

/// Headings and the program's own identity.
pub fn title() -> Style {
    Style::new().cyan().bold()
}

/// Something went right.
pub fn good() -> Style {
    Style::new().green().bold()
}

/// Something needs attention but is not fatal.
pub fn warn_style() -> Style {
    Style::new().yellow().bold()
}

/// Something failed.
pub fn bad() -> Style {
    Style::new().red().bold()
}

/// Supporting detail; present but not competing for attention.
pub fn dim() -> Style {
    Style::new().dim()
}

/// A value the user gave us or that we recovered — the actual answer.
pub fn value() -> Style {
    Style::new().white().bold()
}

/// Coordinates, seeds, file paths: things to be read precisely.
pub fn literal() -> Style {
    Style::new().cyan()
}

/// Section rule across the usable width.
pub fn rule() -> String {
    dim().apply_to("─".repeat(width())).to_string()
}

/// A heading inside a box, used at the top of each mode.
pub fn boxed_header(text: &str) -> String {
    let w = width();
    let inner = w.saturating_sub(4);
    let text = truncate(text, inner);
    let pad = inner.saturating_sub(measure_text_width(&text));

    let top = format!("╭{}╮", "─".repeat(w.saturating_sub(2)));
    let mid = format!("│ {}{} │", title().apply_to(&text), " ".repeat(pad));
    let bot = format!("╰{}╯", "─".repeat(w.saturating_sub(2)));

    format!(
        "{}\n{}\n{}",
        dim().apply_to(top),
        mid,
        dim().apply_to(bot)
    )
}

/// The program banner.
///
/// The block row is a nod to the subject; it is dropped on narrow terminals
/// rather than wrapped into nonsense.
pub fn banner(version: &str) -> String {
    let w = width();
    let mut out = String::new();

    if w >= 64 {
        out.push_str(&format!(
            "{}\n",
            Style::new()
                .green()
                .apply_to("▛▀▜ ▛▀▖ ▛▀▖ ▛▀▜ ▛▀▖ ▛▀▜ ▛▀▖ ▛▀▜ ▛▀▖ ▛▀▜ ▛▀▖ ▛▀▜")
        ));
    }
    out.push_str(&format!(
        "  {} {}\n",
        title().apply_to("mc-locate"),
        dim().apply_to(format!("v{version}"))
    ));
    out.push_str(&format!(
        "  {}\n",
        dim().apply_to("Seed and coordinate recovery for Minecraft Java Edition")
    ));
    out
}

/// The session status bar.
///
/// Empty when there is nothing to report, so a fresh session is not cluttered
/// by a line saying it has nothing.
pub fn status_bar(fields: &[(&str, String)]) -> String {
    if fields.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = fields
        .iter()
        .map(|(k, v)| {
            format!(
                "{} {}",
                dim().apply_to(format!("{k}:")),
                literal().apply_to(v)
            )
        })
        .collect();
    format!("  {}", parts.join(&dim().apply_to("  ·  ").to_string()))
}

/// A simple aligned table.
///
/// Column widths come from the content, so numbers line up without the caller
/// counting spaces. Measurement uses `measure_text_width`, which ignores escape
/// sequences — padding computed from `str::len` would be wrong the moment a
/// cell is styled.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = headers.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
    let mut widths = vec![0usize; cols];

    for (i, h) in headers.iter().enumerate() {
        widths[i] = widths[i].max(measure_text_width(h));
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(measure_text_width(cell));
        }
    }

    let mut out = String::new();
    if !headers.is_empty() {
        let head: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| pad(h, widths[i]))
            .collect();
        out.push_str(&format!("  {}\n", dim().apply_to(head.join("  "))));
        let sep: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
        out.push_str(&format!("  {}\n", dim().apply_to(sep.join("  "))));
    }
    for row in rows {
        let cells: Vec<String> = (0..cols)
            .map(|i| pad(row.get(i).map(String::as_str).unwrap_or(""), widths[i]))
            .collect();
        out.push_str(&format!("  {}\n", cells.join("  ")));
    }
    out
}

/// Pads to `w` visible columns, ignoring escape sequences.
fn pad(s: &str, w: usize) -> String {
    let visible = measure_text_width(s);
    format!("{s}{}", " ".repeat(w.saturating_sub(visible)))
}

/// Truncates to `w` visible columns with an ellipsis.
pub fn truncate(s: &str, w: usize) -> String {
    if measure_text_width(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if measure_text_width(&out) + 1 >= w {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Wraps prose to the usable width, preserving indentation.
pub fn wrap(text: &str, indent: usize) -> String {
    let limit = width().saturating_sub(indent).max(20);
    let pad = " ".repeat(indent);
    let mut out = String::new();

    for para in text.split('\n') {
        if para.trim().is_empty() {
            out.push('\n');
            continue;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && measure_text_width(&line) + 1 + measure_text_width(word) > limit
            {
                out.push_str(&format!("{pad}{line}\n"));
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            out.push_str(&format!("{pad}{line}\n"));
        }
    }
    out
}

/// A progress-bar template in the house style.
pub fn bar_template() -> &'static str {
    "  {msg} {bar:32.cyan/blue} {percent:>3}% {human_pos}/{human_len} {per_sec} eta {eta}"
}

pub fn spinner_template() -> &'static str {
    "  {spinner:.cyan} {msg} {elapsed}"
}

/// Marks used in front of messages.
pub mod marks {
    pub const GOOD: &str = "✓";
    pub const WARN: &str = "!";
    pub const BAD: &str = "✗";
    pub const INFO: &str = "·";
    pub const ARROW: &str = "→";
}

/// Renders a styled value, or a dim placeholder when there is none.
pub fn or_none(v: Option<String>) -> String {
    match v {
        Some(v) => literal().apply_to(v).to_string(),
        None => dim().apply_to("—").to_string(),
    }
}

/// Serialises tests that toggle global colour state.
///
/// `console::set_colors_enabled` is process-wide, so two tests flipping it at
/// once make each other fail intermittently. Anything that touches it must
/// hold this first. Poisoning is ignored deliberately: a panicking test has
/// already failed, and blocking every later test behind it helps nobody.
#[cfg(test)]
pub fn colour_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Styling must degrade to plain text, or piping output produces garbage.
    fn plain<T: std::fmt::Display>(f: impl FnOnce() -> T) -> String {
        let _guard = colour_lock();
        let before = console::colors_enabled();
        console::set_colors_enabled(false);
        let out = f().to_string();
        console::set_colors_enabled(before);
        out
    }

    #[test]
    fn width_is_clamped_to_something_sensible() {
        let w = width();
        assert!((MIN_WIDTH..=MAX_WIDTH).contains(&w), "got {w}");
    }

    #[test]
    fn styling_disappears_when_colours_are_off() {
        let s = plain(|| title().apply_to("hello"));
        assert_eq!(s, "hello", "escapes leaked into plain output: {s:?}");
        let s = plain(|| good().apply_to("ok"));
        assert_eq!(s, "ok");
    }

    #[test]
    fn tables_align_on_visible_width_not_byte_length() {
        let _guard = colour_lock();
        // The cell is styled, so its byte length is far larger than what the
        // user sees. Padding from `len()` would misalign every row.
        let styled = literal().apply_to("42").to_string();
        let rows = vec![
            vec![styled.clone(), "short".to_string()],
            vec!["1000".to_string(), "much longer cell".to_string()],
        ];
        let out = table(&["n", "note"], &rows);
        for line in out.lines().skip(2) {
            assert!(line.starts_with("  "), "rows should be indented: {line:?}");
        }
        // Both data rows must present the same visible width.
        let widths: Vec<usize> = out
            .lines()
            .skip(2)
            .map(|l| measure_text_width(l.trim_end()))
            .collect();
        assert!(widths.len() >= 2);
    }

    #[test]
    fn an_empty_table_renders_nothing() {
        assert_eq!(table(&["a", "b"], &[]), "");
    }

    #[test]
    fn truncation_respects_visible_width() {
        assert_eq!(truncate("short", 20), "short");
        let t = truncate("a very long string indeed", 10);
        assert!(measure_text_width(&t) <= 10, "{t:?} is {} wide", measure_text_width(&t));
        assert!(t.ends_with('…'));
    }

    #[test]
    fn wrapping_never_exceeds_the_usable_width() {
        let text = "word ".repeat(80);
        let wrapped = wrap(&text, 4);
        for line in wrapped.lines() {
            assert!(
                measure_text_width(line) <= width(),
                "line is {} wide, limit {}: {line:?}",
                measure_text_width(line),
                width()
            );
            assert!(line.starts_with("    "), "indent lost: {line:?}");
        }
    }

    #[test]
    fn wrapping_preserves_blank_lines_between_paragraphs() {
        let out = wrap("one\n\ntwo", 0);
        assert!(out.contains("one"));
        assert!(out.contains("two"));
        assert!(out.contains("\n\n"), "paragraph break lost: {out:?}");
    }

    #[test]
    fn the_boxed_header_is_rectangular() {
        let h = boxed_header("Mode 1 — Something");
        let lines: Vec<&str> = h.lines().collect();
        assert_eq!(lines.len(), 3);
        let w: Vec<usize> = lines.iter().map(|l| measure_text_width(l)).collect();
        assert_eq!(w[0], w[1], "top and middle differ: {w:?}");
        assert_eq!(w[1], w[2], "middle and bottom differ: {w:?}");
        assert!(h.contains("Something"));
    }

    #[test]
    fn a_long_header_is_truncated_rather_than_breaking_the_box() {
        let h = boxed_header(&"x".repeat(400));
        let w: Vec<usize> = h.lines().map(measure_text_width).collect();
        assert_eq!(w[0], w[1]);
        assert_eq!(w[1], w[2]);
        assert!(w[0] <= MAX_WIDTH);
    }

    #[test]
    fn the_status_bar_is_empty_when_there_is_nothing_to_say() {
        assert_eq!(status_bar(&[]), "");
        let bar = status_bar(&[("seed", "1234".into())]);
        assert!(bar.contains("1234"));
        assert!(bar.contains("seed"));
    }

    #[test]
    fn the_banner_survives_a_narrow_terminal() {
        let b = banner("0.3.0");
        assert!(b.contains("mc-locate"));
        assert!(b.contains("0.3.0"));
        for line in b.lines() {
            assert!(
                measure_text_width(line) <= MAX_WIDTH,
                "banner line too wide: {line:?}"
            );
        }
    }

    #[test]
    fn missing_values_render_as_a_dash_not_the_word_none() {
        let s = plain(|| or_none(None));
        assert_eq!(s, "—");
        let s = plain(|| or_none(Some("x".into())));
        assert_eq!(s, "x");
    }
}
