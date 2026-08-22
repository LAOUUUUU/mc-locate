//! Mode 7 — pulling block coordinates out of chat logs, console output and
//! pasted text.
//!
//! Players leak their position constantly without meaning to: `/tp` commands,
//! "meet me at 1200 64 -340", waypoint bots echoing `x: … y: … z: …`, mod
//! chat spam. This module turns that noise into a list of candidate positions
//! that the search modes can consume.
//!
//! The regex pipeline here is deliberately separate from the UI: it is
//! compiled exactly once (see [`patterns`]) and is also driven line-by-line by
//! [`crate::log_watcher`] while tailing a live `latest.log`, which would
//! otherwise recompile the same dozen patterns hundreds of times a second.
//!
//! Note what a log *cannot* give you: `latest.log` only ever contains
//! chat-visible and console text, never the F3 debug overlay. See the module
//! docs of [`crate::log_watcher`].

use anyhow::{Context, Result, bail};
use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::session::{BBox, Session};
use crate::ui;

/// One coordinate reading recovered from a line of text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordHit {
    pub x: f64,
    /// `None` for the two-value `x: … z: …` form, which is common in chat
    /// because Y is the least interesting axis to share.
    pub y: Option<f64>,
    pub z: f64,
    /// Which pattern produced this hit: `"xyz"`, `"xz"`, `"tp"`, `"bracket"`
    /// or `"triple"`. Carried through to the CSV so a hit from a bare
    /// `N N N` triple (the noisiest form) can be told apart from an explicit
    /// `/tp`.
    pub kind: &'static str,
}

impl std::fmt::Display for CoordHit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.y {
            Some(y) => write!(f, "x {} y {} z {}", fmt_num(self.x), fmt_num(y), fmt_num(self.z)),
            None => write!(f, "x {} y ? z {}", fmt_num(self.x), fmt_num(self.z)),
        }
    }
}

/// Prints whole numbers without a trailing `.0`, so `100` does not become
/// `100.0` in the middle of a coordinate the user is about to retype.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Half-width of the vanilla world border, in blocks. Matches
/// [`crate::worldgen::Version::world_border`]; duplicated as a plain constant
/// only so that validation does not need a `Version` in hand.
pub const WORLD_BORDER: f64 = 29_999_984.0;

/// Plausible Y range for a coordinate: the 1.18+ Overworld build range
/// (-64..=320 including the 1.17-and-earlier 0..=255 range as a subset).
///
/// The bound is generous on purpose — it is a *sanity* filter, not a claim
/// about the dimension the reading came from (the Nether tops out at 127).
pub const MIN_Y: f64 = -64.0;
/// See [`MIN_Y`].
pub const MAX_Y: f64 = 320.0;

/// A number: optionally negative, optionally fractional. Minecraft prints
/// player positions with decimals, and any coordinate can be negative.
const NUM: &str = r"-?\d+(?:\.\d+)?";

/// A consumed left-hand guard, standing in for a look-behind (the `regex`
/// crate has no look-around at all).
///
/// It matches either the start of the line or one character that cannot be
/// part of a number or an identifier, which is what stops `Max: 100` from
/// reading as an X coordinate and `1.20.1` from being torn into pieces.
/// `+` and `-` are excluded so that `5-100` is not read as the number `100`.
const GUARD: &str = r"(?:^|[^0-9A-Za-z_.+-])";

/// One compiled coordinate pattern, with the meaning of its capture groups.
struct CoordPattern {
    re: Regex,
    kind: &'static str,
    /// True when group 2 is Y and group 3 is Z; false for the `x…z` pair form
    /// where group 2 is already Z.
    has_y: bool,
}

