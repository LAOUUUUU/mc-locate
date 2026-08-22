//! Mode 14 — the built-in documentation browser.
//!
//! The pages live as markdown under `docs/` and are embedded with
//! `include_str!`, so they ship inside the binary: no repository, no network,
//! no separate download. The mode list they describe comes from
//! [`crate::modes::MODES`], which is also what the main menu reads — so the
//! documentation cannot silently fall out of step with the program.

use anyhow::Result;

use crate::modes::MODES;
use crate::session::Session;
use crate::ui;

/// The two pages that are not about a single mode.
pub const OVERVIEW: &str = include_str!("../docs/overview.md");
pub const GLOSSARY: &str = include_str!("../docs/glossary.md");

/// Renders a markdown page to the terminal.
///
/// Not a real markdown implementation and not trying to be — just enough to
/// make headings, code blocks and bullets readable at a glance. Anything it
/// does not recognise passes through unchanged, which is the right failure
/// mode for text the user needs to read.
pub fn render(page: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;

    for line in page.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            out.push_str(&format!("    \x1b[2m{line}\x1b[0m\n"));
            continue;
        }
        if let Some(h) = line.strip_prefix("# ") {
            out.push_str(&format!("\n\x1b[1;36m{h}\x1b[0m\n{}\n", "─".repeat(h.chars().count())));
        } else if let Some(h) = line.strip_prefix("## ") {
            out.push_str(&format!("\n\x1b[1m{h}\x1b[0m\n"));
        } else if let Some(item) = line.strip_prefix("* ") {
            out.push_str(&format!("  • {}\n", emphasis(item)));
        } else if line.starts_with("    ") {
            out.push_str(&format!("\x1b[2m{line}\x1b[0m\n"));
        } else {
            out.push_str(&format!("{}\n", emphasis(line)));
        }
    }
    out
}

