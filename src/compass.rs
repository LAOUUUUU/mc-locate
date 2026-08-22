//! Mode 8 — work out which way the observer was facing, then find where along
//! that heading the biome sequence they describe actually occurs.
//!
//! The two stages are deliberately separate because they fail differently.
//! Stage 1 is a small, exact bit of trigonometry: Minecraft's orientation cues
//! are fixed by the game, so once a cue is identified the heading follows.
//! Stage 2 is a *fuzzy search*: biome sequences repeat all over a world, the
//! player's distance estimates are eyeballed, and the cue in stage 1 was read
//! off a screenshot to maybe a few degrees. So stage 2 sweeps a cone rather
//! than a ray, and reports ranked candidates rather than an answer.
//!
//! Nothing here tries to be cleverer than the evidence allows. If the user
//! feeds in "plains, forest, plains" the honest output is a long list of
//! near-ties, and that is what gets printed.

use std::cmp::Ordering;
use std::ffi::CStr;
use std::time::Instant;

use anyhow::{Result, bail};
use cubiomes::enums::BiomeID;
use cubiomes_sys::num_traits::FromPrimitive;
use indicatif::ProgressBar;
use rayon::prelude::*;

use crate::session::{BBox, Session};
use crate::ui;
use crate::worldgen::{Version, WorldGen};

// ---------------------------------------------------------------------------
// Stage 1 — the yaw convention
// ---------------------------------------------------------------------------

/// Minecraft yaw for each cardinal direction.
///
/// Minecraft's yaw is measured from +Z and increases as the player turns to
/// their right (mouse right), which puts west — not east — at 90°:
///
/// ```text
///   yaw    0 = +Z = south
///   yaw   90 = -X = west
///   yaw  180 = -Z = north
///   yaw  270 = +X = east   (also written -90)
/// ```
///
/// This is the same convention the F3 debug screen prints and the same one
/// [`crate::session::Session::heading`] stores, so mode 5's camera pose and
/// this mode's cue-derived heading are directly interchangeable.
pub const YAW_SOUTH: f64 = 0.0;
pub const YAW_WEST: f64 = 90.0;
pub const YAW_NORTH: f64 = 180.0;
pub const YAW_EAST: f64 = 270.0;

/// Wraps any angle into `[0, 360)`.
pub fn normalise_yaw(deg: f64) -> f64 {
    let wrapped = deg % 360.0;
    if wrapped < 0.0 { wrapped + 360.0 } else { wrapped }
}

/// The unit vector the player is looking along, ignoring pitch.
///
/// `dx = -sin(yaw)`, `dz = cos(yaw)` — the sign on `dx` is what makes yaw 90
/// point at -X (west) rather than +X, and getting it backwards mirrors every
/// candidate in stage 2 about the Z axis, which is the sort of bug that still
/// produces plausible-looking output. Hence the cardinal test below.
pub fn facing_vector(yaw_deg: f64) -> (f64, f64) {
    let rad = yaw_deg.to_radians();
    (-rad.sin(), rad.cos())
}

/// Signed angular difference `a - b`, wrapped into `(-180, 180]`.
pub fn angle_delta(a: f64, b: f64) -> f64 {
    let d = normalise_yaw(a - b);
    if d > 180.0 { d - 360.0 } else { d }
}

/// An eight-point compass label, for output only.
pub fn compass_name(yaw_deg: f64) -> &'static str {
    const NAMES: [&str; 8] = ["S", "SW", "W", "NW", "N", "NE", "E", "SE"];
    let idx = (normalise_yaw(yaw_deg) / 45.0).round() as usize % 8;
    NAMES[idx]
}

/// Combines a cue's known world bearing with where the cue sat in the frame.
///
/// `left_offset_deg` is how far the cue appears to the **left** of the centre
/// of the screenshot (negative for right). Centring the cue means turning left
/// by that much, and turning left *decreases* yaw, so the heading the player
/// actually had is the cue's bearing plus the left offset.
pub fn compose_heading(cue_bearing_deg: f64, left_offset_deg: f64) -> f64 {
    normalise_yaw(cue_bearing_deg + left_offset_deg)
}

/// A fixed in-world visual whose orientation is decided by the game rather
/// than by worldgen, and so can be read as a compass.
struct Cue {
    /// Menu text. Unverified cues say so here as well as in the comment.
    label: &'static str,
    /// How the prompt should refer to the thing being measured.
    reference: &'static str,
    /// The absolute Minecraft yaw the reference points along.
    bearing: f64,
    /// False when this claim could not be confirmed against the wiki.
    verified: bool,
    /// Where the direction comes from, so a future reader can re-check it.
    source: &'static str,
}

