//! Shared prompt and progress helpers, so every mode looks and behaves the
//! same and so a value already in the [`Session`] is offered as a default
//! rather than asked for again.

use anyhow::{Context, Result, bail};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Select};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::{BufRead, IsTerminal};
use std::str::FromStr;

use crate::session::{BBox, Session};
use crate::theme;
use crate::worldgen::Version;

pub fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

/// A titled section header, printed at the top of each mode.
pub fn header(title: &str) {
    println!();
    println!("{}", theme::boxed_header(title));
}

/// Supporting detail. Wrapped to the terminal, because a wall of text that
/// runs off the right edge is worse than no explanation.
pub fn note(msg: &str) {
    print!("{}", theme::dim().apply_to(theme::wrap(msg, 2)));
}

pub fn warn(msg: &str) {
    let body = theme::wrap(msg, 4);
    print!(
        "{}",
        theme::warn_style().apply_to(format!("  {} {}", theme::marks::WARN, body.trim_start()))
    );
}

pub fn success(msg: &str) {
    let body = theme::wrap(msg, 4);
    print!(
        "{}",
        theme::good().apply_to(format!("  {} {}", theme::marks::GOOD, body.trim_start()))
    );
}

/// A failure, for results rather than errors returned up the stack.
pub fn failure(msg: &str) {
    let body = theme::wrap(msg, 4);
    print!(
        "{}",
        theme::bad().apply_to(format!("  {} {}", theme::marks::BAD, body.trim_start()))
    );
}

/// A recovered answer — the thing the user came for.
pub fn result(label: &str, value: &str) {
    println!(
        "  {} {} {}",
        theme::good().apply_to(theme::marks::ARROW),
        theme::dim().apply_to(format!("{label}:")),
        theme::value().apply_to(value)
    );
}

/// An aligned table of results.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    print!("{}", theme::table(headers, rows));
}

/// A horizontal rule.
pub fn rule() {
    println!("{}", theme::rule());
}

pub fn select(prompt: &str, items: &[String]) -> Result<usize> {
    Select::with_theme(&theme())
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact()
        .context("selection cancelled")
}

pub fn select_str(prompt: &str, items: &[&str]) -> Result<usize> {
    let owned: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    select(prompt, &owned)
}

pub fn input<T>(prompt: &str) -> Result<T>
where
    T: Clone + FromStr + ToString,
    <T as FromStr>::Err: ToString + std::fmt::Debug,
{
    Input::<T>::with_theme(&theme())
        .with_prompt(prompt)
        .interact_text()
        .context("input cancelled")
}

pub fn input_default<T>(prompt: &str, default: T) -> Result<T>
where
    T: Clone + FromStr + ToString,
    <T as FromStr>::Err: ToString + std::fmt::Debug,
{
    Input::<T>::with_theme(&theme())
        .with_prompt(prompt)
        .default(default)
        .interact_text()
        .context("input cancelled")
}

/// Free-text input that is allowed to be empty (returns an empty string).
pub fn input_optional(prompt: &str) -> Result<String> {
    Input::<String>::with_theme(&theme())
        .with_prompt(prompt)
        .allow_empty(true)
        .interact_text()
        .context("input cancelled")
}

pub fn confirm(prompt: &str, default: bool) -> Result<bool> {
    Confirm::with_theme(&theme())
        .with_prompt(prompt)
        .default(default)
        .interact()
        .context("confirmation cancelled")
}

/// Reads a multi-line block from stdin, terminated by a blank line or EOF.
///
/// Used wherever the user pastes something bulky: an ASCII bedrock grid, a
/// chat log, a list of chunk coordinates.
pub fn read_block(prompt: &str) -> Result<Vec<String>> {
    println!("{prompt}");
    note("(finish with an empty line, or Ctrl-D)");
    let stdin = std::io::stdin();
    let mut lines = Vec::new();
    for line in stdin.lock().lines() {
        let line = line.context("could not read stdin")?;
        if line.trim().is_empty() {
            break;
        }
        lines.push(line);
    }
    Ok(lines)
}

pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// The seed to work with, reusing the session's seed if one is already known.
pub fn prompt_seed(session: &mut Session) -> Result<i64> {
    if let Some(seed) = session.seed
        && confirm(&format!("Use the session seed ({seed})?"), true)?
    {
        return Ok(seed);
    }
    let seed = prompt_seed_value("World seed")?;
    session.seed = Some(seed);
    Ok(seed)
}

/// Parses a seed the way Minecraft does: a numeric string is used directly,
/// anything else is hashed with `String.hashCode()` and sign-extended.
pub fn prompt_seed_value(prompt: &str) -> Result<i64> {
    let raw: String = input(prompt)?;
    parse_seed(&raw)
}