/// Turns `**bold**`, `*italic*` and `` `code` `` into terminal escapes.
///
/// Only ever called on prose lines — indented and fenced blocks bypass it — so
/// the asterisks in formulas like `h*h*42317861` are never seen here.
fn emphasis(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    loop {
        let bold = rest.find("**");
        let code = rest.find('`');
        let ital = lone_star(rest);

        let next = [(bold, Marker::Bold), (code, Marker::Code), (ital, Marker::Italic)]
            .into_iter()
            .filter_map(|(at, kind)| at.map(|i| (i, kind)))
            .min_by_key(|(i, _)| *i);

        let Some((at, kind)) = next else {
            out.push_str(rest);
            return out;
        };

        // Attribute-specific "off" codes rather than a blanket reset, so a
        // code span inside bold does not cancel the bold when it ends.
        let (skip, close, on, off) = match kind {
            Marker::Bold => (2, "**", "\x1b[1m", "\x1b[22m"),
            Marker::Italic => (1, "*", "\x1b[3m", "\x1b[23m"),
            Marker::Code => (1, "`", "\x1b[36m", "\x1b[39m"),
        };

        let after = &rest[at + skip..];
        let end = match kind {
            Marker::Italic => lone_star(after),
            _ => after.find(close),
        };

        match end {
            Some(end) => {
                out.push_str(&rest[..at]);
                let inner = &after[..end];
                // Code spans are verbatim; bold and italic can contain more
                // markup, so recurse. The inner slice is strictly shorter, so
                // this terminates.
                let body = match kind {
                    Marker::Code => inner.to_string(),
                    _ => emphasis(inner),
                };
                out.push_str(&format!("{on}{body}{off}"));
                rest = &after[end + skip..];
            }
            // An unmatched marker is literal text, not a formatting bug.
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Marker {
    Bold,
    Italic,
    Code,
}

/// The first `*` that is not part of a `**` pair.
///
/// `*` is ASCII, so the byte index it returns is always a char boundary and is
/// safe to slice on.
fn lone_star(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (0..b.len()).find(|&i| {
        b[i] == b'*' && (i + 1 >= b.len() || b[i + 1] != b'*') && (i == 0 || b[i - 1] != b'*')
    })
}

pub fn run(session: &mut Session) -> Result<()> {
    let _ = session;

    loop {
        ui::header("Mode 14 — Documentation");

        let mut items = vec![
            "Overview — how the modes fit together".to_string(),
            "Glossary — the vocabulary the pages assume".to_string(),
        ];
        for (i, m) in MODES.iter().enumerate() {
            items.push(format!("{:>2}. {} — {}", i + 1, m.name, m.summary));
        }
        items.push("(back to the main menu)".to_string());

        let pick = ui::select("What would you like to read?", &items)?;

        let page = match pick {
            0 => OVERVIEW,
            1 => GLOSSARY,
            n if n - 2 < MODES.len() => MODES[n - 2].doc,
            _ => return Ok(()),
        };

        println!();
        print!("{}", render(page));
        println!();
        ui::pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standalone_pages_are_substantial() {
        assert!(OVERVIEW.trim().starts_with("# "));
        assert!(GLOSSARY.trim().starts_with("# "));
        assert!(OVERVIEW.len() > 1000, "the overview should actually orient someone");
        assert!(GLOSSARY.len() > 1000);
    }

    #[test]
    fn the_glossary_defines_the_terms_the_pages_lean_on() {
        // These are the words the mode pages use without explaining.
        for term in [
            "Structure seed",
            "World seed",
            "LCG",
            "Salt",
            "Bit-lifting",
            "Pillar seed",
            "cubiomes",
        ] {
            assert!(
                GLOSSARY.contains(term),
                "the glossary never defines {term:?}"
            );
        }
    }

    #[test]
    fn rendering_strips_markdown_and_survives_odd_input() {
        let r = render("# Title\n\nSome **bold** and `code` here.\n* a bullet\n");
        assert!(r.contains("Title"));
        assert!(!r.contains("**"), "bold markers should be consumed: {r:?}");
        assert!(!r.contains("# "), "heading markers should be consumed");
        assert!(r.contains("• a bullet"));

        // Unmatched markers are text, not a panic or a swallowed line.
        assert!(render("an ** unmatched marker").contains("unmatched marker"));
        assert!(render("a ` lone backtick").contains("lone backtick"));
        assert_eq!(render(""), "");
    }

    #[test]
    fn italics_render_and_formula_asterisks_are_left_alone() {
        let r = render("this *word* is emphasised");
        assert!(!r.contains('*'), "italic markers should be consumed: {r:?}");
        assert!(r.contains("word"));

        // Bold still wins over italic where both could match.
        let r = render("**strong** and *soft*");
        assert!(!r.contains('*'), "{r:?}");
        assert!(r.contains("strong") && r.contains("soft"));

        // Indented formula lines bypass emphasis entirely, so their asterisks
        // must survive verbatim.
        let r = render("    h = h*h*42317861 + h*11");
        assert!(r.contains("h*h*42317861"), "formula was mangled: {r:?}");

        // An odd number of markers is text, not a panic.
        assert!(render("a * lone star").contains("lone star"));
        assert_eq!(lone_star("**bold**"), None);
        assert_eq!(lone_star("a*b"), Some(1));
    }

    /// Removes the spans where literal punctuation is legitimate: code blocks
    /// (dim) and inline code (cyan). An asterisk inside `cz*cz` is content,
    /// not an unclosed marker.
    fn strip_verbatim(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while let Some(at) = rest.find("\x1b[36m") {
            out.push_str(&rest[..at]);
            match rest[at..].find("\x1b[39m") {
                Some(end) => rest = &rest[at + end..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn no_page_leaves_unrendered_markdown_on_screen() {
        // Catches an unpaired marker anywhere in the hand-written pages —
        // notably bold spanning a line break, which a line-by-line renderer
        // cannot close and which this found in mode 3's page.
        for (name, page) in MODES
            .iter()
            .map(|m| (m.name, m.doc))
            .chain([("overview", OVERVIEW), ("glossary", GLOSSARY)])
        {
            for line in render(page).lines() {
                if line.contains("\x1b[2m") {
                    continue; // verbatim code block
                }
                let visible = strip_verbatim(line);
                assert!(
                    !visible.contains('*'),
                    "{name}: unrendered markdown in {line:?}"
                );
                assert!(
                    !visible.contains('`'),
                    "{name}: unrendered backtick in {line:?}"
                );
            }
        }
    }

    #[test]
    fn code_fences_are_not_printed_as_backticks() {
        let r = render("before\n```\nlet x = 1;\n```\nafter");
        assert!(!r.contains("```"));
        assert!(r.contains("let x = 1;"));
        assert!(r.contains("before") && r.contains("after"));
    }

    #[test]
    fn every_page_renders_without_panicking() {
        // The pages are hand-written markdown; rendering must never be the
        // thing that breaks.
        for m in MODES {
            let r = render(m.doc);
            assert!(!r.is_empty());
            assert!(!r.contains("```"));
        }
        assert!(!render(OVERVIEW).is_empty());
        assert!(!render(GLOSSARY).is_empty());
    }
}