/// The orientation cues offered in stage 1.
///
/// Every entry below was checked against the Minecraft Wiki before being
/// hardcoded; the `source` field records where. The wiki's Tutorial:Navigation
/// page carries most of them in one table, which is also a useful reminder
/// that several *other* documented cues (planks, farmland, bedrock foliation,
/// jukebox slot) only fix an **axis**, not a direction, and so are deliberately
/// not offered here — they cannot resolve a heading on their own.
const CUES: &[Cue] = &[
    // Verified. Minecraft Wiki, Tutorial:Navigation: "The sun and the moon
    // rise in the east and set in the west. Their paths are always the same,
    // and they are fixed against the stars." The Sun page's history section
    // notes this was only true from Beta 1.9 Prerelease 4 onwards (before
    // that the sun rose in the north), which is comfortably older than the
    // oldest version this tool offers, 1.8.9 — so it holds for every
    // `Version::ALL` entry.
    Cue {
        label: "Sun or moon rising on the horizon (rises in the east)",
        reference: "the rising sun/moon",
        bearing: YAW_EAST,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation and Sun",
    },
    Cue {
        label: "Sun or moon setting on the horizon (sets in the west)",
        reference: "the setting sun/moon",
        bearing: YAW_WEST,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation and Sun",
    },
    // Verified. Tutorial:Navigation: "Clouds always float west, and are
    // visible above-ground during day and night." The Cloud page agrees
    // ("Clouds always float westward between layer 192 and 196") and notes
    // cloud position is driven by world time, so it is identical for every
    // player on a server — which is what makes it usable from a screenshot.
    // Note this is the direction clouds move *towards*, not where they come
    // from.
    Cue {
        label: "Cloud drift — the direction the clouds are moving towards (west)",
        reference: "the direction the clouds are drifting towards",
        bearing: YAW_WEST,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation and Cloud",
    },
    // Verified, but not as described in folklore. The wiki documents a "T",
    // on the **top face**, for the whole brick family — Tutorial:Navigation:
    // "Stone Bricks, Deepslate Bricks, Polished Blackstone Bricks, End Stone
    // Bricks ... have a 'T' shape on their tops, and the bottom of the 'T'
    // always point south." It is the stem of the T that points south, and the
    // cue only works looking down at a top face. Caveat worth knowing: the
    // wiki gives no version qualification, but the 1.14 Texture Update redrew
    // most block textures, so treat this with suspicion on pre-1.14 worlds.
    Cue {
        label: "Stone/deepslate/blackstone/end-stone brick top face — stem of the \"T\" (south)",
        reference: "the stem of the \"T\" on the block's top face",
        bearing: YAW_SOUTH,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation",
    },
    // Verified. Sunflower page: "Sunflowers always face east, making them
    // useful for navigation if the sun is not visible." Tutorial:Navigation
    // repeats it. Unlike a real sunflower this does not track the sun, so it
    // works at any time of day.
    Cue {
        label: "Sunflower — the face of the flower head (east)",
        reference: "the direction the sunflower head is facing",
        bearing: YAW_EAST,
        verified: true,
        source: "Minecraft Wiki, Sunflower and Tutorial:Navigation",
    },
    // Verified. Tutorial:Navigation: "The feather on top of the block always
    // faces north." The fletching table only exists from 1.14 onwards.
    Cue {
        label: "Fletching table top face — the feather (north)",
        reference: "the direction the feather points",
        bearing: YAW_NORTH,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation",
    },
    // Verified, Java-only. Tutorial:Navigation: "in Java Edition the block
    // breaking texture is always oriented the same way, with the first few
    // frames forming a 'Y' shape which, when appearing on top of a block,
    // always points south (i.e. the bottom line points south...)". Useful
    // because it needs no particular block, just a screenshot mid-mine.
    Cue {
        label: "Block-breaking overlay on a top face — stem of the \"Y\" (south, Java only)",
        reference: "the stem of the \"Y\" in the breaking overlay",
        bearing: YAW_SOUTH,
        verified: true,
        source: "Minecraft Wiki, Tutorial:Navigation",
    },
    // NOT VERIFIED. The prompt for this mode suggested an "L"/notch in the
    // cobblestone texture pointing north. The wiki's Tutorial:Navigation
    // block-texture table lists crafting table, jukebox, the brick family,
    // sunflower, planks, farmland, bedrock, observer and fletching table —
    // cobblestone is absent, and the Cobblestone page says nothing about
    // texture orientation. The claim only turns up in forum and blog posts,
    // and cobblestone was redrawn in the 1.14 Texture Update, so even if it
    // were once true it would be version-dependent. Offered, but flagged
    // loudly, rather than silently presented as fact.
    Cue {
        label: "Cobblestone \"L\"/notch points north  [UNVERIFIED — not documented on the wiki]",
        reference: "the \"L\"/notch in the cobblestone texture",
        bearing: YAW_NORTH,
        verified: false,
        source: "community folklore only; absent from the Minecraft Wiki",
    },
];

/// Resolves an absolute heading, reusing [`Session::heading`] when mode 5 has
/// already worked one out.
fn prompt_heading(session: &mut Session) -> Result<f64> {
    if let Some(h) = session.heading
        && ui::confirm(
            &format!(
                "Reuse the heading already in the session ({:.1}° / {})?",
                h,
                compass_name(h)
            ),
            true,
        )?
    {
        return Ok(h);
    }

    let mut items: Vec<String> = CUES.iter().map(|c| c.label.to_string()).collect();
    items.push("Type an absolute yaw directly (F3 screen, or already known)".to_string());
    let choice = ui::select("Which orientation cue can you see?", &items)?;

    let heading = if choice == CUES.len() {
        let yaw: f64 = ui::input_default("Absolute yaw in degrees", 0.0)?;
        let offset: f64 =
            ui::input_default("Extra offset — degrees the target sits LEFT of centre", 0.0)?;
        compose_heading(yaw, offset)
    } else {
        let cue = &CUES[choice];
        if !cue.verified {
            ui::warn(&format!(
                "This cue is UNVERIFIED ({}). Treat the resulting heading as a guess, \
                 and prefer a second cue if you have one.",
                cue.source
            ));
            if !ui::confirm("Use it anyway?", false)? {
                bail!("no orientation cue chosen");
            }
        } else {
            ui::note(&format!("Source: {}", cue.source));
        }
        ui::note(&format!(
            "{} points at yaw {:.0}° ({}).",
            cue.reference,
            cue.bearing,
            compass_name(cue.bearing)
        ));
        ui::note("Positive offset = the cue sits to the LEFT of the centre of your screenshot.");
        let offset: f64 = ui::input_default(
            &format!("Degrees {} sits left of centre", cue.reference),
            0.0,
        )?;
        compose_heading(cue.bearing, offset)
    };

    ui::success(&format!(
        "Heading {:.1}° ({}) — facing vector ({:+.3}, {:+.3})",
        heading,
        compass_name(heading),
        facing_vector(heading).0,
        facing_vector(heading).1
    ));
    session.heading = Some(heading);
    Ok(heading)
}

