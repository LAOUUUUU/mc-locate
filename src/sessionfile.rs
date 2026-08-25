//! Mode 13 — saving, loading and importing observations, and watching a
//! screenshots folder.
//!
//! Two jobs that turn out to be the same job.
//!
//! **Persistence.** A [`Session`] used to live only in memory, so quitting
//! discarded every coordinate typed into it. Forty bedrock blocks is real
//! effort; it should survive closing the program.
//!
//! **An intake for other producers.** The same file format
//! ([`crate::observations`]) is the contract anything else writes to: a Fabric
//! mod dumping bedrock as you fly the Nether, a script, a hand-written file.
//! mc-locate never has to know who wrote it.
//!
//! # Watching a screenshots folder
//!
//! Minecraft cannot be made to press F2 on our behalf, and driving its window
//! from outside would be fragile and intrusive. What works instead is the
//! other half of that loop: watch the screenshots folder, and the moment the
//! game writes a new PNG, pick it up and read the F3 overlay out of it. The
//! advisor says what to go and look at, you press F2, and the observation
//! arrives without any typing.
//!
//! Only new files are considered — the folder is inventoried on entry so an
//! existing pile of screenshots is not re-read every session.

use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::observations::ObservationFile;
use crate::session::Session;
use crate::{logscrape, ocr, ui};

/// Default file name, so the common case needs no typing.
pub const DEFAULT_FILE: &str = "mc-locate-session.json";

/// Screenshot folders for the launchers we already know about.
///
/// Deliberately mirrors [`crate::log_watcher::candidate_log_paths`]: a user who
/// found their log there will find their screenshots alongside it.
pub fn candidate_screenshot_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::log_watcher::candidate_game_dirs()
        .into_iter()
        .map(|d| d.join("screenshots"))
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Folders the exporter mod writes to, one per known launcher game dir.
///
/// Mirrors [`candidate_screenshot_dirs`]: the mod writes to `<gameDir>/mc-locate`,
/// alongside the `screenshots` folder the OCR watcher already knows about.
pub fn candidate_observation_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = crate::log_watcher::candidate_game_dirs()
        .into_iter()
        .map(|d| d.join("mc-locate"))
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// JSON files in `dir`, each mapped to its last-modified time.
///
/// The modification time matters because the mod keeps a single rolling file
/// (`session-current.json`) that grows as you play; re-reading it when it
/// changes is exactly what makes the watch live rather than one-shot.
fn json_files(dir: &Path) -> HashMap<PathBuf, SystemTime> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json"))
                })
                .map(|p| {
                    let mtime = std::fs::metadata(&p)
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    (p, mtime)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Files in `dir` that look like images.
fn image_files(dir: &Path) -> HashSet<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && ocr::has_image_extension(p))
                .collect()
        })
        .unwrap_or_default()
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 13 — Session & Observations");

    let choice = ui::select_str(
        "What would you like?",
        &[
            "Save this session to a file",
            "Load / import observations from a file",
            "Watch the exporter mod's folder and auto-import (live)",
            "Watch a screenshots folder and read new ones",
            "Show what this session currently holds",
        ],
    )?;

    match choice {
        0 => save(session),
        1 => load(session),
        2 => watch_observations(session),
        3 => watch_screenshots(session),
        _ => show(session),
    }
}

fn save(session: &mut Session) -> Result<()> {
    let file = ObservationFile::from_session(session, Some("mc-locate".to_string()));
    if file.is_empty() {
        ui::warn("This session is empty — there is nothing worth saving yet.");
        return Ok(());
    }

    let path: String = ui::input_default("Save to", DEFAULT_FILE.to_string())?;
    if Path::new(&path).exists()
        && !ui::confirm(&format!("{path} already exists. Overwrite?"), false)?
    {
        return Ok(());
    }

    file.save(&path)?;
    ui::success(&format!(
        "Wrote {} observation(s) to {path}{}.",
        file.count(),
        if file.count() == 0 {
            " (session state only — no observations yet)"
        } else {
            ""
        }
    ));
    ui::note("Plain JSON — readable, diffable, and the format any mod or script should write.");
    Ok(())
}

fn load(session: &mut Session) -> Result<()> {
    let path: String = ui::input_default("Load from", DEFAULT_FILE.to_string())?;
    let file = ObservationFile::load(&path)?;

    ui::note(&format!(
        "{} carries {} observation(s){}.",
        path,
        file.count(),
        file.source
            .as_deref()
            .map(|s| format!(", written by {s}"))
            .unwrap_or_default()
    ));

    // Merging is the safe default: a mod dump should add to what you have, not
    // replace a seed you already cracked.
    let overwrite = if session.seed.is_some() || session.version.is_some() {
        ui::confirm(
            "Let the file overwrite the seed/version already in this session?",
            false,
        )?
    } else {
        false
    };

    let summary = file.apply_to_session(session, overwrite)?;
    ui::success(&format!("Imported: {summary}."));
    for w in &summary.warnings {
        ui::warn(w);
    }
    Ok(())
}