/// The compiled pattern set, in priority order.
///
/// Order matters: an earlier pattern claims the character range it matched,
/// so `[100, 64, -200]` is reported once as a bracketed hit rather than twice
/// (once bracketed, once as the bare triple sitting inside it). The most
/// specific and most trustworthy forms therefore come first.
fn patterns() -> &'static [CoordPattern] {
    static PATTERNS: OnceLock<Vec<CoordPattern>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            let build = |pattern: String, kind: &'static str, has_y: bool| CoordPattern {
                // These are compile-time-constant patterns, so a failure here
                // is a bug in this file rather than anything the user did.
                re: Regex::new(&pattern).expect("built-in coordinate regex is valid"),
                kind,
                has_y,
            };

            vec![
                // /tp 100 64 -200, /tp @s 100 64 -200, /teleport @p[...] ...
                // The leading slash is required: bare "tp" is far too common a
                // word to spend three numbers on.
                build(
                    format!(
                        r"(?i){GUARD}/(?:tp|teleport)(?:\s+@[a-z](?:\[[^\]]*\])?)?\s+({NUM})\s+({NUM})\s+({NUM})"
                    ),
                    "tp",
                    true,
                ),
                // x: 100 y: 64 z: -200 / X=100, Y=64, Z=-200 / x 100 y 64 z -200
                build(
                    format!(
                        r"(?i){GUARD}x\s*[:=]?[\s,;]*({NUM})[\s,;]*y\s*[:=]?[\s,;]*({NUM})[\s,;]*z\s*[:=]?[\s,;]*({NUM})"
                    ),
                    "xyz",
                    true,
                ),
                // [100, 64, -200] — how most waypoint and minimap mods print.
                build(
                    format!(r"{GUARD}\[\s*({NUM})\s*,\s*({NUM})\s*,\s*({NUM})\s*\]"),
                    "bracket",
                    true,
                ),
                // x: 100 z: -200 — no Y, which players omit constantly.
                // Must be tried after the xyz form, or it would swallow it.
                build(
                    format!(r"(?i){GUARD}x\s*[:=]?[\s,;]*({NUM})[\s,;]*z\s*[:=]?[\s,;]*({NUM})"),
                    "xz",
                    false,
                ),
                // Bare 100, 64, -200 or 100 64 -200. The loosest form, and the
                // one the Y-range check below earns its keep on.
                build(
                    format!(r"{GUARD}({NUM})(?:\s*,\s*|\s+)({NUM})(?:\s*,\s*|\s+)({NUM})"),
                    "triple",
                    true,
                ),
            ]
        })
        .as_slice()
}

/// Scans one line of text for every coordinate it can defend.
///
/// Hits are returned in the order they appear in the line, and no two hits
/// overlap: whichever pattern claims a stretch of characters first keeps it.
pub fn scan_line(line: &str) -> Vec<CoordHit> {
    let mut found: Vec<(usize, CoordHit)> = Vec::new();
    // Character ranges already accounted for by a higher-priority pattern.
    let mut claimed: Vec<(usize, usize)> = Vec::new();

    for pattern in patterns() {
        for caps in pattern.re.captures_iter(line) {
            let (Some(first), Some(last)) = (caps.get(1), caps.get(caps.len() - 1)) else {
                continue;
            };
            let (start, end) = (first.start(), last.end());

            if claimed.iter().any(|(s, e)| start < *e && *s < end) {
                continue;
            }
            if !right_boundary_ok(line, end) {
                continue;
            }

            let nums: Option<Vec<f64>> = (1..caps.len())
                .map(|i| caps.get(i).and_then(|m| m.as_str().parse::<f64>().ok()))
                .collect();
            let Some(nums) = nums else { continue };

            let hit = if pattern.has_y {
                CoordHit {
                    x: nums[0],
                    y: Some(nums[1]),
                    z: nums[2],
                    kind: pattern.kind,
                }
            } else {
                CoordHit {
                    x: nums[0],
                    y: None,
                    z: nums[1],
                    kind: pattern.kind,
                }
            };

            if !is_plausible(&hit) {
                // Deliberately *not* claimed: a rejected span may still
                // contain a real coordinate under a looser pattern.
                continue;
            }

            claimed.push((start, end));
            found.push((start, hit));
        }
    }

    found.sort_by_key(|(start, _)| *start);
    found.into_iter().map(|(_, hit)| hit).collect()
}

/// The right-hand half of the boundary check that [`GUARD`] does on the left.
///
/// Done in Rust rather than in the pattern because the `regex` crate has no
/// look-ahead, and *consuming* the trailing character would make a coordinate
/// at the end of a sentence ("…meet me at 100 64 200.") unmatchable.
fn right_boundary_ok(line: &str, end: usize) -> bool {
    let mut rest = line[end..].chars();
    match rest.next() {
        None => true,
        // "100 64 200mm" is a measurement, not a position.
        Some(c) if c.is_alphanumeric() || c == '_' => false,
        // A dot followed by a digit means we cut a longer dotted number in
        // half — almost always a version string such as "1.20.1".
        Some('.') => !matches!(rest.next(), Some(d) if d.is_ascii_digit()),
        Some(_) => true,
    }
}