// ---------------------------------------------------------------------------
// Stage 2 — matching a biome sequence along the heading
// ---------------------------------------------------------------------------

/// One leg of the walk as the user remembers it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservedStep {
    pub biome: BiomeID,
    /// Roughly how many blocks this biome went on for, if the user could
    /// estimate it. `None` means "no idea" and is scored on order only.
    pub span: Option<f64>,
}

/// A stretch of one biome as the generator actually produces it along a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    pub biome: BiomeID,
    pub length: f64,
}

/// One (angle, start-offset) hypothesis and how well it explains the
/// observations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// Absolute heading of this ray.
    pub yaw: f64,
    /// How far this ray's heading deviates from the cue-derived one.
    pub angle_offset: f64,
    /// How far along the heading the ray starts, relative to the anchor.
    pub start_offset: f64,
    pub start_x: i32,
    pub start_z: i32,
    /// Normalised into `[0, 1]`; 1.0 means every observed leg matched in both
    /// biome and (where given) length.
    pub score: f64,
}

/// Everything the cone sweep needs that is not the observations themselves.
#[derive(Debug, Clone, Copy)]
pub struct SweepParams {
    /// Half-angle of the cone, in degrees.
    pub half_angle: f64,
    pub angular_step: f64,
    /// How far either side of the anchor, along the heading, to try starting.
    pub offset_range: f64,
    pub offset_step: f64,
    /// Spacing between biome samples along a ray.
    pub sample_step: f64,
    /// How far out to walk.
    pub max_range: f64,
    /// Y to sample at. From 1.18 biomes are fully 3D, so this matters;
    /// sea level is the best single proxy for "what a walking player saw".
    pub sample_y: i32,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            half_angle: 10.0,
            angular_step: 2.0,
            offset_range: 256.0,
            offset_step: 64.0,
            sample_step: 32.0,
            max_range: 2000.0,
            sample_y: 63,
        }
    }
}

// Scoring weights. These are not calibrated against anything — they are a
// stated preference ordering (right biomes first, right distances second,
// tolerate the generator's slivers, punish invented legs) and the normalised
// score should be read as "how much of the story fits", not as a probability.
const MATCH_REWARD: f64 = 1.0;
// Deliberately well below MATCH_REWARD. A player reports the biomes they
// crossed far more reliably than how far apart they were — the prompt asks for
// "rough spacing" and accepts "unknown" — so three biomes in the right order
// with badly wrong distances must still outrank a sequence with the wrong
// biome in the middle. Weighting distance equally inverted that ordering.
const DISTANCE_REWARD: f64 = 0.35;
const MISMATCH_PENALTY: f64 = -1.0;
const SKIP_OBSERVED_PENALTY: f64 = -0.5;
/// A generated run shorter than this is cheap to skip: a ray clipping the
/// corner of a river or a beach produces a stretch the player would never
/// have called a separate biome.
const SLIVER_BLOCKS: f64 = 128.0;

fn skip_actual_penalty(length: f64) -> f64 {
    -0.5 * (length / SLIVER_BLOCKS).min(1.0)
}

/// How well a remembered distance agrees with a generated one, in `[0, 1]`.
///
/// The tolerance grows with the estimate because "about 600 blocks" is a much
/// looser claim than "about 60".
fn distance_agreement(observed: f64, actual: f64) -> f64 {
    let tolerance = (observed.abs() * 0.35).max(64.0);
    (-(observed - actual).abs() / tolerance).exp()
}

fn pair_reward(observed: &ObservedStep, actual: &Run) -> f64 {
    if observed.biome != actual.biome {
        return MISMATCH_PENALTY;
    }
    match observed.span {
        Some(span) => MATCH_REWARD + DISTANCE_REWARD * distance_agreement(span, actual.length),
        // Unknown spacing: order still counts, distance simply is not scored.
        None => MATCH_REWARD,
    }
}

/// Scores an observed sequence against what a ray actually crosses.
///
/// This is a Needleman-Wunsch style alignment rather than a positional
/// comparison, because both sides are noisy in different ways: the generator
/// emits slivers the player never noticed, and the player merges or forgets
/// legs. Alignment lets each side skip at a cost instead of throwing the whole
/// match away on one extra run.
///
/// Runs left over at the *far* end are free — the player stopped looking — so
/// the result is the best cell in the last row. Runs before the first match
/// are not free, because that is exactly what the start-offset sweep exists to
/// explain.
pub fn score_sequence(observed: &[ObservedStep], actual: &[Run]) -> f64 {
    if observed.is_empty() {
        return 0.0;
    }
    let max_total: f64 = observed
        .iter()
        .map(|o| MATCH_REWARD + if o.span.is_some() { DISTANCE_REWARD } else { 0.0 })
        .sum();
    if max_total <= 0.0 {
        return 0.0;
    }

    let (m, n) = (observed.len(), actual.len());
    let mut dp = vec![vec![f64::NEG_INFINITY; n + 1]; m + 1];
    dp[0][0] = 0.0;
    for j in 1..=n {
        dp[0][j] = dp[0][j - 1] + skip_actual_penalty(actual[j - 1].length);
    }
    for i in 1..=m {
        dp[i][0] = dp[i - 1][0] + SKIP_OBSERVED_PENALTY;
        for j in 1..=n {
            let paired = dp[i - 1][j - 1] + pair_reward(&observed[i - 1], &actual[j - 1]);
            let skip_actual = dp[i][j - 1] + skip_actual_penalty(actual[j - 1].length);
            let skip_observed = dp[i - 1][j] + SKIP_OBSERVED_PENALTY;
            dp[i][j] = paired.max(skip_actual).max(skip_observed);
        }
    }

    let raw = dp[m].iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (raw / max_total).clamp(0.0, 1.0)
}