fn show(session: &Session) -> Result<()> {
    let file = ObservationFile::from_session(session, None);
    println!();
    ui::success(&format!("{} observation(s) held:", file.count()));
    println!("    slime chunks   {}", file.slime.len());
    println!("    bedrock blocks {}", file.bedrock.len());
    println!("    structures     {}", file.structures.len());
    println!(
        "    End pillars    {}",
        match &file.pillar_heights {
            Some(p) => format!("{} measured", p.iter().filter(|h| h.is_some()).count()),
            None => "none".to_string(),
        }
    );
    println!("    candidates     {}", file.candidates.len());
    println!();
    ui::note(&session.summary());
    Ok(())
}

fn watch_observations(session: &mut Session) -> Result<()> {
    ui::header("Watching the exporter mod's folder");
    ui::note(
        "Run /mclocate export (or leave passive collection on) in game, and every \
         file the mod writes here is imported as it appears. The rolling \
         session-current.json is re-read whenever it grows, so this stays in sync \
         as you play.",
    );

    let mut dirs = candidate_observation_dirs();
    let mut labels: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
    labels.push("Type a path".to_string());
    let pick = ui::select("Observation folder", &labels)?;

    let dir = if pick < dirs.len() {
        dirs.remove(pick)
    } else {
        let typed: String = ui::input("Folder path")?;
        shellexpand_home(&typed)
    };
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }

    // Unlike the screenshot watcher, existing files are imported once on entry:
    // a folder that already holds an export is data the user wants, not history
    // to ignore. Their mtimes then seed the change-detection below.
    let mut seen: HashMap<PathBuf, SystemTime> = HashMap::new();
    let initial = json_files(&dir);
    ui::success(&format!(
        "Watching {} ({} existing file(s) will be imported first).",
        dir.display(),
        initial.len()
    ));

    ui::note("Press Enter at any time to stop.");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    let mut imports = 0usize;
    let started = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let now = json_files(&dir);
        let mut fresh: Vec<PathBuf> = now
            .iter()
            .filter(|(path, mtime)| match seen.get(*path) {
                Some(prev) => *mtime > prev,
                None => true,
            })
            .map(|(path, _)| path.clone())
            .collect();
        fresh.sort();
        for path in fresh {
            // The file may still be mid-write when the listing catches it.
            std::thread::sleep(Duration::from_millis(200));
            import_one(session, &path, &mut imports);
        }
        seen = now;
        std::thread::sleep(Duration::from_millis(500));
    }

    println!();
    ui::note(&format!(
        "Stopped after {:.0}s; {imports} import(s).",
        started.elapsed().as_secs_f64()
    ));
    ui::note(&session.summary());
    Ok(())
}

/// Imports one observation file into the session, reporting the outcome.
fn import_one(session: &mut Session, path: &Path, imports: &mut usize) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match ObservationFile::load(path) {
        Ok(file) => match file.apply_to_session(session, false) {
            Ok(summary) => {
                *imports += 1;
                ui::success(&format!("{name}: {summary}"));
            }
            Err(e) => ui::warn(&format!("{name}: could not apply — {e}")),
        },
        // A partial write parses as invalid JSON; it will succeed on the next
        // poll once the writer has finished, so this is a note, not a failure.
        Err(e) => ui::note(&format!("{name}: not readable yet ({e})")),
    }
}

fn watch_screenshots(session: &mut Session) -> Result<()> {
    ui::header("Watching for new screenshots");
    ui::note(
        "Press F2 in game and the shot is read automatically. Nothing already in the folder is \
         touched — only files that appear from now on.",
    );

    let mut dirs = candidate_screenshot_dirs();
    let mut labels: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
    labels.push("Type a path".to_string());
    let pick = ui::select("Screenshots folder", &labels)?;

    let dir = if pick < dirs.len() {
        dirs.remove(pick)
    } else {
        let typed: String = ui::input("Folder path")?;
        shellexpand_home(&typed)
    };
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }

    let mut seen = image_files(&dir);
    ui::success(&format!(
        "Watching {} ({} existing file(s) ignored).",
        dir.display(),
        seen.len()
    ));

    if !cfg!(feature = "ocr") {
        ui::warn(
            "This build has no OCR support, so new screenshots will be reported but not read. \
             Rebuild with `--features ocr` to have coordinates extracted automatically.",
        );
    }

    ui::note("Press Enter at any time to stop.");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        });
    }

    let mut found = 0usize;
    let started = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(400));
        let now = image_files(&dir);
        let mut fresh: Vec<PathBuf> = now.difference(&seen).cloned().collect();
        fresh.sort();
        for path in fresh {
            // A screenshot may still be being written when the directory
            // listing catches it; give the writer a moment before reading.
            std::thread::sleep(Duration::from_millis(250));
            found += 1;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            ui::success(&format!("new screenshot: {name}"));
            report_screenshot(session, &path);
        }
        seen = now;
    }

    println!();
    ui::note(&format!(
        "Stopped after {:.0}s; {found} new screenshot(s) seen.",
        started.elapsed().as_secs_f64()
    ));
    Ok(())
}

