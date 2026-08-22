//! Mode 7 (live) — tailing a running Minecraft client's `latest.log`.
//!
//! # What a log can and cannot tell you
//!
//! **`latest.log` contains only chat-visible and console text. It does NOT
//! contain F3 debug-screen coordinates.** The debug overlay is drawn on the
//! client every frame and is never written to disk, so no amount of log
//! scraping will recover the position it shows. To capture raw coordinates
//! from a screenshot or a video frame you still need mode 3 (OCR) or manual
//! entry.
//!
//! What the log *does* carry, and what this module goes after: chat messages
//! (including anything a player types with coordinates in it), `/tp` and
//! `/teleport` commands, the `Seed: [..]` reply to `/seed`, join and leave
//! notices, and death messages.
//!
//! Coordinate extraction itself is not duplicated here — every line is handed
//! to [`crate::logscrape::scan_line`], which owns the compiled pattern set.

use anyhow::{Context, Result, bail};
use regex::Regex;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use crate::logscrape::{self, CoordHit};
use crate::session::Session;
use crate::ui;

/// Printed once when live watching starts, and repeated in the module docs
/// above, because it is the single most common misunderstanding about this
/// mode: people expect a log tail to give them F3 coordinates.
pub const F3_CAVEAT: &str = "latest.log holds only chat-visible and console text. It does NOT \
contain F3 debug-screen coordinates — that overlay is drawn client-side and never written to \
disk. Use mode 3 (OCR) on a screenshot, or type them in, to capture raw F3 coordinates.";

/// How often the tail wakes up when the filesystem watcher has nothing to say.
///
/// Also the whole of the fallback strategy: if `notify` cannot be created the
/// loop still ticks on this timeout, it just never gets an early nudge.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Log path discovery
// ---------------------------------------------------------------------------

/// Every `latest.log` this machine appears to have.
///
/// Only paths that exist as files are returned, so an empty vector is a
/// perfectly normal result on a machine without Minecraft installed.
///
/// The launcher layouts below are best effort: MultiMC forks, CurseForge and
/// ATLauncher all let the user move their instance root, and modpack launchers
/// come and go. A "type a custom path" option in [`watch`] covers the rest.
pub fn candidate_log_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = candidate_game_dirs()
        .into_iter()
        .map(|d| d.join("logs").join("latest.log"))
        .filter(|p| p.is_file())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every Minecraft game directory we can find, across launchers and OSes.
