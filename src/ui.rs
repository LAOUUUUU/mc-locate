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
use crate::worldgen::Version;

pub fn theme() -> ColorfulTheme {
    ColorfulTheme::default()
}

/// A titled section header, printed at the top of each mode.
pub fn header(title: &str) {
    println!();
    println!("\x1b[1;36m{title}\x1b[0m");
    println!("{}", "─".repeat(title.chars().count().max(8)));
}

pub fn note(msg: &str) {
    println!("  \x1b[2m{msg}\x1b[0m");
}

pub fn warn(msg: &str) {
    println!("  \x1b[1;33m! {msg}\x1b[0m");
}

pub fn success(msg: &str) {
    println!("  \x1b[1;32m✓ {msg}\x1b[0m");
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

pub fn prompt_version(session: &mut Session) -> Result<Version> {
    if let Some(v) = session.version
        && confirm(&format!("Use the session version ({})?", v.label()), true)?
    {
        return Ok(v);
    }
    let labels: Vec<String> = Version::ALL.iter().map(|v| v.label().to_string()).collect();
    let idx = select("Minecraft version", &labels)?;
    let v = Version::ALL[idx];
    session.version = Some(v);
    Ok(v)
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
        ProgressStyle::with_template(
            "  {msg} [{bar:34.cyan/blue}] {human_pos}/{human_len} ({percent}%) {per_sec} eta {eta}",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-"),
    );
    pb.set_message(label.to_string());
    pb
}

/// An indeterminate spinner, for work whose size is not known up front.
pub fn spinner(label: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner:.cyan} {msg} ({elapsed})")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
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
    fn bbox_geometry() {
        let b = BBox::around(0, 0, 100);
        assert_eq!(b.min_x, -100);
        assert_eq!(b.max_x, 100);
        assert_eq!(b.width(), 201);
        assert_eq!(b.area(), 201 * 201);
    }
}