/// Rejects readings that cannot be positions in a Minecraft world.
///
/// Log lines are full of number triples that are not coordinates: timestamps
/// (`[15:04:23]`), version strings (`1.20.1`), memory figures, mod counts,
/// tick durations. Anything outside the world border or outside the build
/// range is far more likely to be one of those than a coordinate, and letting
/// it through would pollute the CSV and the search box handed to modes 2/6.
fn is_plausible(hit: &CoordHit) -> bool {
    if !hit.x.is_finite() || !hit.z.is_finite() {
        return false;
    }
    if hit.x.abs() > WORLD_BORDER || hit.z.abs() > WORLD_BORDER {
        return false;
    }
    match hit.y {
        Some(y) => y.is_finite() && (MIN_Y..=MAX_Y).contains(&y),
        None => true,
    }
}

/// A hit together with where it came from.
#[derive(Debug, Clone)]
pub struct LocatedHit {
    /// 1-based line number within the scanned text.
    pub line_no: usize,
    pub hit: CoordHit,
    /// The whole line the hit came from, so the user can judge it.
    pub context: String,
}

/// Scans a whole block of lines.
pub fn scan_lines<S: AsRef<str>>(lines: &[S]) -> Vec<LocatedHit> {
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        for hit in scan_line(line) {
            out.push(LocatedHit {
                line_no: idx + 1,
                hit,
                context: line.trim().to_string(),
            });
        }
    }
    out
}

/// Tidies a path the user typed or drag-and-dropped: strips the quotes a
/// terminal adds around a dropped path, and expands a leading `~`.
pub fn expand_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    if trimmed == "~"
        && let Some(home) = home_dir()
    {
        return home;
    }
    PathBuf::from(trimmed)
}

/// The user's home directory, without pulling in a crate for it.
///
/// `USERPROFILE` is the Windows equivalent of `HOME`; both are set by the
/// shell for any interactive session this tool is realistically run from.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Reads a log file as text.
///
/// Logs are read as bytes and decoded lossily rather than with
/// [`std::fs::read_to_string`], because a crashed client or a mod printing raw
/// bytes can leave invalid UTF-8 in the middle of an otherwise useful file,
/// and refusing the whole file over one bad byte helps nobody.
pub fn read_log_file(path: &std::path::Path) -> Result<Vec<String>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .map(|l| l.to_string())
        .collect())
}

/// How many hits to print before deferring the rest to the CSV. A modded
/// client's `latest.log` can carry tens of thousands of matches.
const MAX_PRINTED: usize = 200;

/// Mode 7 entry point.
pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 7 — chat / log coordinate scraper");
    ui::note(
        "Recovers coordinates from chat, /tp commands and console output. \
         Logs never contain F3 debug coordinates — use mode 3 for those.",
    );

    let route = ui::select_str(
        "Scan a pasted/file log, or watch live?",
        &["Scan a log file or pasted text", "Watch a live log"],
    )?;
    if route == 1 {
        return crate::log_watcher::watch(session);
    }

    let source = ui::select_str("Where is the text?", &["A log file on disk", "Paste it here"])?;
    let (label, lines) = if source == 0 {
        let raw: String = ui::input("Path to the log file")?;
        let path = expand_path(&raw);
        let lines = read_log_file(&path)?;
        (path.display().to_string(), lines)
    } else {
        let lines = ui::read_block("Paste the log or chat text:")?;
        ("pasted text".to_string(), lines)
    };

    if lines.is_empty() {
        bail!("there was nothing to scan");
    }

    let hits = scan_lines(&lines);
    println!();
    if hits.is_empty() {
        ui::warn(&format!(
            "No coordinates found in {} line(s) of {label}.",
            lines.len()
        ));
        ui::note(
            "Supported forms: 'x: N y: N z: N', 'x: N z: N', bare 'N N N' or 'N, N, N', \
             '[N, N, N]', '/tp N N N' and '/teleport @s N N N'.",
        );
        return Ok(());
    }

    ui::success(&format!(
        "{} coordinate hit(s) in {} line(s) of {label}.",
        hits.len(),
        lines.len()
    ));
    println!();

    for located in hits.iter().take(MAX_PRINTED) {
        println!(
            "  \x1b[1mline {:>6}\x1b[0m  {}  \x1b[2m({})\x1b[0m",
            located.line_no, located.hit, located.hit.kind
        );
        println!("                \x1b[2m{}\x1b[0m", truncate(&located.context, 110));
    }
    if hits.len() > MAX_PRINTED {
        println!();
        ui::note(&format!(
            "…and {} more (write the CSV to see them all).",
            hits.len() - MAX_PRINTED
        ));
    }

    println!();
    if ui::confirm("Write all hits to a CSV?", false)? {
        let path: String = ui::input_default("CSV path", "log-coords.csv".to_string())?;
        let path = expand_path(&path);
        write_csv(&path, &hits)?;
        ui::success(&format!("Wrote {} row(s) to {}", hits.len(), path.display()));
    }

    // The last hit is the most recent position in a chronological log, which
    // is nearly always the one worth searching around.
    if let Some(last) = hits.last() {
        println!();
        offer_search_box(session, last.hit.x, last.hit.z)?;
    }

    Ok(())
}