///
/// The launcher-and-platform table lives here once. Logs are one thing that
/// hangs off a game directory and screenshots are another
/// ([`crate::sessionfile::candidate_screenshot_dirs`]), so the knowledge of
/// *where installations live* is deliberately kept separate from what we then
/// want out of them.
pub fn candidate_game_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = logscrape::home_dir();

    if cfg!(target_os = "windows") {
        // %APPDATA% is roaming AppData; the vanilla launcher puts .minecraft
        // there, and the MultiMC family puts its instance roots there.
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            push_game_dir(&mut out, appdata.join(".minecraft"));
            for launcher in ["PrismLauncher", "MultiMC", "ATLauncher", "GDLauncher"] {
                scan_instance_root(&mut out, &appdata.join(launcher).join("instances"));
            }
        }
        if let Some(home) = &home {
            // CurseForge's default instance root lives under the user profile,
            // not AppData.
            scan_instance_root(
                &mut out,
                &home.join("curseforge").join("minecraft").join("Instances"),
            );
            scan_instance_root(
                &mut out,
                &home
                    .join("Documents")
                    .join("curseforge")
                    .join("minecraft")
                    .join("Instances"),
            );
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            let support = home.join("Library").join("Application Support");
            // Note the missing dot: the macOS vanilla launcher uses
            // "minecraft", not ".minecraft".
            push_game_dir(&mut out, support.join("minecraft"));
            push_game_dir(&mut out, home.join(".minecraft"));
            for launcher in ["PrismLauncher", "MultiMC", "ATLauncher"] {
                scan_instance_root(&mut out, &support.join(launcher).join("instances"));
            }
            scan_instance_root(
                &mut out,
                &home
                    .join("Documents")
                    .join("curseforge")
                    .join("minecraft")
                    .join("Instances"),
            );
        }
    } else {
        // Linux and the BSDs.
        if let Some(home) = &home {
            push_game_dir(&mut out, home.join(".minecraft"));
            let share = home.join(".local").join("share");
            for launcher in ["PrismLauncher", "multimc", "MultiMC", "ATLauncher"] {
                scan_instance_root(&mut out, &share.join(launcher).join("instances"));
            }
            // Flatpak sandboxes each app's data under ~/.var/app/<app-id>.
            scan_instance_root(
                &mut out,
                &home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
            );
            scan_instance_root(
                &mut out,
                &home
                    .join("Documents")
                    .join("curseforge")
                    .join("minecraft")
                    .join("Instances"),
            );
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Appends a game directory if it looks like one.
///
/// "Looks like one" means it exists and holds either a `logs` or a
/// `screenshots` folder — enough to rule out unrelated directories without
/// requiring the game to have been run for whichever purpose the caller has
/// in mind.
fn push_game_dir(out: &mut Vec<PathBuf>, dir: PathBuf) {
    if dir.is_dir() && (dir.join("logs").is_dir() || dir.join("screenshots").is_dir()) {
        out.push(dir);
    }
}

/// Walks one launcher's `instances/` directory looking for game directories.
///
/// There is no glob crate available, and none is needed: instance roots are a
/// single flat level of directories. Which subdirectory holds the game varies
/// by launcher — MultiMC and Prism nest it in `.minecraft` (or `minecraft` on
/// instances created by newer Prism versions), while CurseForge and ATLauncher
/// use the instance directory itself — so all three shapes are tried.
fn scan_instance_root(out: &mut Vec<PathBuf>, root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        push_game_dir(out, dir.join(".minecraft"));
        push_game_dir(out, dir.join("minecraft"));
        push_game_dir(out, dir);
    }
}

// ---------------------------------------------------------------------------
// Line parsing
// ---------------------------------------------------------------------------

/// A `latest.log` line broken into its prefix and its message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedLine {
    /// The `HH:mm:ss` stamp the game wrote, if the line had a prefix.
    pub time: Option<String>,
    /// The thread name from the prefix, e.g. `Render thread` or `Server thread`.
    pub thread: Option<String>,
    /// True when the line carried a `[CHAT]` tag.
    pub is_chat: bool,
    /// Everything after the prefix (and after `[CHAT]`, when present).
    pub message: String,
}

/// Splits a log line into prefix and message.
///
/// Two prefix shapes are recognised:
///
/// ```text
/// [12:34:56] [Client thread/INFO]: <Steve> hello        (≈1.7 – 1.16)
/// [12:34:56] [Render thread/INFO]: [CHAT] <Steve> hello (1.17+)
/// ```
///
/// **This format varies by Minecraft version and by launcher** — the chat
/// thread was renamed from `Client thread` to `Render thread` in 1.17, the
/// `[CHAT]` tag only appears on client logs, and log4j configurations shipped
/// by mod loaders and servers can change the layout wholesale. If a future
/// version stops matching, this is the regex to adjust; nothing else depends
/// on the prefix, and coordinate scanning runs on the raw line regardless.
pub fn parse_log_line(line: &str) -> ParsedLine {
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    let re = PREFIX.get_or_init(|| {
        Regex::new(r"^\[(\d{1,2}:\d{2}:\d{2})\]\s*\[([^\]/]+)(?:/([A-Z]+))?\]:?\s*(.*)$")
            .expect("built-in log prefix regex is valid")
    });

    let Some(caps) = re.captures(line) else {
        return ParsedLine {
            message: line.to_string(),
            ..Default::default()
        };
    };

    let rest = caps.get(4).map(|m| m.as_str()).unwrap_or("");
    let (is_chat, message) = match rest.strip_prefix("[CHAT]") {
        Some(chat) => (true, chat.trim_start()),
        // A client log from before the [CHAT] tag existed still routed chat
        // through "Client thread", so treat that as chat too.
        None => (
            caps.get(2).is_some_and(|m| m.as_str() == "Client thread"),
            rest,
        ),
    };

    ParsedLine {
        time: caps.get(1).map(|m| m.as_str().to_string()),
        thread: caps.get(2).map(|m| m.as_str().to_string()),
        is_chat,
        message: message.to_string(),
    }
}