/// Walks one ray and collapses the samples into biome runs.
///
/// Deliberately point sampling rather than [`WorldGen::biome_rect`]: a ray is
/// a thin diagonal, so the rectangle enclosing even a ±10° cone at 2 km is
/// order a million block samples, against a few thousand for the rays
/// themselves. `biome_rect` wins for area scans; it loses badly here.
///
/// A run of *k* samples is reported as `k * sample_step` blocks long, which
/// over-counts by up to one step. That is well inside the precision of the
/// distances a player types in, and it keeps the scoring symmetric.
pub fn sample_runs(
    world: &WorldGen,
    start_x: f64,
    start_z: f64,
    yaw_deg: f64,
    params: &SweepParams,
) -> Vec<Run> {
    let (dx, dz) = facing_vector(yaw_deg);
    let steps = (params.max_range / params.sample_step).floor().max(1.0) as i64;
    let mut runs: Vec<Run> = Vec::new();

    for i in 0..=steps {
        let dist = i as f64 * params.sample_step;
        let x = (start_x + dx * dist).round() as i32;
        let z = (start_z + dz * dist).round() as i32;
        // A failure here means cubiomes refused the position (outside the
        // world border, say); the honest response is to stop the ray rather
        // than to invent samples past the edge.
        let Ok(biome) = world.biome_at(x, params.sample_y, z) else {
            break;
        };
        match runs.last_mut() {
            Some(last) if last.biome == biome => last.length += params.sample_step,
            _ => runs.push(Run {
                biome,
                length: params.sample_step,
            }),
        }
    }
    runs
}

/// Ranking order: best score first, then — among ties — the explanation that
/// takes the fewest liberties with what the user told us. Biome sequences tie
/// constantly, so this tiebreak is doing real work: it keeps the ray the user
/// actually described at the top of a block of equal scores instead of
/// surfacing an arbitrary member of it.
fn rank_order(a: &Candidate, b: &Candidate) -> Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(Ordering::Equal)
        .then(
            a.angle_offset
                .abs()
                .partial_cmp(&b.angle_offset.abs())
                .unwrap_or(Ordering::Equal),
        )
        .then(
            a.start_offset
                .abs()
                .partial_cmp(&b.start_offset.abs())
                .unwrap_or(Ordering::Equal),
        )
}

/// Every (angle deviation, along-heading offset) pair the sweep will try.
fn sweep_combinations(params: &SweepParams) -> Vec<(f64, f64)> {
    let angle_steps = if params.angular_step > 0.0 {
        (params.half_angle / params.angular_step).floor().max(0.0) as i64
    } else {
        0
    };
    let offset_steps = if params.offset_step > 0.0 {
        (params.offset_range / params.offset_step).floor().max(0.0) as i64
    } else {
        0
    };

    let mut combos = Vec::with_capacity(((2 * angle_steps + 1) * (2 * offset_steps + 1)) as usize);
    for a in -angle_steps..=angle_steps {
        for o in -offset_steps..=offset_steps {
            combos.push((
                a as f64 * params.angular_step,
                o as f64 * params.offset_step,
            ));
        }
    }
    combos
}

/// Sweeps the cone and returns every candidate, best first.
///
/// Each rayon worker builds its own [`WorldGen`] via `map_init`. cubiomes
/// generators carry mutable internal state (the layer stack and noise caches
/// are scratch space, not just configuration), so sharing one across threads
/// would be a data race even though the Rust wrapper marks it `Sync`.
pub fn sweep(
    version: Version,
    seed: i64,
    anchor: (f64, f64),
    heading: f64,
    observed: &[ObservedStep],
    params: &SweepParams,
    progress: Option<&ProgressBar>,
) -> Vec<Candidate> {
    let combos = sweep_combinations(params);

    let mut candidates: Vec<Candidate> = combos
        .par_iter()
        .map_init(
            || WorldGen::overworld(version, seed),
            |world, &(angle_offset, start_offset)| {
                let yaw = normalise_yaw(heading + angle_offset);
                let (dx, dz) = facing_vector(yaw);
                let start_x = anchor.0 + dx * start_offset;
                let start_z = anchor.1 + dz * start_offset;
                let runs = sample_runs(world, start_x, start_z, yaw, params);
                let score = score_sequence(observed, &runs);
                if let Some(pb) = progress {
                    pb.inc(1);
                }
                Candidate {
                    yaw,
                    angle_offset,
                    start_offset,
                    start_x: start_x.round() as i32,
                    start_z: start_z.round() as i32,
                    score,
                }
            },
        )
        .collect();

    candidates.sort_by(rank_order);
    candidates
}

// ---------------------------------------------------------------------------
// Biome names
// ---------------------------------------------------------------------------

/// Highest `BiomeID` discriminant cubiomes currently defines is 186
/// (`pale_garden`); scan a little past it so a newer cubiomes still works.
const BIOME_ID_SCAN_MAX: i32 = 255;