/// Writes every hit to a CSV, one row per hit.
pub fn write_csv(path: &std::path::Path, hits: &[LocatedHit]) -> Result<()> {
    let mut writer = csv::Writer::from_path(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    writer.write_record(["line", "x", "y", "z", "kind", "context"])?;
    for located in hits {
        writer.write_record([
            located.line_no.to_string(),
            located.hit.x.to_string(),
            located.hit.y.map(|y| y.to_string()).unwrap_or_default(),
            located.hit.z.to_string(),
            located.hit.kind.to_string(),
            located.context.clone(),
        ])?;
    }
    writer.flush().context("could not flush the CSV")?;
    Ok(())
}

/// Offers to park an X/Z in [`Session::search_box`] so modes 2 and 6 can pick
/// it up without the user retyping it.
///
/// Shared with [`crate::log_watcher`], which offers the same thing for the
/// last position seen while tailing.
pub fn offer_search_box(session: &mut Session, x: f64, z: f64) -> Result<()> {
    let (cx, cz) = (x.round() as i32, z.round() as i32);
    if !ui::confirm(
        &format!("Store X {cx}, Z {cz} as the session search box (for modes 2/6)?"),
        true,
    )? {
        return Ok(());
    }
    // 128 blocks is eight chunks either way: wide enough to cover the drift
    // between where someone said a coordinate and where the thing actually
    // is, narrow enough that a structure search over it is instant.
    let radius: i32 = ui::input_default("Radius around it (blocks)", 128)?;
    let bbox = BBox::around(cx, cz, radius.abs());
    session.search_box = Some(bbox);
    ui::success(&format!("Search box set: {bbox}"));
    Ok(())
}

/// Character-safe truncation for echoing a log line back at the user.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(line: &str) -> CoordHit {
        let hits = scan_line(line);
        assert_eq!(hits.len(), 1, "expected exactly one hit in {line:?}, got {hits:?}");
        hits[0]
    }

    #[test]
    fn labelled_xyz_in_every_punctuation_style() {
        for line in [
            "x: 100 y: 64 z: -200",
            "X: 100, Y: 64, Z: -200",
            "x 100 y 64 z -200",
            "X=100,Y=64,Z=-200",
            "[CHAT] <Steve> im at x: 100 y: 64 z: -200 come find me",
        ] {
            let hit = only(line);
            assert_eq!(hit.x, 100.0, "{line}");
            assert_eq!(hit.y, Some(64.0), "{line}");
            assert_eq!(hit.z, -200.0, "{line}");
            assert_eq!(hit.kind, "xyz", "{line}");
        }
    }

    #[test]
    fn decimals_and_negatives_survive() {
        let hit = only("x: -1245.5 y: 71.0 z: 340.25");
        assert_eq!(hit.x, -1245.5);
        assert_eq!(hit.y, Some(71.0));
        assert_eq!(hit.z, 340.25);

        let hit = only("-1245.5 71 340.25");
        assert_eq!(hit.x, -1245.5);
        assert_eq!(hit.z, 340.25);
        assert_eq!(hit.kind, "triple");
    }

    #[test]
    fn bare_triples_in_both_separators() {
        for line in ["100, 64, -200", "100 64 -200", "100,64,-200"] {
            let hit = only(line);
            assert_eq!(hit.x, 100.0, "{line}");
            assert_eq!(hit.y, Some(64.0), "{line}");
            assert_eq!(hit.z, -200.0, "{line}");
            assert_eq!(hit.kind, "triple", "{line}");
        }
    }

    #[test]
    fn teleport_commands() {
        for line in [
            "/tp 100 64 -200",
            "/tp @s 100 64 -200",
            "/teleport 100 64 -200",
            "/teleport @p 100 64 -200",
            "[15:04:23] [Server thread/INFO]: Steve issued server command: /tp 100 64 -200",
        ] {
            let hit = only(line);
            assert_eq!(hit.x, 100.0, "{line}");
            assert_eq!(hit.y, Some(64.0), "{line}");
            assert_eq!(hit.z, -200.0, "{line}");
            assert_eq!(hit.kind, "tp", "{line}");
        }
    }

    #[test]
    fn bracketed_triples_are_reported_once() {
        let hit = only("waypoint [100, 64, -200] added");
        assert_eq!(hit.x, 100.0);
        assert_eq!(hit.y, Some(64.0));
        assert_eq!(hit.z, -200.0);
        assert_eq!(hit.kind, "bracket");
    }

    #[test]
    fn two_value_xz_form_has_no_y() {
        let hit = only("x: 1200 z: -340");
        assert_eq!(hit.x, 1200.0);
        assert_eq!(hit.y, None);
        assert_eq!(hit.z, -340.0);
        assert_eq!(hit.kind, "xz");

        // The three-value form must win where both could match.
        assert_eq!(only("x: 1200 y: 70 z: -340").kind, "xyz");
    }

    #[test]
    fn coordinates_beyond_the_world_border_are_rejected() {
        // 29_999_984 is the border; one block past it cannot be a position.
        assert!(scan_line("x: 29999985 y: 64 z: 0").is_empty());
        assert!(scan_line("x: 0 y: 64 z: -29999985").is_empty());
        // …and one block inside it is fine.
        assert_eq!(only("x: 29999984 y: 64 z: 0").x, 29_999_984.0);
    }

    #[test]
    fn impossible_y_values_are_rejected() {
        assert!(scan_line("x: 0 y: 321 z: 0").is_empty());
        assert!(scan_line("x: 0 y: -65 z: 0").is_empty());
        assert_eq!(only("x: 0 y: 320 z: 0").y, Some(320.0));
        assert_eq!(only("x: 0 y: -64 z: 0").y, Some(-64.0));
    }

    #[test]
    fn ordinary_log_noise_is_not_mistaken_for_coordinates() {
        for line in [
            "[15:04:23] [main/INFO]: Loading Minecraft 1.20.1 with Fabric",
            "[15:04:23] [Render thread/INFO]: OpenAL initialized",
            "Setting user: Steve",
            // Timestamps use colons, which no coordinate pattern accepts.
            "[12:34:56] [Worker-Main-7/INFO]: Prepared 441 spawn chunks",
            // A version triple: the Y slot alone would rule it out, but the
            // dotted-number guard rejects it before that.
            "Mod loader version 1.20.1 detected",
        ] {
            assert!(scan_line(line).is_empty(), "false positive in {line:?}: {:?}", scan_line(line));
        }
    }

    #[test]
    fn a_coordinate_at_the_end_of_a_sentence_still_matches() {
        // The trailing '.' must not be mistaken for part of a dotted number.
        let hit = only("meet me at 100 64 -200.");
        assert_eq!(hit.z, -200.0);
    }

    #[test]
    fn identifier_prefixes_do_not_produce_an_x() {
        // "max: 100" is not an X coordinate; without the left guard it would
        // pair up with a later z.
        assert!(scan_line("max: 100 z: 200").is_empty());
    }

    #[test]
    fn several_hits_in_one_line_come_back_in_order() {
        let hits = scan_line("from x: 10 y: 64 z: 20 to /tp 30 70 40");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].kind, "xyz");
        assert_eq!(hits[0].x, 10.0);
        assert_eq!(hits[1].kind, "tp");
        assert_eq!(hits[1].x, 30.0);
    }

    #[test]
    fn scan_lines_reports_line_numbers_and_context() {
        let lines = vec![
            "nothing here".to_string(),
            "  x: 5 y: 64 z: 6  ".to_string(),
        ];
        let hits = scan_lines(&lines);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line_no, 2);
        assert_eq!(hits[0].context, "x: 5 y: 64 z: 6");
    }

    #[test]
    fn paths_are_tidied_before_use() {
        assert_eq!(expand_path("  \"/tmp/latest.log\" "), PathBuf::from("/tmp/latest.log"));
        if let Some(home) = home_dir() {
            assert_eq!(expand_path("~/logs/latest.log"), home.join("logs/latest.log"));
        }
    }

    #[test]
    fn the_pattern_set_compiles_once_and_is_reused() {
        let first = patterns().as_ptr();
        let second = patterns().as_ptr();
        assert_eq!(first, second, "patterns() must not recompile per call");
    }
}
