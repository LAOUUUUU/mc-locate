//! Mode 1 — the Nether bedrock toolkit.
//!
//! # How 1.18+ nether bedrock is generated
//!
//! Since 1.18 the Nether's bedrock floor and roof are seed-dependent, which is
//! what makes them useful both for locating yourself (1a) and for recovering a
//! seed (1b). The derivation, ported from 19MisterX98's `Nether_Bedrock_Cracker`
//! (`bedrock_cracker/src/block_data.rs` and `layer.rs`) and cross-checked
//! against the test vectors that repository ships:
//!
//! ```text
//! rand       = new Random(worldSeed).nextLong()
//! floorSeed  = new Random(rand ^ hash("minecraft:bedrock_floor")).nextLong() & 2^48-1
//! roofSeed   = new Random(rand ^ hash("minecraft:bedrock_roof" )).nextLong() & 2^48-1
//! ```
//!
//! then for each individual block position:
//!
//! ```text
//! h = (x * 3129871) ^ (z * 116129781L) ^ y        // x term in 32-bit, z term in 64-bit
//! h = h*h*42317861 + h*11
//! posHash = (unsigned) h >> 16
//! value = new Random(layerSeed ^ posHash).nextFloat()
//! ```
//!
//! The threshold is a linear gradient over the five candidate layers:
//!
//! * **Floor** (y 0..=4): bedrock when `value < (5 - y) / 5`.
//!   y=0 is therefore always bedrock and y=4 is bedrock 20% of the time.
//! * **Roof** (y 122..=127, using `l = y - 122`): bedrock when
//!   `value >= (5 - l) / 5`. y=127 is always bedrock and y=123 is bedrock 20%
//!   of the time.
//!
//! y=4 and y=123 are the informative layers — bedrock is rarest there, so each
//! observed block carries the most information. That is why the reference
//! cracker tells users to collect from those two layers specifically.
//!
//! # Pre-1.18
//!
//! Before 1.18 the nether bedrock RNG was seeded from the chunk coordinates
//! alone (`ChunkRandom.setTerrainSeed`), with no world seed mixed in. That is
//! why tools like `BedrockFinder` can locate you from a bedrock pattern without
//! knowing the seed — and equally why sub-mode 1b cannot exist for those
//! versions: there is no seed information in the pattern to recover.
//!
//! We deliberately do **not** implement pre-1.18 pattern matching here. Doing
//! it correctly requires reproducing the exact order in which the surface
//! builder consumes a single per-chunk RNG across every column, and we could
//! not verify that order against a primary source. Guessing it would produce
//! confident, wrong coordinates, so the mode says so and points elsewhere
//! instead.

use anyhow::{Result, bail};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::grid::{Cell, Grid};
use crate::random::JavaRandom;
use crate::session::{BBox, BedrockObservation, Session};
use crate::ui;

/// `"minecraft:bedrock_floor".hashCode()`.
pub const FLOOR_HASH: i64 = 2042456806;
/// `"minecraft:bedrock_roof".hashCode()`.
pub const ROOF_HASH: i64 = 343340730;

const MASK48: u64 = (1 << 48) - 1;

/// Which of the two bedrock surfaces a y-level belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Floor,
    Roof,
}

impl Surface {
    pub fn of(y: i32) -> Surface {
        // The reference cracker splits on y < 64, i.e. anything in the lower
        // half of the Nether is floor.
        if y < 64 { Surface::Floor } else { Surface::Roof }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Surface::Floor => "floor",
            Surface::Roof => "roof",
        }
    }

    /// The layer whose bedrock is rarest, and therefore most informative.
    pub fn most_informative_y(&self) -> i32 {
        match self {
            Surface::Floor => 4,
            Surface::Roof => 123,
        }
    }
}

/// The two per-surface layer seeds derived from a world seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerSeeds {
    pub floor: u64,
    pub roof: u64,
}