/// Minecraft's own seed parsing: numeric text is the seed; any other text is
/// `String.hashCode()` widened to 64 bits.
pub fn parse_seed(raw: &str) -> Result<i64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("a seed is required");
    }
    if let Ok(n) = trimmed.parse::<i64>() {
        return Ok(n);
    }
    Ok(crate::random::java_string_hash(trimmed) as i64)
}

/// A version the user picked, which may be beyond what the generator knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChosenVersion {
    /// The bundled generator can generate this version.
    Generated(Version),
    /// Newer than the generator supports. Structure and biome generation are
    /// unavailable; anything that does not consult cubiomes still works.
    Newer(String),
}

impl ChosenVersion {
    pub fn label(&self) -> String {
        match self {
            ChosenVersion::Generated(v) => v.label().to_string(),
            ChosenVersion::Newer(s) => s.clone(),
        }
    }

    /// Everything past the generator's ceiling is comfortably 1.18+.
    pub fn is_1_18_plus(&self) -> bool {
        match self {
            ChosenVersion::Generated(v) => v.is_1_18_plus(),
            ChosenVersion::Newer(_) => true,
        }
    }

    pub fn generated(&self) -> Option<Version> {
        match self {
            ChosenVersion::Generated(v) => Some(*v),
            ChosenVersion::Newer(_) => None,
        }
    }
}

/// The newest version the bundled generator can produce.
pub fn newest_supported() -> Version {
    Version::ALL[0]
}

/// Asks for a version, allowing one newer than the generator supports.
///
/// For modes that do not consult cubiomes — nether bedrock, slime chunks, End
/// pillars — where all that matters is which side of a formula boundary the
/// version falls on.
pub fn prompt_version_any(session: &mut Session) -> Result<ChosenVersion> {
    if let Some(v) = session.version
        && confirm(&format!("Use the session version ({})?", v.label()), true)?
    {
        return Ok(ChosenVersion::Generated(v));
    }
    if let Some(v) = session.newer_version.clone()
        && confirm(&format!("Use the session version ({v})?"), true)?
    {
        return Ok(ChosenVersion::Newer(v));
    }

    let newest = newest_supported();
    let mut labels: Vec<String> = Version::ALL.iter().map(|v| v.label().to_string()).collect();
    labels.push(format!("Newer than {} (e.g. 26.2)", newest.label()));

    let idx = select("Minecraft version", &labels)?;
    if idx < Version::ALL.len() {
        let v = Version::ALL[idx];
        session.version = Some(v);
        session.newer_version = None;
        return Ok(ChosenVersion::Generated(v));
    }

    let typed: String = input_default("Which version?", "26.2".to_string())?;
    let typed = typed.trim().to_string();
    if typed.is_empty() {
        bail!("a version is required");
    }
    note(&format!(
        "Recorded as {typed}. Modes that generate structures, biomes or strongholds cannot run \
         on it — the bundled worldgen library stops at {}. Slime chunks, nether bedrock, End \
         pillars and portal maths are unaffected and work on any version.",
        newest.label()
    ));
    session.version = None;
    session.newer_version = Some(typed.clone());
    Ok(ChosenVersion::Newer(typed))
}

/// Asks for a version the generator can actually generate.
///
/// Refuses a newer one rather than quietly substituting the closest supported
/// version, which would produce confident, wrong structures and biomes.
pub fn prompt_version(session: &mut Session) -> Result<Version> {
    match prompt_version_any(session)? {
        ChosenVersion::Generated(v) => Ok(v),
        ChosenVersion::Newer(v) => bail!(
            "this mode generates world data, and the bundled generator stops at {} — it cannot \
             generate {v}. Picking a nearby supported version instead would give confident, \
             wrong answers, so it is refused. Modes 1, 4, 11 and the pillar route in 9 do not \
             need the generator and still work.",
            newest_supported().label()
        ),
    }
}

/// Prompts for a bounding box, offering the session's stored one if present.
pub fn prompt_bbox(session: &mut Session, purpose: &str) -> Result<BBox> {
    if let Some(b) = session.search_box
        && confirm(&format!("Use the search box from an earlier mode? ({b})"), true)?
    {
        return Ok(b);
    }

    let idx = select_str(
        &format!("How do you want to describe the {purpose} area?"),
        &["Centre point + radius", "Explicit min/max corners"],
    )?;

    let b = if idx == 0 {
        let x: i32 = input_default("Centre X", 0)?;
        let z: i32 = input_default("Centre Z", 0)?;
        let r: i32 = input_default("Radius (blocks)", 2000)?;
        BBox::around(x, z, r)
    } else {
        let min_x: i32 = input("Min X")?;
        let min_z: i32 = input("Min Z")?;
        let max_x: i32 = input("Max X")?;
        let max_z: i32 = input("Max Z")?;
        if max_x < min_x || max_z < min_z {
            bail!("max corner must be greater than or equal to min corner");
        }
        BBox {
            min_x,
            min_z,
            max_x,
            max_z,
        }
    };

    session.search_box = Some(b);
    Ok(b)
}