/// Extracts the world seed from a `/seed` reply.
///
/// Vanilla answers `/seed` with `Seed: [12345]`; servers and some mods print
/// the bare `Seed: 12345`, so the brackets are optional. The leading guard
/// stops `Reseed: 1` and similar from matching — the same trick
/// [`crate::logscrape`] uses, because the `regex` crate has no look-behind.
pub fn sniff_seed(line: &str) -> Option<i64> {
    static SEED: OnceLock<Regex> = OnceLock::new();
    let re = SEED.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[^0-9A-Za-z_])seed\s*:\s*\[?\s*(-?\d+)")
            .expect("built-in seed regex is valid")
    });
    re.captures(line)?.get(1)?.as_str().parse::<i64>().ok()
}

/// Classifies a player-event message, or `None` if it is not one.
///
/// The death verbs are a **best-effort** list taken from the vanilla
/// `death.attack.*` translation keys in `en_us.json`. Minecraft ships roughly
/// a hundred of them, gains new ones most releases (`freeze`, `sting`,
/// `sonic_boom`…), and servers routinely replace them outright — so this
/// recognises the common ones and makes no claim to be exhaustive. A missed
/// death message costs nothing: the line is still scanned for coordinates
/// like any other.
pub fn classify_event(message: &str) -> Option<&'static str> {
    static JOIN_LEAVE: OnceLock<Regex> = OnceLock::new();
    static DEATH: OnceLock<Regex> = OnceLock::new();

    let join_leave = JOIN_LEAVE.get_or_init(|| {
        Regex::new(r"(?i)\b(joined the game|left the game)\b")
            .expect("built-in join/leave regex is valid")
    });
    if let Some(caps) = join_leave.captures(message) {
        return match caps.get(1).map(|m| m.as_str().to_ascii_lowercase()) {
            Some(s) if s.starts_with("joined") => Some("join"),
            _ => Some("leave"),
        };
    }

    let death = DEATH.get_or_init(|| {
        Regex::new(
            r"(?i)\b(was slain by|was shot by|was killed by|was blown up by|was fireballed by|\
was pummelled by|was pummeled by|was skewered by|was impaled by|was squashed by|\
was stung to death|was struck by lightning|was pricked to death|\
was poked to death by a sweet berry bush|was roasted in dragon breath|was doomed to fall|\
went up in flames|burned to death|was burnt to a crisp|tried to swim in lava|drowned|\
suffocated in a wall|was squished too much|starved to death|withered away|froze to death|\
hit the ground too hard|fell from a high place|fell out of the world|blew up|\
walked into a cactus|walked into fire|discovered the floor was lava|died)\b",
        )
        .expect("built-in death regex is valid")
    });
    if death.is_match(message) {
        return Some("death");
    }
    None
}

// ---------------------------------------------------------------------------
// Tailing
// ---------------------------------------------------------------------------

/// An open file plus the offset we have consumed up to.
///
/// Only the bytes appended since the previous poll are ever read; the file is
/// never re-read from the start unless it visibly rotated.
struct Tail {
    path: PathBuf,
    file: File,
    offset: u64,
    /// Bytes of a line that arrived without its terminating newline yet.
    pending: Vec<u8>,
}

impl Tail {
    /// Opens the file and skips straight to its end, so the user sees new
    /// activity rather than a wall of history.
    fn open_at_end(path: &Path) -> Result<Tail> {
        let mut file =
            File::open(path).with_context(|| format!("could not open {}", path.display()))?;
        let offset = file
            .seek(SeekFrom::End(0))
            .with_context(|| format!("could not seek to the end of {}", path.display()))?;
        Ok(Tail {
            path: path.to_path_buf(),
            file,
            offset,
            pending: Vec::new(),
        })
    }

    /// Returns whatever complete lines have been appended, plus whether the
    /// file rotated.
    ///
    /// Rotation detection is deliberately simple: Minecraft renames
    /// `latest.log` and starts a new one at session start (and some launchers
    /// truncate it), so a length *below* our offset means the thing under this
    /// path is no longer the file we were reading. Re-opening by path picks up
    /// the replacement.
    fn poll(&mut self) -> Result<(Vec<String>, bool)> {
        // A file that briefly vanishes mid-rotation is expected; try again on
        // the next tick rather than tearing the whole mode down.
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return Ok((Vec::new(), false));
        };
        let len = meta.len();