/// Derives the floor and roof layer seeds from a world seed.
pub fn layer_seeds(world_seed: i64) -> LayerSeeds {
    let rand = JavaRandom::new(world_seed).next_long();
    LayerSeeds {
        floor: (JavaRandom::new(rand ^ FLOOR_HASH).next_long() as u64) & MASK48,
        roof: (JavaRandom::new(rand ^ ROOF_HASH).next_long() as u64) & MASK48,
    }
}

impl LayerSeeds {
    pub fn for_surface(&self, s: Surface) -> u64 {
        match s {
            Surface::Floor => self.floor,
            Surface::Roof => self.roof,
        }
    }
}

/// The position hash mixed into the per-block RNG.
///
/// Note the asymmetry, which mirrors the reference implementation: the x term
/// is computed in 32-bit and then widened, while the z term is widened first.
#[inline]
pub fn position_hash(x: i32, y: i32, z: i32) -> u64 {
    let mut h = (x.wrapping_mul(3129871)) as i64 ^ (z as i64).wrapping_mul(116129781) ^ (y as i64);
    h = h
        .wrapping_mul(h)
        .wrapping_mul(42317861)
        .wrapping_add(h.wrapping_mul(11));
    (h as u64) >> 16
}

/// The bedrock probability threshold at a y-level, as used by the game.
///
/// Returns `None` for y-levels that are not part of either gradient.
pub fn threshold(y: i32) -> Option<(Surface, f32)> {
    if (0..=5).contains(&y) {
        Some((Surface::Floor, (5 - y) as f32 / 5.0))
    } else if (122..=127).contains(&y) {
        let l = y - 122;
        Some((Surface::Roof, (5 - l) as f32 / 5.0))
    } else {
        None
    }
}

/// Whether there is bedrock at `(x, y, z)` for the given layer seeds.
#[inline]
pub fn is_bedrock(seeds: &LayerSeeds, x: i32, y: i32, z: i32) -> bool {
    let Some((surface, bound)) = threshold(y) else {
        // Outside the gradient: y=0 handled above, everything between the two
        // surfaces is never bedrock.
        return false;
    };
    is_bedrock_with_seed(seeds.for_surface(surface), surface, x, y, z, bound)
}

/// Hot path: the same test with the layer seed and threshold already resolved.
#[inline(always)]
pub fn is_bedrock_with_seed(
    layer_seed: u64,
    surface: Surface,
    x: i32,
    y: i32,
    z: i32,
    bound: f32,
) -> bool {
    let mixed = layer_seed ^ position_hash(x, y, z);
    let value = JavaRandom::new(mixed as i64).next_float();
    match surface {
        Surface::Floor => value < bound,
        Surface::Roof => value >= bound,
    }
}

/// A bedrock pattern compiled for repeated matching at different offsets.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// `(dx, dz, expected_bedrock)` relative to the pattern's origin.
    cells: Vec<(i32, i32, bool)>,
    pub y: i32,
    pub surface: Surface,
    bound: f32,
}