/// Every biome that exists in `version`, as `(id, resource name)`, sorted by
/// name.
///
/// `BiomeID` implements **neither** `FromStr` nor `Display`, contrary to what
/// one might assume from its sibling enums. cubiomes-sys says why on
/// `BiomeID::to_mc_biome_str`: several biomes were renamed in 1.18
/// (`snowy_tundra` → `snowy_plains`, `jungle_edge` → `sparse_jungle`, and
/// friends) without changing id, so the name is a function of *(id, version)*
/// and a single global impl would be wrong for half the versions. The table is
/// therefore built per version here.
///
/// The two C calls are used directly instead of the safe
/// `to_mc_biome_str` wrapper because that wrapper `assert!`s on a null return
/// — i.e. it panics for any id absent from the chosen version, which is
/// exactly the case we need to detect in order to filter the list.
pub fn biome_catalogue(version: Version) -> Vec<(BiomeID, &'static str)> {
    let mc = version.mc() as i32;
    let mut out = Vec::new();

    for id in 0..=BIOME_ID_SCAN_MAX {
        // SAFETY: both functions are pure lookups over plain `int`s with no
        // preconditions. `biomeExists` returns 0 for anything it does not
        // know, and `biome2str` returns NULL, which is checked below. The
        // strings it returns are C string literals, hence `'static`.
        let exists = unsafe { cubiomes_sys::biomeExists(mc, id) } != 0;
        if !exists {
            continue;
        }
        let ptr = unsafe { cubiomes_sys::biome2str(mc, id) };
        if ptr.is_null() {
            continue;
        }
        let Ok(name) = (unsafe { CStr::from_ptr(ptr) }).to_str() else {
            continue;
        };
        let Some(biome) = BiomeID::from_i32(id) else {
            continue;
        };
        out.push((biome, name));
    }

    out.sort_by_key(|(_, name)| *name);
    out
}

/// The resource name for one biome in one version, or a debug fallback.
pub fn biome_name(version: Version, biome: BiomeID) -> String {
    biome_catalogue(version)
        .into_iter()
        .find(|(b, _)| *b == biome)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("{biome:?}"))
}