/// A determinate progress bar with a rate and ETA.
pub fn progress_bar(total: u64, label: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(theme::bar_template())
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            // Block characters rather than ASCII: they render a smooth bar and
            // suit the subject.
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_message(label.to_string());
    pb
}

/// An indeterminate spinner, for work whose size is not known up front.
pub fn spinner(label: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template(theme::spinner_template())
            .unwrap_or_else(|_| ProgressStyle::default_spinner())
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(label.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Formats a duration estimate in a way that makes a multi-hour brute force
/// obviously multi-hour, rather than hiding it behind a spinner.
pub fn humanize_duration(secs: f64) -> String {
    if !secs.is_finite() {
        return "unknown".to_string();
    }
    if secs < 1.0 {
        return "under a second".to_string();
    }
    if secs < 90.0 {
        return format!("{secs:.0} seconds");
    }
    let mins = secs / 60.0;
    if mins < 90.0 {
        return format!("{mins:.0} minutes");
    }
    let hours = mins / 60.0;
    if hours < 48.0 {
        return format!("{hours:.1} hours");
    }
    format!("{:.1} days", hours / 24.0)
}

/// Parses a whitespace/comma separated coordinate pair or triple.
pub fn parse_coords(s: &str) -> Option<Vec<f64>> {
    let cleaned: String = s
        .chars()
        .map(|c| if c == ',' || c == ';' { ' ' } else { c })
        .collect();
    let parts: Vec<f64> = cleaned
        .split_whitespace()
        .filter_map(|p| p.parse::<f64>().ok())
        .collect();
    if parts.len() >= 2 { Some(parts) } else { None }
}

pub fn pause() {
    if !is_interactive() {
        return;
    }
    let _ = Input::<String>::with_theme(&theme())
        .with_prompt("Press Enter to return to the menu")
        .allow_empty(true)
        .interact_text();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_seeds_pass_through_and_text_seeds_hash() {
        assert_eq!(parse_seed("12345").unwrap(), 12345);
        assert_eq!(parse_seed("-98765").unwrap(), -98765);
        // Minecraft hashes non-numeric seed text with String.hashCode().
        assert_eq!(
            parse_seed("glacier").unwrap(),
            crate::random::java_string_hash("glacier") as i64
        );
        assert!(parse_seed("   ").is_err());
    }

    #[test]
    fn coordinate_parsing_accepts_the_usual_shapes() {
        assert_eq!(parse_coords("100 64 -200").unwrap(), vec![100.0, 64.0, -200.0]);
        assert_eq!(parse_coords("100, -200").unwrap(), vec![100.0, -200.0]);
        assert!(parse_coords("nope").is_none());
    }

    #[test]
    fn durations_read_sensibly() {
        assert_eq!(humanize_duration(0.5), "under a second");
        assert_eq!(humanize_duration(45.0), "45 seconds");
        assert_eq!(humanize_duration(600.0), "10 minutes");
        assert!(humanize_duration(36000.0).ends_with("hours"));
        assert!(humanize_duration(3_600_000.0).ends_with("days"));
    }

    #[test]
    fn a_newer_version_is_usable_but_not_generatable() {
        // The whole point of ChosenVersion: a player on 26.2 can still use the
        // formula-only modes, and is refused — loudly — by the ones that would
        // otherwise generate the wrong world.
        let newer = ChosenVersion::Newer("26.2".to_string());
        assert_eq!(newer.label(), "26.2");
        assert!(newer.is_1_18_plus(), "anything past the ceiling is 1.18+");
        assert_eq!(newer.generated(), None, "it must not map to a generator version");

        let known = ChosenVersion::Generated(Version::V1_16_5);
        assert_eq!(known.generated(), Some(Version::V1_16_5));
        assert!(!known.is_1_18_plus());
        assert_eq!(known.label(), "1.16.5");
    }

    #[test]
    fn the_newest_supported_version_is_the_top_of_the_menu() {
        // prompt_version's refusal message quotes this, so it must not drift.
        assert_eq!(newest_supported(), Version::ALL[0]);
        assert_eq!(newest_supported().label(), "1.21.4");
    }

    #[test]
    fn bbox_geometry() {
        let b = BBox::around(0, 0, 100);
        assert_eq!(b.min_x, -100);
        assert_eq!(b.max_x, 100);
        assert_eq!(b.width(), 201);
        assert_eq!(b.area(), 201 * 201);
    }
}