impl Pattern {
    pub fn from_grid(grid: &Grid, y: i32) -> Result<Pattern> {
        let Some((surface, bound)) = threshold(y) else {
            bail!(
                "y={y} is not a bedrock layer; use 0-4 for the floor or 122-127 for the roof"
            );
        };
        if bound <= 0.0 || bound >= 1.0 {
            bail!(
                "y={y} is always the same block ({}), so a pattern there constrains nothing — \
                 use y=1..4 or y=123..126",
                if (surface == Surface::Floor && bound >= 1.0)
                    || (surface == Surface::Roof && bound <= 0.0)
                {
                    "always bedrock"
                } else {
                    "never bedrock"
                }
            );
        }

        let mut cells: Vec<(i32, i32, bool)> = grid
            .known_cells()
            .map(|(dx, dz, c)| (dx, dz, c == Cell::Present))
            .collect();

        if cells.is_empty() {
            bail!("the pattern has no known cells");
        }

        // Test the rarest outcome first: at y=4 bedrock is 20% likely, so a
        // '#' rejects 80% of positions while a '.' rejects only 20%. Ordering
        // by rarity makes the common case exit after one test.
        let p_bedrock = match surface {
            Surface::Floor => bound,
            Surface::Roof => 1.0 - bound,
        };
        cells.sort_by(|a, b| {
            let pa = if a.2 { p_bedrock } else { 1.0 - p_bedrock };
            let pb = if b.2 { p_bedrock } else { 1.0 - p_bedrock };
            pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(Pattern {
            cells,
            y,
            surface,
            bound,
        })
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Probability a random position matches this whole pattern by chance.
    pub fn false_positive_rate(&self) -> f64 {
        let p_bedrock = match self.surface {
            Surface::Floor => self.bound as f64,
            Surface::Roof => 1.0 - self.bound as f64,
        };
        self.cells
            .iter()
            .map(|(_, _, want)| if *want { p_bedrock } else { 1.0 - p_bedrock })
            .product()
    }

    #[inline(always)]
    pub fn matches_at(&self, layer_seed: u64, ox: i32, oz: i32) -> bool {
        for (dx, dz, want) in &self.cells {
            let got = is_bedrock_with_seed(
                layer_seed,
                self.surface,
                ox + dx,
                self.y,
                oz + dz,
                self.bound,
            );
            if got != *want {
                return false;
            }
        }
        true
    }
}

/// Searches a bounding box for every position where `pattern` matches.
pub fn search_area(
    pattern: &Pattern,
    layer_seed: u64,
    area: BBox,
    scanned: &AtomicU64,
    cancel: &AtomicBool,
    limit: usize,
) -> Vec<(i32, i32)> {
    let (min_x, min_z, max_x, max_z) = (area.min_x, area.min_z, area.max_x, area.max_z);
    let rows: Vec<i32> = (min_z..=max_z).collect();
    let width = (max_x as i64 - min_x as i64 + 1).max(0) as u64;

    rows.into_par_iter()
        .map(|z| {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            let mut hits = Vec::new();
            for x in min_x..=max_x {
                if pattern.matches_at(layer_seed, x, z) {
                    hits.push((x, z));
                    if hits.len() >= limit {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            scanned.fetch_add(width, Ordering::Relaxed);
            hits
        })
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        })
}

/// Checks a candidate world seed against a set of bedrock observations.
pub fn seed_matches_observations(world_seed: i64, obs: &[BedrockObservation]) -> bool {
    let seeds = layer_seeds(world_seed);
    obs.iter()
        .all(|o| is_bedrock(&seeds, o.x, o.y, o.z) == o.is_bedrock)
}

/// Expected number of 48-bit structure seeds surviving a set of observations.
pub fn expected_survivors(obs: &[BedrockObservation]) -> f64 {
    let mut keep = 1.0f64;
    for o in obs {
        let Some((surface, bound)) = threshold(o.y) else {
            continue;
        };
        let p_bedrock = match surface {
            Surface::Floor => bound as f64,
            Surface::Roof => 1.0 - bound as f64,
        };
        keep *= if o.is_bedrock { p_bedrock } else { 1.0 - p_bedrock };
    }
    (1u64 << 48) as f64 * keep
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 1 — Nether Bedrock Toolkit");

    let choice = ui::select_str(
        "Which sub-mode?",
        &[
            "1a — Find coordinates from a bedrock pattern (seed known)",
            "1b — Crack the seed from bedrock observations (1.18+ only)",
        ],
    )?;

    if choice == 0 { run_locate(session) } else { run_crack(session) }
}

fn run_locate(session: &mut Session) -> Result<()> {
    ui::header("1a — Coordinates from a bedrock pattern");

    let version = ui::prompt_version(session)?;
    if !version.is_1_18_plus() {
        ui::warn(&format!(
            "Nether bedrock is not seed-dependent in {}.",
            version.label()
        ));
        ui::note(
            "Before 1.18 the pattern is generated from the chunk coordinates alone, so it does \
             locate you — but reproducing it needs the exact order in which the surface builder \
             consumes one shared per-chunk RNG, which this tool does not implement because we \
             could not verify that order from a primary source.",
        );
        ui::note(
            "Use JorianWoltjer/BedrockFinder or user32dll/bedrock_finder for pre-1.18 patterns; \
             they implement it and run on the GPU.",
        );
        return Ok(());
    }

    let seed = ui::prompt_seed(session)?;
    let seeds = layer_seeds(seed);

    ui::note("Which layer did you record? Bedrock is rarest — and so most informative — at y=4 (floor) and y=123 (roof).");
    let y: i32 = ui::input_default("Y level", 4)?;
    let Some((surface, _)) = threshold(y) else {
        bail!("y={y} is not a bedrock layer (use 0-5 for the floor or 122-127 for the roof)");
    };

    ui::note("Enter the pattern: '#' = bedrock, '.' = not bedrock, '?' = unknown.");
    ui::note("Rows run along +Z, columns along +X, top-left is the pattern origin.");
    let source = ui::select_str("Where is the pattern?", &["Type/paste it", "Load from a file"])?;
    let grid = if source == 0 {
        Grid::parse(&ui::read_block("Pattern:")?)?
    } else {
        let path: String = ui::input("File path")?;
        Grid::from_file(&path)?
    };

    let pattern = Pattern::from_grid(&grid, y)?;
    println!();
    ui::note(&format!(
        "{} known cells on the {} at y={y}.",
        pattern.len(),
        surface.label()
    ));

    let fpr = pattern.false_positive_rate();
    let border = version.world_border() as f64;
    let expected_full_world = fpr * (2.0 * border) * (2.0 * border);
    ui::note(&format!(
        "Chance a random position matches: 1 in {:.0}. Across the whole world that is about \
         {expected_full_world:.0} false positive(s).",
        1.0 / fpr
    ));
    if expected_full_world > 5.0 {
        ui::warn("Record more cells if you want a unique answer.");
    }

    let bbox = ui::prompt_bbox(session, "bedrock search")?;
    let total = bbox.area() as u64;

    // Measure before committing: a full world border scan is ~3.6e15 positions,
    // which is weeks of CPU time, and the user deserves to know that up front
    // rather than discovering it from a stalled progress bar.
    let rate = benchmark_locate(&pattern, seeds.for_surface(surface));
    let est = total as f64 / rate;
    println!();
    ui::note(&format!(
        "Measured {:.1} million positions/second across {} threads.",
        rate / 1e6,
        rayon::current_num_threads()
    ));
    ui::warn(&format!(
        "Scanning {} positions will take about {}.",
        total,
        ui::humanize_duration(est)
    ));
    if est > 300.0 {
        ui::note(
            "The whole world border is ~3.6e15 positions — weeks of CPU time. Narrow the box \
             with mode 11 (a nether coordinate from a screenshot) or mode 6 first.",
        );
        if !ui::confirm("Start the scan anyway?", false)? {
            return Ok(());
        }
    }

    let limit: usize = ui::input_default("Stop after this many hits", 32usize)?;
    let scanned = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);
    let pb = ui::progress_bar(total, "scanning");

    let hits = std::thread::scope(|scope| {
        let (s, c, p) = (&scanned, &cancel, &pb);
        scope.spawn(move || {
            while !c.load(Ordering::Relaxed) {
                let done = s.load(Ordering::Relaxed);
                p.set_position(done.min(total));
                if done >= total {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        let hits = search_area(
            &pattern,
            seeds.for_surface(surface),
            bbox,
            &scanned,
            &cancel,
            limit,
        );
        cancel.store(true, Ordering::Relaxed);
        hits
    });
    pb.finish_and_clear();

    println!();
    if hits.is_empty() {
        ui::warn("No match in that area.");
        ui::note("Either the pattern is outside the box, or a cell was transcribed wrongly — mark anything uncertain as '?' rather than guessing.");
        return Ok(());
    }

    ui::success(&format!("{} match(es) — pattern origin in Nether coordinates:", hits.len()));
    for (x, z) in hits.iter().take(32) {
        let (ox, oz) = crate::portal::nether_to_overworld(*x, *z);
        println!("    nether {x}, {z}   ->  overworld about {ox}, {oz}");
    }
    if hits.len() > 32 {
        ui::note(&format!("… and {} more", hits.len() - 32));
    }

    if hits.len() == 1 && ui::confirm("Store the overworld equivalent as the session search box?", true)? {
        let (ox, oz) = crate::portal::nether_to_overworld(hits[0].0, hits[0].1);
        session.search_box = Some(BBox::around(ox, oz, 128));
        ui::success("Stored.");
    }
    Ok(())
}

fn benchmark_locate(pattern: &Pattern, layer_seed: u64) -> f64 {
    let sample = 400_000i32;
    let start = std::time::Instant::now();
    let count = (0..sample)
        .into_par_iter()
        .filter(|i| pattern.matches_at(layer_seed, *i, 0))
        .count();
    std::hint::black_box(count);
    (sample as f64 / start.elapsed().as_secs_f64().max(1e-6)).max(1.0)
}

fn run_crack(session: &mut Session) -> Result<()> {
    ui::header("1b — Seed crack from bedrock observations");

    let version = ui::prompt_version(session)?;
    if !version.is_1_18_plus() {
        ui::warn(&format!(
            "Nether bedrock only became seed-dependent in 1.18; {} carries no seed information \
             in its bedrock at all.",
            version.label()
        ));
        ui::note("Sub-mode 1b is unavailable for this version. Use 1a to locate yourself instead.");
        return Ok(());
    }

    collect_bedrock(session)?;
    let obs = session.bedrock.clone();

    let floor = obs.iter().filter(|o| Surface::of(o.y) == Surface::Floor).count();
    let roof = obs.len() - floor;
    println!();
    ui::note(&format!("{} observation(s): {floor} on the floor, {roof} on the roof.", obs.len()));
    if floor == 0 || roof == 0 {
        ui::warn(
            "Collecting from BOTH the floor and the roof cuts false positives dramatically — \
             the two surfaces use independent layer seeds.",
        );
    }

    let survivors = expected_survivors(&obs);
    ui::note(&format!(
        "Expected surviving structure seeds across the whole space: about {survivors:.1}."
    ));
    if survivors > 1000.0 {
        ui::warn("That is not enough to isolate a seed — collect more blocks, especially at y=4 and y=123.");
    }

    // A full 2^48 sweep is days of CPU time here, so this mode is built as a
    // filter that composes with the cheaper constraints rather than pretending
    // to be a standalone cracker. See the note printed below.
    let choices = if session.candidates.is_empty() {
        vec!["Scan a range of the seed space".to_string()]
    } else {
        vec![
            format!("Filter the {} candidate seed(s) in this session", session.candidates.len()),
            "Scan a range of the seed space".to_string(),
        ]
    };
    let has_candidates = !session.candidates.is_empty();
    let choice = ui::select("How should the search run?", &choices)?;

    let found = if has_candidates && choice == 0 {
        let hits: Vec<i64> = session
            .candidates
            .par_iter()
            .copied()
            .filter(|s| seed_matches_observations(*s, &obs))
            .collect();
        ui::success(&format!(
            "{} of {} candidates match the bedrock.",
            hits.len(),
            session.candidates.len()
        ));
        hits
    } else {
        ui::note(
            "A from-scratch 2^48 bedrock crack needs the layered filter tree from \
             19MisterX98/Nether_Bedrock_Cracker; this tool implements exact verification and a \
             ranged brute force instead, which is what composes with mode 9's pillar shortcut.",
        );
        let start: u64 = ui::input_default("Start of range", 0u64)?;
        let end: u64 = ui::input_default("End of range (exclusive)", (start + (1 << 32)).min(1 << 48))?;
        if end <= start || end > (1 << 48) {
            bail!("range must satisfy 0 <= start < end <= 2^48");
        }

        let total = end - start;
        let t0 = std::time::Instant::now();
        let probe = 200_000u64;
        let c = (start..start + probe.min(total))
            .into_par_iter()
            .filter(|s| seed_matches_observations(*s as i64, &obs))
            .count();
        std::hint::black_box(c);
        let rate = (probe as f64 / t0.elapsed().as_secs_f64().max(1e-6)).max(1.0);
        ui::warn(&format!(
            "Scanning {total} seeds will take about {}.",
            ui::humanize_duration(total as f64 / rate)
        ));
        if !ui::confirm("Start?", false)? {
            return Ok(());
        }

        let pb = ui::progress_bar(total, "cracking");
        let hits: Vec<i64> = (start..end)
            .into_par_iter()
            .filter(|s| seed_matches_observations(*s as i64, &obs))
            .map(|s| s as i64)
            .collect();
        pb.finish_and_clear();
        hits
    };

    println!();
    if found.is_empty() {
        ui::warn("Nothing matched. If you scanned a range, the seed is probably outside it; if you filtered candidates, one of the observations may be wrong.");
        return Ok(());
    }
    ui::success(&format!("{} matching seed(s):", found.len()));
    for s in found.iter().take(32) {
        println!("    {s}");
    }
    if ui::confirm("Store as session candidates?", true)? {
        session.candidates = found.clone();
    }
    if found.len() == 1 && ui::confirm("Set as the session seed?", true)? {
        session.seed = Some(found[0]);
    }
    Ok(())
}

fn collect_bedrock(session: &mut Session) -> Result<()> {
    if !session.bedrock.is_empty()
        && ui::confirm(
            &format!("Reuse the {} bedrock observation(s) in this session?", session.bedrock.len()),
            true,
        )?
    {
        return Ok(());
    }

    ui::note("Enter one observation per line as: X Y Z B    (B = 1/# for bedrock, 0/. for not bedrock)");
    ui::note("Prioritise y=4 and y=123 — bedrock is rarest there and each block tells you the most.");
    let lines = ui::read_block("Observations:")?;

    let mut fresh = Vec::new();
    for line in lines {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 4 {
            ui::warn(&format!("skipping (need 4 fields): {line:?}"));
            continue;
        }
        let (Ok(x), Ok(y), Ok(z)) = (
            toks[0].parse::<i32>(),
            toks[1].parse::<i32>(),
            toks[2].parse::<i32>(),
        ) else {
            ui::warn(&format!("skipping unparseable coordinates: {line:?}"));
            continue;
        };
        let is_bedrock = matches!(toks[3], "1" | "#" | "b" | "B" | "true" | "yes");
        if threshold(y).is_none() {
            ui::warn(&format!("skipping y={y}: not a bedrock layer"));
            continue;
        }
        fresh.push(BedrockObservation { x, y, z, is_bedrock });
    }

    if fresh.is_empty() {
        bail!("no usable observations were entered");
    }
    session.bedrock = fresh;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::java_string_hash;

    /// Test vectors published in 19MisterX98/Nether_Bedrock_Cracker
    /// (`bedrock_cracker/src/layer.rs`, `test_world_seed_to_roof`).
    const WORLD_SEED: i64 = 765906787396911863;
    const ROOF_SEED: u64 = 191924403737289;
    const FLOOR_SEED: u64 = 18240473916414;

    #[test]
    fn layer_seeds_match_the_reference_vectors() {
        let s = layer_seeds(WORLD_SEED);
        assert_eq!(s.roof, ROOF_SEED, "roof layer seed");
        assert_eq!(s.floor, FLOOR_SEED, "floor layer seed");
    }

    #[test]
    fn namespace_hashes_are_right() {
        assert_eq!(java_string_hash("minecraft:bedrock_floor") as i64, FLOOR_HASH);
        assert_eq!(java_string_hash("minecraft:bedrock_roof") as i64, ROOF_HASH);
    }

    #[test]
    fn position_hash_matches_the_reference() {
        // From the reference cracker's `test_hashcode`: block (-98, 4, -469)
        // hashes to 99261249361405.
        assert_eq!(position_hash(-98, 4, -469), 99261249361405);
    }

    #[test]
    fn reference_blocks_are_classified_correctly() {
        // The 24 blocks the reference cracker uses as its fixture, for the
        // same world seed. If our threshold direction or surface split were
        // backwards, these would flip wholesale.
        let seeds = layer_seeds(WORLD_SEED);
        let expected: &[(i32, i32, i32, bool)] = &[
            (18, 123, -117, false),
            (18, 123, -118, false),
            (18, 123, -119, false),
            (33, 126, -99, false),
            (35, 126, -99, false),
            (38, 126, -99, false),
            (19, 123, -117, true),
            (19, 123, -118, true),
            (19, 123, -119, true),
            (25, 126, -112, true),
            (25, 126, -113, true),
            (25, 126, -114, true),
            (11, 1, -111, false),
            (11, 1, -110, false),
            (11, 1, -109, false),
            (14, 4, -97, false),
            (14, 4, -96, false),
            (14, 4, -94, false),
            (10, 1, -111, true),
            (10, 1, -110, true),
            (10, 1, -109, true),
            (11, 4, -97, true),
            (11, 4, -96, true),
            (11, 4, -94, true),
        ];
        for (x, y, z, want) in expected {
            assert_eq!(
                is_bedrock(&seeds, *x, *y, *z),
                *want,
                "block ({x}, {y}, {z}) misclassified"
            );
        }
    }

    #[test]
    fn the_gradient_endpoints_are_absolute() {
        let seeds = layer_seeds(WORLD_SEED);
        for x in -40..40 {
            for z in -40..40 {
                assert!(is_bedrock(&seeds, x, 0, z), "y=0 must always be bedrock");
                assert!(is_bedrock(&seeds, x, 127, z), "y=127 must always be bedrock");
                assert!(!is_bedrock(&seeds, x, 5, z), "y=5 is never bedrock");
                assert!(!is_bedrock(&seeds, x, 122, z), "y=122 is never bedrock");
                assert!(!is_bedrock(&seeds, x, 64, z), "mid-nether is never bedrock");
            }
        }
    }

    #[test]
    fn rare_layers_are_about_twenty_percent_bedrock() {
        let seeds = layer_seeds(WORLD_SEED);
        for (y, label) in [(4, "floor y=4"), (123, "roof y=123")] {
            let mut hits = 0;
            let n = 200;
            for x in 0..n {
                for z in 0..n {
                    if is_bedrock(&seeds, x, y, z) {
                        hits += 1;
                    }
                }
            }
            let rate = hits as f64 / (n * n) as f64;
            assert!(
                (0.18..0.22).contains(&rate),
                "{label} bedrock rate was {rate}, expected about 0.2"
            );
        }
    }

    #[test]
    fn a_pattern_taken_from_the_world_is_found_where_it_came_from() {
        // End-to-end: lift a real 6x6 patch out of the generator, then search
        // a window around it and confirm the origin comes back.
        let seeds = layer_seeds(WORLD_SEED);
        let (ox, oz, y) = (1234, -567, 4);
        let mut lines = Vec::new();
        for dz in 0..6 {
            let row: String = (0..6)
                .map(|dx| if is_bedrock(&seeds, ox + dx, y, oz + dz) { '#' } else { '.' })
                .collect();
            lines.push(row);
        }
        let grid = Grid::parse(&lines).unwrap();
        let pattern = Pattern::from_grid(&grid, y).unwrap();
        assert_eq!(pattern.len(), 36);
        assert!(pattern.matches_at(seeds.floor, ox, oz));

        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let hits = search_area(
            &pattern,
            seeds.floor,
            BBox::around_rect(ox - 60, oz - 60, ox + 60, oz + 60),
            &scanned,
            &cancel,
            64,
        );
        assert!(hits.contains(&(ox, oz)), "did not rediscover the origin; got {hits:?}");
    }

    #[test]
    fn observations_verify_against_their_own_seed() {
        let seeds = layer_seeds(WORLD_SEED);
        let obs: Vec<BedrockObservation> = [(11, 4, -97), (14, 4, -96), (19, 123, -117), (18, 123, -118)]
            .iter()
            .map(|(x, y, z)| BedrockObservation {
                x: *x,
                y: *y,
                z: *z,
                is_bedrock: is_bedrock(&seeds, *x, *y, *z),
            })
            .collect();
        assert!(seed_matches_observations(WORLD_SEED, &obs));
        assert!(!seed_matches_observations(WORLD_SEED ^ 1, &obs));
    }

    #[test]
    fn surface_split_and_thresholds() {
        assert_eq!(Surface::of(4), Surface::Floor);
        assert_eq!(Surface::of(123), Surface::Roof);
        assert_eq!(Surface::of(63), Surface::Floor);
        assert_eq!(Surface::of(64), Surface::Roof);
        assert_eq!(threshold(0), Some((Surface::Floor, 1.0)));
        assert_eq!(threshold(4), Some((Surface::Floor, 0.2)));
        assert_eq!(threshold(123), Some((Surface::Roof, 0.8)));
        assert_eq!(threshold(127), Some((Surface::Roof, 0.0)));
        assert_eq!(threshold(60), None);
    }

    #[test]
    fn patterns_on_constant_layers_are_rejected() {
        let lines: Vec<String> = vec!["##".to_string(), "##".to_string()];
        let g = Grid::parse(&lines).unwrap();
        assert!(Pattern::from_grid(&g, 0).is_err(), "y=0 constrains nothing");
        assert!(Pattern::from_grid(&g, 127).is_err(), "y=127 constrains nothing");
        assert!(Pattern::from_grid(&g, 60).is_err(), "y=60 is not a bedrock layer");
        assert!(Pattern::from_grid(&g, 4).is_ok());
    }

    #[test]
    fn rarest_cells_are_tested_first() {
        // At y=4 bedrock is the 20% case, so '#' must sort ahead of '.'.
        let lines: Vec<String> = vec![".#".to_string()];
        let g = Grid::parse(&lines).unwrap();
        let p = Pattern::from_grid(&g, 4).unwrap();
        assert!(p.cells[0].2, "the bedrock cell should be tested first at y=4");

        // y=123 is the roof's rare layer too — the gradient runs the other way
        // up there, so bedrock is again the 20% case and again sorts first.
        let p = Pattern::from_grid(&g, 123).unwrap();
        assert!(p.cells[0].2, "the bedrock cell should be tested first at y=123");

        // y=126 is where the roof gradient has flipped: bedrock is the 80%
        // case, so now the empty cell is the rarer, more selective test.
        let p = Pattern::from_grid(&g, 126).unwrap();
        assert!(!p.cells[0].2, "the non-bedrock cell should be tested first at y=126");
    }

    #[test]
    fn false_positive_rate_tracks_pattern_size() {
        let small = Grid::parse(&["#".to_string()]).unwrap();
        let big = Grid::parse(&["####".to_string()]).unwrap();
        let ps = Pattern::from_grid(&small, 4).unwrap();
        let pb = Pattern::from_grid(&big, 4).unwrap();
        assert!((ps.false_positive_rate() - 0.2).abs() < 1e-6);
        assert!((pb.false_positive_rate() - 0.2f64.powi(4)).abs() < 1e-6);
    }
}