        let mut rotated = false;
        if len < self.offset {
            rotated = true;
            self.file = File::open(&self.path)
                .with_context(|| format!("could not reopen {}", self.path.display()))?;
            self.offset = 0;
            self.pending.clear();
        }
        if len == self.offset {
            return Ok((Vec::new(), rotated));
        }

        self.file.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = Vec::new();
        let read = (&mut self.file)
            .take(len - self.offset)
            .read_to_end(&mut chunk)
            .with_context(|| format!("could not read from {}", self.path.display()))?;
        self.offset += read as u64;
        self.pending.extend_from_slice(&chunk);

        // Split on newlines and hold back any trailing partial line: the game
        // can flush half a line and finish it on the next write.
        let mut lines = Vec::new();
        while let Some(idx) = self.pending.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=idx).collect();
            // Lossy, because a crash or a misbehaving mod can leave invalid
            // UTF-8 in a file we otherwise want to keep reading.
            lines.push(
                String::from_utf8_lossy(&raw)
                    .trim_end_matches(['\n', '\r'])
                    .to_string(),
            );
        }
        Ok((lines, rotated))
    }
}

/// Running totals and findings for one watch session.
#[derive(Debug, Default)]
struct Live {
    lines: u64,
    coords: u64,
    chats: u64,
    events: u64,
    last_coord: Option<CoordHit>,
    /// Seeds seen in `/seed` replies, most recent last.
    seeds: Vec<i64>,
}

impl Live {
    fn handle(&mut self, line: &str) {
        self.lines += 1;
        let parsed = parse_log_line(line);
        // The game's own stamp is preferred over the wall clock: it is the
        // time the event actually happened, not the time we noticed it.
        let stamp = parsed
            .time
            .clone()
            .unwrap_or_else(|| chrono::Local::now().format("%H:%M:%S").to_string());

        // Coordinates are looked for in the whole line, not just the chat
        // message: /tp echoes arrive on the server thread with no [CHAT] tag.
        for hit in logscrape::scan_line(line) {
            self.coords += 1;
            self.last_coord = Some(hit);
            println!(
                "  \x1b[2m[{stamp}]\x1b[0m \x1b[1;32mcoords\x1b[0m {hit}  \x1b[2m({})\x1b[0m",
                hit.kind
            );
            println!("           \x1b[2m{}\x1b[0m", truncate(line.trim(), 110));
        }

        if let Some(seed) = sniff_seed(line) {
            // A /seed reply repeated verbatim is not news.
            if self.seeds.last() != Some(&seed) {
                self.seeds.push(seed);
                println!("  \x1b[2m[{stamp}]\x1b[0m \x1b[1;36mseed\x1b[0m   {seed}");
            }
        } else if let Some(kind) = classify_event(&parsed.message) {
            self.events += 1;
            println!(
                "  \x1b[2m[{stamp}]\x1b[0m \x1b[1;35m{kind:<6}\x1b[0m {}",
                truncate(parsed.message.trim(), 110)
            );
        } else if parsed.is_chat && !parsed.message.trim().is_empty() {
            self.chats += 1;
            println!(
                "  \x1b[2m[{stamp}]\x1b[0m \x1b[36mchat\x1b[0m   {}",
                truncate(parsed.message.trim(), 110)
            );
        }
    }
}