/// Picks one biome by substring search, so nobody has to spell
/// `modified_wooded_badlands_plateau` from memory.
fn prompt_biome(catalogue: &[(BiomeID, &'static str)], prompt: &str) -> Result<BiomeID> {
    loop {
        let query: String = ui::input_optional(&format!("{prompt} (type part of the name, blank to list all)"))?;
        let needle = query.trim().to_ascii_lowercase().replace(' ', "_");

        let matches: Vec<&(BiomeID, &'static str)> = catalogue
            .iter()
            .filter(|(_, name)| needle.is_empty() || name.contains(&needle))
            .collect();

        if matches.is_empty() {
            ui::warn(&format!("nothing matches {query:?} in this version; try again"));
            continue;
        }
        if matches.len() == 1 {
            ui::note(&format!("→ {}", matches[0].1));
            return Ok(matches[0].0);
        }

        let labels: Vec<String> = matches.iter().map(|(_, n)| (*n).to_string()).collect();
        let idx = ui::select("Which biome?", &labels)?;
        return Ok(matches[idx].0);
    }
}

/// Reads the ordered biome sequence, with optional per-leg spacing.
fn prompt_observations(version: Version) -> Result<Vec<ObservedStep>> {
    let catalogue = biome_catalogue(version);
    ui::note(&format!(
        "{} biomes exist in {}.",
        catalogue.len(),
        version.label()
    ));

    let mut steps = Vec::new();
    loop {
        let n = steps.len() + 1;
        let biome = prompt_biome(&catalogue, &format!("Biome #{n} along the heading"))?;
        let raw: String =
            ui::input_optional("  …how many blocks did it go on for? (blank = unknown)")?;
        let span = match raw.trim() {
            "" => None,
            other => match other.parse::<f64>() {
                Ok(v) if v > 0.0 => Some(v),
                _ => {
                    ui::warn("not a positive number — recording that leg as unknown spacing");
                    None
                }
            },
        };
        steps.push(ObservedStep { biome, span });

        if steps.len() >= 2 && !ui::confirm("Add another biome?", true)? {
            break;
        }
    }
    Ok(steps)
}

fn prompt_params(observed: &[ObservedStep]) -> Result<SweepParams> {
    let defaults = SweepParams::default();

    // A sensible default range is however far the user says they walked, plus
    // headroom for the offset sweep and for legs they gave no distance for.
    let known: f64 = observed.iter().filter_map(|o| o.span).sum();
    let unknown = observed.iter().filter(|o| o.span.is_none()).count() as f64;
    let suggested_range = if known > 0.0 || unknown > 0.0 {
        (known * 1.5 + unknown * 400.0).clamp(512.0, 16_000.0)
    } else {
        defaults.max_range
    };

    let half_angle: f64 = ui::input_default("Cone half-angle (degrees)", defaults.half_angle)?;
    let angular_step: f64 = ui::input_default("Angular step (degrees)", defaults.angular_step)?;
    let offset_range: f64 = ui::input_default(
        "Anchor uncertainty along the heading (± blocks)",
        defaults.offset_range,
    )?;
    let offset_step: f64 = ui::input_default("Anchor offset step (blocks)", defaults.offset_step)?;
    let sample_step: f64 =
        ui::input_default("Sampling step along each ray (blocks)", defaults.sample_step)?;
    let max_range: f64 =
        ui::input_default("How far to walk each ray (blocks)", suggested_range.round())?;
    let sample_y: i32 = ui::input_default(
        "Y to sample biomes at (1.18+ biomes are 3D; sea level is the usual proxy)",
        defaults.sample_y,
    )?;

    if half_angle < 0.0 || angular_step <= 0.0 || sample_step <= 0.0 || max_range <= 0.0 {
        bail!("angles and distances must be positive");
    }

    Ok(SweepParams {
        half_angle,
        angular_step,
        offset_range: offset_range.max(0.0),
        offset_step: if offset_step > 0.0 { offset_step } else { 1.0 },
        sample_step,
        max_range,
        sample_y,
    })
}

fn print_candidates(version: Version, candidates: &[Candidate], top_n: usize, heading: f64) {
    println!();
    println!("  {:>3}  {:>6}  {:>14}  {:>18}  {:>9}  {:>9}", "#", "score", "heading", "start point", "angle Δ", "along Δ");
    println!("  {}", "─".repeat(72));
    for (i, c) in candidates.iter().take(top_n).enumerate() {
        println!(
            "  {:>3}  {:>6.3}  {:>9.1}° ({:<2})  {:>8}, {:>7}  {:>+8.1}°  {:>+8.0}",
            i + 1,
            c.score,
            c.yaw,
            compass_name(c.yaw),
            c.start_x,
            c.start_z,
            c.angle_offset,
            c.start_offset
        );
    }
    let _ = (version, heading);
}

/// Radius that honestly covers the candidates the data cannot tell apart.
///
/// Anything scoring within 5% of the best is, for this method, indistinguishable
/// from the best; the box has to be big enough to contain all of them, plus one
/// sweep granule so we are not claiming sub-step precision.
fn residual_radius(candidates: &[Candidate], params: &SweepParams) -> i32 {
    let Some(best) = candidates.first() else {
        return params.sample_step.round() as i32;
    };
    let cutoff = best.score - 0.05;
    let spread = candidates
        .iter()
        .filter(|c| c.score >= cutoff)
        .map(|c| {
            let dx = (c.start_x - best.start_x) as f64;
            let dz = (c.start_z - best.start_z) as f64;
            (dx * dx + dz * dz).sqrt()
        })
        .fold(0.0_f64, f64::max);

    let granule = params.sample_step.max(params.offset_step);
    // The angular uncertainty also translates into position error at range.
    let angular = params.max_range * params.half_angle.to_radians();
    (spread + granule + angular * 0.5).round().max(64.0) as i32
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 8 — compass + biome triangulation estimator");

    if !ui::is_interactive() {
        bail!("mode 8 is interactive; run it from a terminal");
    }

    ui::note(
        "Stage 1 turns an orientation cue into an absolute heading. Stage 2 sweeps a cone \
         around that heading looking for the biome sequence you saw.",
    );

    // ---- Stage 1 -----------------------------------------------------------
    let heading = prompt_heading(session)?;

    // ---- Stage 2 -----------------------------------------------------------
    let seed = ui::prompt_seed(session)?;
    let version = ui::prompt_version(session)?;
    ui::note("Biomes are sampled in the Overworld.");

    let anchor_x: f64 = ui::input_default("Anchor X (roughly where the sequence started)", 0.0)?;
    let anchor_z: f64 = ui::input_default("Anchor Z", 0.0)?;

    let observed = prompt_observations(version)?;
    let params = prompt_params(&observed)?;

    let combos = sweep_combinations(&params).len();
    if combos == 0 {
        bail!("the sweep is empty — check the angular and offset steps");
    }

    // Estimate from one real ray rather than a guessed constant. The generator
    // is built outside the timer because it is per-thread, not per-combination.
    let probe = WorldGen::overworld(version, seed);
    let started = Instant::now();
    let _ = sample_runs(&probe, anchor_x, anchor_z, heading, &params);
    let per_combo = started.elapsed().as_secs_f64();
    drop(probe);

    let threads = rayon::current_num_threads().max(1) as f64;
    let estimate = per_combo * combos as f64 / threads;
    ui::note(&format!(
        "{combos} (angle, offset) combinations × {} samples each — about {}",
        (params.max_range / params.sample_step).round() as i64 + 1,
        ui::humanize_duration(estimate)
    ));

    let pb = ui::progress_bar(combos as u64, "sweeping");
    let candidates = sweep(
        version,
        seed,
        (anchor_x, anchor_z),
        heading,
        &observed,
        &params,
        Some(&pb),
    );
    pb.finish_and_clear();

    if candidates.is_empty() {
        bail!("the sweep produced no candidates");
    }

    let top_n: usize = ui::input_default("How many candidates to show", 10usize)?;
    print_candidates(version, &candidates, top_n.max(1), heading);

    println!();
    ui::warn("These are RANKED GUESSES, not a determination.");
    ui::note("Biome sequences repeat across a world, spacing estimates are rough, and the");
    ui::note("heading cue itself is only good to a few degrees. A high score means \"consistent");
    ui::note("with what you described\", not \"this is where you are\". Treat the top entries as");
    ui::note("a shortlist to check against a second, independent observation.");

    let best = candidates[0];
    let runners_up = candidates
        .iter()
        .filter(|c| c.score >= best.score - 1e-9)
        .count();
    if runners_up > 1 {
        ui::warn(&format!(
            "{runners_up} candidates tie for the top score — the sequence you gave does not \
             distinguish between them."
        ));
    }
    if best.score < 0.6 {
        ui::warn(&format!(
            "Even the best candidate only scores {:.2}. Nothing in this cone really matches; \
             check the heading, the anchor, or whether the walk was longer than the range swept.",
            best.score
        ));
    }

    // A few named biomes make the top hit easier to sanity-check by eye.
    let world = WorldGen::overworld(version, seed);
    let runs = sample_runs(&world, best.start_x as f64, best.start_z as f64, best.yaw, &params);
    if !runs.is_empty() {
        ui::note("Top candidate actually crosses:");
        let described: Vec<String> = runs
            .iter()
            .take(observed.len() + 2)
            .map(|r| format!("{} ({:.0}b)", biome_name(version, r.biome), r.length))
            .collect();
        ui::note(&format!("  {}", described.join(" → ")));
    }

    if ui::confirm("Store the best candidate as the session search box?", true)? {
        let radius = residual_radius(&candidates, &params);
        let bbox = BBox::around(best.start_x, best.start_z, radius);
        session.search_box = Some(bbox);
        ui::success(&format!("Search box set to {bbox}"));
        ui::note("Mode 2 or 6 can now refine this against a different kind of observation.");
    }

    ui::pause();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VERSION: Version = Version::V1_21_1;
    const TEST_SEED: i64 = 1234;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn yaw_maps_to_the_documented_facing_vector() {
        // yaw 0 = +Z (south), 90 = -X (west), 180 = -Z (north), 270 = +X (east)
        let cases = [
            (YAW_SOUTH, 0.0, 1.0, "S"),
            (YAW_WEST, -1.0, 0.0, "W"),
            (YAW_NORTH, 0.0, -1.0, "N"),
            (YAW_EAST, 1.0, 0.0, "E"),
        ];
        for (yaw, want_dx, want_dz, name) in cases {
            let (dx, dz) = facing_vector(yaw);
            assert!(
                (dx - want_dx).abs() < 1e-12 && (dz - want_dz).abs() < 1e-12,
                "yaw {yaw} gave ({dx}, {dz}), expected ({want_dx}, {want_dz})"
            );
            assert_eq!(compass_name(yaw), name);
        }

        // -90 is the other spelling of 270, and must behave identically.
        let (dx, dz) = facing_vector(-90.0);
        assert!(approx(dx, 1.0) && approx(dz, 0.0));
        assert!(approx(normalise_yaw(-90.0), YAW_EAST));
    }

    #[test]
    fn heading_and_offset_compose_and_wrap_across_zero() {
        // Sun rising (east, 270) sitting 30° left of centre: the player has
        // turned 30° right of east, i.e. towards south.
        assert!(approx(compose_heading(YAW_EAST, 30.0), 300.0));

        // Wrapping upward past 360.
        assert!(approx(compose_heading(YAW_EAST, 120.0), 30.0));
        // …and downward past 0.
        assert!(approx(compose_heading(YAW_SOUTH, -30.0), 330.0));
        // A full turn is a no-op.
        assert!(approx(compose_heading(YAW_WEST, 360.0), YAW_WEST));
        assert!(approx(compose_heading(YAW_WEST, -720.0), YAW_WEST));

        // Signed differences wrap the short way round.
        assert!(approx(angle_delta(10.0, 350.0), 20.0));
        assert!(approx(angle_delta(350.0, 10.0), -20.0));
        assert!(approx(angle_delta(YAW_NORTH, YAW_SOUTH), 180.0));
    }

    #[test]
    fn every_offered_cue_has_a_cardinal_bearing_and_a_source() {
        for cue in CUES {
            assert!(
                [YAW_SOUTH, YAW_WEST, YAW_NORTH, YAW_EAST].contains(&cue.bearing),
                "{} has a non-cardinal bearing",
                cue.label
            );
            assert!(!cue.source.is_empty());
            // An unverified cue must say so in the menu text too, not only in
            // the source field, so nobody picks it thinking it is a fact.
            if !cue.verified {
                assert!(
                    cue.label.contains("UNVERIFIED"),
                    "{} is unverified but does not say so in its label",
                    cue.label
                );
            }
        }
    }

    #[test]
    fn biome_catalogue_tracks_the_1_18_renames() {
        let old = biome_catalogue(Version::V1_16_5);
        let new = biome_catalogue(Version::V1_21_1);
        assert!(!old.is_empty() && !new.is_empty());

        // Same id, different resource name either side of 1.18 — the reason
        // BiomeID has no blanket Display/FromStr impl.
        assert_eq!(biome_name(Version::V1_16_5, BiomeID::snowy_tundra), "snowy_tundra");
        assert_eq!(biome_name(Version::V1_21_1, BiomeID::snowy_tundra), "snowy_plains");

        // And a biome that simply did not exist yet is absent, rather than
        // panicking the way the safe `to_mc_biome_str` wrapper would.
        assert!(!old.iter().any(|(b, _)| *b == BiomeID::cherry_grove));
        assert!(new.iter().any(|(b, _)| *b == BiomeID::cherry_grove));
    }

    #[test]
    fn a_wrong_biome_in_the_middle_scores_worse_than_the_exact_sequence() {
        let actual = vec![
            Run { biome: BiomeID::plains, length: 300.0 },
            Run { biome: BiomeID::forest, length: 150.0 },
            Run { biome: BiomeID::desert, length: 400.0 },
        ];
        let exact = vec![
            ObservedStep { biome: BiomeID::plains, span: Some(300.0) },
            ObservedStep { biome: BiomeID::forest, span: Some(150.0) },
            ObservedStep { biome: BiomeID::desert, span: Some(400.0) },
        ];
        let wrong_middle = vec![
            ObservedStep { biome: BiomeID::plains, span: Some(300.0) },
            ObservedStep { biome: BiomeID::jungle, span: Some(150.0) },
            ObservedStep { biome: BiomeID::desert, span: Some(400.0) },
        ];

        let exact_score = score_sequence(&exact, &actual);
        let wrong_score = score_sequence(&wrong_middle, &actual);
        assert!(approx(exact_score, 1.0), "exact sequence scored {exact_score}");
        assert!(
            wrong_score < exact_score,
            "wrong middle scored {wrong_score}, not worse than {exact_score}"
        );

        // Wrong distances should also cost something, without destroying the
        // match — the order is still right.
        let bad_spacing = vec![
            ObservedStep { biome: BiomeID::plains, span: Some(1200.0) },
            ObservedStep { biome: BiomeID::forest, span: Some(1200.0) },
            ObservedStep { biome: BiomeID::desert, span: Some(1200.0) },
        ];
        let spacing_score = score_sequence(&bad_spacing, &actual);
        assert!(spacing_score < exact_score && spacing_score > wrong_score);
    }

    #[test]
    fn unknown_spacing_is_not_scored_on_distance() {
        let actual = vec![
            Run { biome: BiomeID::plains, length: 32.0 },
            Run { biome: BiomeID::forest, length: 4096.0 },
        ];
        // Wildly different run lengths, but no claim was made about them, so
        // the order alone should be a perfect match.
        let observed = vec![
            ObservedStep { biome: BiomeID::plains, span: None },
            ObservedStep { biome: BiomeID::forest, span: None },
        ];
        assert!(approx(score_sequence(&observed, &actual), 1.0));

        // Mixing known and unknown must not blow up either, and the unknown
        // leg must not be penalised for the length it never claimed.
        let mixed = vec![
            ObservedStep { biome: BiomeID::plains, span: Some(32.0) },
            ObservedStep { biome: BiomeID::forest, span: None },
        ];
        assert!(approx(score_sequence(&mixed, &actual), 1.0));

        // Degenerate inputs are answers, not panics.
        assert_eq!(score_sequence(&[], &actual), 0.0);
        assert_eq!(score_sequence(&observed, &[]), 0.0);
    }

    #[test]
    fn round_trip_recovers_the_true_anchor_and_heading() {
        // Short rays and a coarse cone: this has to stay a seconds-long test.
        let params = SweepParams {
            half_angle: 10.0,
            angular_step: 5.0,
            offset_range: 128.0,
            offset_step: 64.0,
            sample_step: 32.0,
            max_range: 1024.0,
            sample_y: 63,
        };
        let anchor = (0.0, 0.0);
        let true_heading = YAW_WEST;

        // Take the real biome sequence straight out of the generator, so the
        // observations are exactly what a perfect observer would have reported.
        let world = WorldGen::overworld(TEST_VERSION, TEST_SEED);
        let runs = sample_runs(&world, anchor.0, anchor.1, true_heading, &params);
        assert!(
            runs.len() >= 3,
            "test ray only crosses {} biome(s); pick a livelier seed or heading",
            runs.len()
        );

        let observed: Vec<ObservedStep> = runs
            .iter()
            .take(4)
            .map(|r| ObservedStep {
                biome: r.biome,
                span: Some(r.length),
            })
            .collect();

        let candidates = sweep(
            TEST_VERSION,
            TEST_SEED,
            anchor,
            true_heading,
            &observed,
            &params,
            None,
        );
        assert!(!candidates.is_empty());

        // The truth must be the best explanation available, and the tiebreak
        // (prefer no angular deviation, prefer no anchor offset) must put it
        // first among any equally-scoring rays.
        let best = candidates[0];
        assert!(
            approx(best.angle_offset, 0.0) && approx(best.start_offset, 0.0),
            "expected the true ray to rank first, got angle Δ {:+.1}°, along Δ {:+.0} (score {:.3})",
            best.angle_offset,
            best.start_offset,
            best.score
        );
        assert!(
            best.score > 0.99,
            "the generator's own sequence should score ~1.0, got {:.3}",
            best.score
        );
        assert_eq!((best.start_x, best.start_z), (0, 0));
        assert!(approx(best.yaw, true_heading));

        // Feeding the same sequence back in with the spacing thrown away must
        // still put the truth on top — order alone is enough here.
        let orderless: Vec<ObservedStep> = observed
            .iter()
            .map(|o| ObservedStep {
                biome: o.biome,
                span: None,
            })
            .collect();
        let loose = sweep(
            TEST_VERSION,
            TEST_SEED,
            anchor,
            true_heading,
            &orderless,
            &params,
            None,
        );
        assert!(loose[0].score > 0.99);
        assert!(approx(loose[0].angle_offset, 0.0) && approx(loose[0].start_offset, 0.0));
    }

    #[test]
    fn sweep_geometry_is_symmetric_and_complete() {
        let params = SweepParams {
            half_angle: 10.0,
            angular_step: 5.0,
            offset_range: 128.0,
            offset_step: 64.0,
            ..SweepParams::default()
        };
        // 5 angles (-10, -5, 0, +5, +10) × 5 offsets (-128..+128 by 64).
        let combos = sweep_combinations(&params);
        assert_eq!(combos.len(), 25);
        assert!(combos.contains(&(0.0, 0.0)));
        assert!(combos.iter().any(|&(a, o)| approx(a, -10.0) && approx(o, -128.0)));
        assert!(combos.iter().any(|&(a, o)| approx(a, 10.0) && approx(o, 128.0)));

        // A zero-width cone still produces the single nominal ray rather than
        // an empty sweep.
        let point = SweepParams {
            half_angle: 0.0,
            offset_range: 0.0,
            ..params
        };
        assert_eq!(sweep_combinations(&point), vec![(0.0, 0.0)]);
    }

    #[test]
    fn residual_radius_grows_with_the_uncertainty_swept() {
        let tight = SweepParams {
            half_angle: 1.0,
            max_range: 1000.0,
            ..SweepParams::default()
        };
        let loose = SweepParams {
            half_angle: 20.0,
            max_range: 1000.0,
            ..SweepParams::default()
        };
        let candidates = vec![Candidate {
            yaw: YAW_WEST,
            angle_offset: 0.0,
            start_offset: 0.0,
            start_x: 0,
            start_z: 0,
            score: 1.0,
        }];
        assert!(residual_radius(&candidates, &tight) < residual_radius(&candidates, &loose));
        assert!(residual_radius(&[], &tight) > 0);
    }
}