/// Reads a screenshot, when this build can.
#[cfg(feature = "ocr")]
fn report_screenshot(session: &mut Session, path: &Path) {
    match ocr::read_screenshot_default(path) {
        Ok(Some(c)) => {
            ui::success(&format!(
                "    XYZ {:.1}, {:.1}, {:.1}  (from the {} line)",
                c.x, c.y, c.z, c.source
            ));
            session.search_box = Some(crate::session::BBox::around(c.x as i32, c.z as i32, 128));
            ui::note("    stored as the session search box");
        }
        Ok(None) => ui::note("    no coordinates found in that shot"),
        Err(e) => ui::warn(&format!("    could not read it: {e}")),
    }
}

#[cfg(not(feature = "ocr"))]
fn report_screenshot(session: &mut Session, path: &Path) {
    let _ = (session, path);
    ui::note("    (built without OCR — rebuild with `--features ocr` to read it)");
}

/// Expands a leading `~` so typed paths behave the way a shell would.
///
/// Falls back to the literal path when `HOME` is unset, which is better than
/// failing: the user can always type an absolute path instead.
fn shellexpand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), logscrape::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_dirs_are_all_real_directories() {
        // May legitimately be empty on a machine with no Minecraft install;
        // what must never happen is returning a path that does not exist.
        for d in candidate_screenshot_dirs() {
            assert!(d.is_dir(), "{} is not a directory", d.display());
        }
    }

    #[test]
    fn observation_dirs_are_all_real_directories() {
        for d in candidate_observation_dirs() {
            assert!(d.is_dir(), "{} is not a directory", d.display());
        }
    }

    #[test]
    fn json_files_tracks_only_json_and_reports_mtime() {
        use std::time::{Duration, SystemTime};
        // A unique dir per run so parallel test processes never collide.
        let dir = std::env::temp_dir().join(format!(
            "mc-locate-obs-watch-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{}").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();

        let listed = json_files(&dir);
        assert_eq!(listed.len(), 1, "only the .json is tracked");
        let a = dir.join("a.json");
        assert!(listed.contains_key(&a));

        // Set an explicit, definitely-newer mtime rather than relying on the
        // wall clock advancing between two writes — that is coarse on some
        // filesystems and made this test flaky in CI. std's set_modified writes
        // the timestamp directly, so the comparison is deterministic.
        let early = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let later = early + Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(early)
            .unwrap();
        let before = json_files(&dir);

        std::fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let after = json_files(&dir);

        assert!(
            after[&a] > before[&a],
            "a file whose mtime advanced must read as newer"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_images_are_picked_up() {
        let dir = std::env::temp_dir().join("mc-locate-shot-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("b.jpg"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let found = image_files(&dir);
        assert_eq!(found.len(), 2, "got {found:?}");
        assert!(found.iter().all(|p| ocr::has_image_extension(p)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_files_are_detected_as_a_set_difference() {
        // The watcher's whole correctness rests on this: existing files must
        // be ignored, and each new one reported exactly once.
        let dir = std::env::temp_dir().join("mc-locate-shot-diff");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("old.png"), b"x").unwrap();

        let seen = image_files(&dir);
        assert_eq!(seen.len(), 1);

        std::fs::write(dir.join("new.png"), b"x").unwrap();
        let now = image_files(&dir);
        let fresh: Vec<_> = now.difference(&seen).collect();
        assert_eq!(fresh.len(), 1);
        assert!(fresh[0].ends_with("new.png"));

        // And after updating the baseline, nothing is fresh any more.
        assert_eq!(image_files(&dir).difference(&now).count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tilde_paths_expand() {
        // Only assert expansion when there is a HOME to expand to; the
        // fallback is deliberately the literal path.
        if logscrape::home_dir().is_some() {
            let p = shellexpand_home("~/screenshots");
            assert!(p.is_absolute(), "{p:?}");
            assert!(!p.to_string_lossy().starts_with('~'));
        }
        assert_eq!(shellexpand_home("/tmp/x"), PathBuf::from("/tmp/x"));
    }
}