/// Character-safe truncation, so a 4 kB mod log line does not shred the
/// terminal.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Asks which log to follow and tails it until the user stops.
pub fn watch(session: &mut Session) -> Result<()> {
    ui::header("Mode 7 (live) — watching a Minecraft log");
    ui::warn(F3_CAVEAT);

    let path = choose_log_path()?;
    if !path.is_file() {
        bail!("{} is not a readable file", path.display());
    }

    let mut tail = Tail::open_at_end(&path)?;
    println!();
    ui::success(&format!("Following {}", path.display()));
    ui::note("Press Enter to stop (Ctrl-C also works).");
    println!();

    // Stopping cleanly: a detached thread blocks on one line of stdin and
    // flips the flag. Reading stdin from a thread rather than poking at the
    // terminal keeps this portable and dependency-free, and an EOF (a piped,
    // non-interactive stdin) ends the watch instead of hanging forever.
    //
    // Ctrl-C is left to the default SIGINT handler on purpose: nothing here
    // holds unflushed state, so killing the process loses nothing.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut discard = String::new();
            let _ = std::io::stdin().read_line(&mut discard);
            stop.store(true, Ordering::Relaxed);
        });
    }

    // `tx` is kept alive in this scope on purpose: if the watcher fails to
    // build, an already-disconnected channel would turn recv_timeout into a
    // busy loop instead of a 250 ms tick.
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let _watcher = build_watcher(&path, tx.clone());

    let mut live = Live::default();
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(POLL_INTERVAL) {
            // Coalesce the burst of events a single write can produce; one
            // drain of the file covers all of them.
            Ok(_) => while rx.try_recv().is_ok() {},
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(POLL_INTERVAL),
        }

        let (lines, rotated) = tail.poll()?;
        if rotated {
            ui::note("the log rotated or was truncated — following the new file from its start");
        }
        for line in lines {
            live.handle(&line);
        }
    }

    println!();
    ui::success(&format!(
        "Stopped after {} line(s): {} coordinate hit(s), {} chat line(s), {} player event(s).",
        live.lines, live.coords, live.chats, live.events
    ));

    // Prompts are deliberately deferred to here rather than raised mid-tail:
    // the Enter-to-stop thread owns stdin while the loop is running, so a
    // prompt asked during the watch would have its answer eaten by it.
    if let Some(&seed) = live.seeds.last()
        && ui::confirm(
            &format!("Seed {seed} found — use this for modes 1/2/6/8/9/10?"),
            true,
        )?
    {
        session.seed = Some(seed);
        ui::success(&format!("Session seed set to {seed}."));
    }

    if let Some(hit) = live.last_coord {
        logscrape::offer_search_box(session, hit.x, hit.z)?;
    }

    Ok(())
}

/// Mode 7 normally routes here through [`crate::logscrape::run`]; this alias
/// lets the menu jump straight to the live watcher.
pub fn run(session: &mut Session) -> Result<()> {
    watch(session)
}

/// Lets the user pick from the auto-detected logs or type their own path.
fn choose_log_path() -> Result<PathBuf> {
    let found = candidate_log_paths();
    if found.is_empty() {
        ui::note("No Minecraft logs auto-detected on this machine.");
    }

    let mut items: Vec<String> = found.iter().map(|p| p.display().to_string()).collect();
    items.push("Type a custom path…".to_string());

    let choice = ui::select("Which log should I follow?", &items)?;
    if choice < found.len() {
        return Ok(found[choice].clone());
    }

    let raw: String = ui::input("Path to latest.log")?;
    Ok(logscrape::expand_path(&raw))
}

/// Builds a filesystem watcher for the log, or `None` if the platform will not
/// give us one (in which case the 250 ms tick in [`watch`] is the fallback).
///
/// The **directory** is watched rather than the file: when a log rotates, the
/// old inode stops receiving writes, and a watch registered against it goes
/// permanently quiet even though `latest.log` is alive and growing again.
fn build_watcher(
    path: &Path,
    tx: mpsc::Sender<notify::Result<notify::Event>>,
) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    match notify::recommended_watcher(tx) {
        Ok(mut watcher) => match watcher.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => Some(watcher),
            Err(e) => {
                ui::warn(&format!(
                    "could not watch {}: {e} — polling every {} ms instead",
                    dir.display(),
                    POLL_INTERVAL.as_millis()
                ));
                None
            }
        },
        Err(e) => {
            ui::warn(&format!(
                "no filesystem watcher available ({e}) — polling every {} ms instead",
                POLL_INTERVAL.as_millis()
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_all_exist() {
        // This machine may have no Minecraft installed at all, so an empty
        // result is a pass; what must never happen is a path being offered
        // that is not there.
        for path in candidate_log_paths() {
            assert!(
                path.is_file(),
                "{} was offered but is not a file",
                path.display()
            );
            assert!(path.ends_with("logs/latest.log") || path.ends_with("logs\\latest.log"));
        }
    }

    #[test]
    fn candidate_paths_are_unique() {
        let paths = candidate_log_paths();
        let mut sorted = paths.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(paths.len(), sorted.len(), "duplicate log paths offered");
    }

    #[test]
    fn seed_replies_are_recognised() {
        assert_eq!(sniff_seed("Seed: [12345]"), Some(12345));
        assert_eq!(sniff_seed("Seed: 12345"), Some(12345));
        assert_eq!(
            sniff_seed("seed: [-4172144997902289642]"),
            Some(-4172144997902289642)
        );
        assert_eq!(
            sniff_seed("[12:34:56] [Render thread/INFO]: [CHAT] Seed: [-987654321]"),
            Some(-987654321)
        );
        // Not a /seed reply.
        assert_eq!(sniff_seed("Reseed: 5"), None);
        assert_eq!(sniff_seed("Seed is unknown"), None);
        assert_eq!(sniff_seed("[12:34:56] [main/INFO]: Loading"), None);
    }

    #[test]
    fn both_chat_prefix_layouts_parse() {
        let modern =
            parse_log_line("[12:34:56] [Render thread/INFO]: [CHAT] <Steve> x: 100 y: 64 z: -200");
        assert_eq!(modern.time.as_deref(), Some("12:34:56"));
        assert_eq!(modern.thread.as_deref(), Some("Render thread"));
        assert!(modern.is_chat);
        assert_eq!(modern.message, "<Steve> x: 100 y: 64 z: -200");

        let legacy = parse_log_line("[12:34:56] [Client thread/INFO]: <Steve> hello");
        assert_eq!(legacy.thread.as_deref(), Some("Client thread"));
        assert!(legacy.is_chat);
        assert_eq!(legacy.message, "<Steve> hello");

        let server = parse_log_line("[12:34:56] [Server thread/INFO]: Preparing spawn area");
        assert!(!server.is_chat);
        assert_eq!(server.message, "Preparing spawn area");

        // A line with no recognisable prefix is still usable as a message.
        let bare = parse_log_line("x: 1 y: 2 z: 3");
        assert_eq!(bare.time, None);
        assert_eq!(bare.message, "x: 1 y: 2 z: 3");
    }

    #[test]
    fn join_leave_and_death_messages_are_classified() {
        assert_eq!(classify_event("Steve joined the game"), Some("join"));
        assert_eq!(classify_event("Steve left the game"), Some("leave"));
        assert_eq!(classify_event("Steve was slain by Zombie"), Some("death"));
        assert_eq!(
            classify_event("Steve fell from a high place"),
            Some("death")
        );
        assert_eq!(classify_event("Steve tried to swim in lava"), Some("death"));
        assert_eq!(classify_event("Steve drowned"), Some("death"));
        assert_eq!(classify_event("Preparing spawn area: 42%"), None);
    }

    #[test]
    fn coordinate_scanning_is_delegated_to_logscrape() {
        // The watcher must not carry its own copy of the coordinate patterns;
        // this is the contract that keeps the two paths behaving identically.
        let line = "[12:34:56] [Render thread/INFO]: [CHAT] <Steve> /tp 100 64 -200";
        let hits = logscrape::scan_line(line);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].x, 100.0);
        assert_eq!(hits[0].z, -200.0);
        assert_eq!(hits[0].kind, "tp");
    }

    #[test]
    fn tail_reads_only_appended_lines_and_survives_rotation() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("mc-locate-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latest.log");
        std::fs::write(&path, b"old line that must not be replayed\n").unwrap();

        let mut tail = Tail::open_at_end(&path).unwrap();
        let (lines, rotated) = tail.poll().unwrap();
        assert!(lines.is_empty(), "history must not be replayed");
        assert!(!rotated);

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            // A partial line must be held back until its newline arrives.
            f.write_all(b"first\nsecon").unwrap();
        }
        let (lines, _) = tail.poll().unwrap();
        assert_eq!(lines, vec!["first".to_string()]);

        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"d\n").unwrap();
        }
        let (lines, _) = tail.poll().unwrap();
        assert_eq!(lines, vec!["second".to_string()]);

        // Rotation: the file is replaced by a shorter one.
        std::fs::write(&path, b"fresh\n").unwrap();
        let (lines, rotated) = tail.poll().unwrap();
        assert!(rotated, "a shrinking file must be treated as a rotation");
        assert_eq!(lines, vec!["fresh".to_string()]);

        std::fs::remove_dir_all(&dir).ok();
    }
}
