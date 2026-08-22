//! Mode 9 — combining independent partial constraints into one seed.
//!
//! No single observation type pins down a seed cheaply. Slime chunks cost one
//! RNG draw each but only reject 90% of seeds per positive; bedrock is
//! stronger but still leaves a 2^48 sweep; structure positions are strong but
//! sparse. Combining them multiplies their power — which is how SeedCrackerX
//! works, and what this mode does.
//!
//! # The End pillar shortcut
//!
//! The 10 obsidian pillars are the cheapest 16 bits in the game. Their layout
//! comes from:
//!
//! ```java
//! long pillarSeed = new Random(worldSeed).nextLong() & 65535L;
//! List<Integer> order = IntStream.range(0, 10).boxed().collect(toList());
//! Collections.shuffle(order, new Random(pillarSeed));
//! ```
//!
//! Only 65,536 arrangements exist, so observing the pillars identifies the
//! pillar seed outright. That is not merely a filter: `pillarSeed` is the low
//! 16 bits of `nextLong()`, which is the low 16 bits of the generator's second
//! `next(32)` call — i.e. bits 16..31 of the LCG state two steps in. Because
//! the LCG is invertible, we can *enumerate* every structure seed consistent
//! with it by walking the 2^32 remaining state bits and stepping backwards,
//! rather than testing 2^48 seeds. See [`structure_seeds_for_pillar_seed`].
//!
//! Pillar geometry, from the Minecraft Wiki's "End spike" page: the ten
//! pillars stand on a circle of radius 42 at fixed, seed-independent
//! coordinates; the shuffle decides only *which* pillar gets which height,
//! radius and iron cage.

use anyhow::{Result, bail};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::bedrock;
use crate::random::{JavaRandom, MASK, MULTIPLIER, collections_shuffle};
use crate::session::{BedrockObservation, Session, SlimeObservation, StructureObservation};
use crate::slime;
use crate::ui;
use crate::worldgen::{STRUCTURES, Version, WorldGen, structure_label};

/// The ten pillar positions, in generation order.
///
/// The game computes these as
///
/// ```text
/// angle = 2.0 * (-PI + 0.3141592653589793 * i)
/// x = floor(42 * cos(angle))
/// z = floor(42 * sin(angle))
/// ```
///
/// and the exact form of that expression matters. Simplifying it to the
/// algebraically identical `PI * i / 5` changes the floating-point rounding:
/// at i=5 the tidied version makes `sin` return `+1.2e-16` instead of
/// `-1.2e-16`, so the floor comes out 0 rather than -1 and pillar 5 lands at
/// (-42, 0) instead of the correct (-42, -1). These match the ten
/// seed-independent coordinates the Minecraft Wiki lists.
pub const PILLAR_POSITIONS: [(i32, i32); 10] = [
    (42, 0),
    (33, 24),
    (12, 39),
    (-13, 39),
    (-34, 24),
    (-42, -1),
    (-34, -25),
    (-13, -40),
    (12, -40),
    (33, -25),
];

/// What one pillar looks like once the shuffle has assigned it an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pillar {
    pub x: i32,
    pub z: i32,
    /// Top of the pillar, y = 76..103 in steps of 3.
    pub height: i32,
    /// Obsidian radius, 3..6.
    pub radius: i32,
    /// Whether an iron bar cage sits on top.
    pub caged: bool,
}

/// The pillar layout implied by a pillar seed.
pub fn pillar_arrangement(pillar_seed: u16) -> [Pillar; 10] {
    let mut order: Vec<usize> = (0..10).collect();
    collections_shuffle(&mut order, &mut JavaRandom::new(pillar_seed as i64));

    std::array::from_fn(|i| {
        let idx = order[i] as i32;
        Pillar {
            x: PILLAR_POSITIONS[i].0,
            z: PILLAR_POSITIONS[i].1,
            // Wiki "End spike": heights run 76, 79 … 103 and radii 3..6, with
            // iron cages on the two pillars at y=79 and y=82.
            height: 76 + 3 * idx,
            radius: 3 + idx / 3,
            caged: idx == 1 || idx == 2,
        }
    })
}

/// The pillar seed a world seed produces.
pub fn pillar_seed_of(world_seed: i64) -> u16 {
    (JavaRandom::new(world_seed).next_long() & 0xFFFF) as u16
}

/// Every pillar seed consistent with the observed pillar heights.
///
/// `observed[i]` is the height of the pillar at `PILLAR_POSITIONS[i]`, or
/// `None` if it was not measured. Usually a handful of pillars is enough.
pub fn pillar_seeds_matching(observed: &[Option<i32>; 10]) -> Vec<u16> {
    if observed.iter().all(|o| o.is_none()) {
        return Vec::new();
    }
    (0..=u16::MAX)
        .filter(|ps| {
            let arrangement = pillar_arrangement(*ps);
            observed
                .iter()
                .zip(arrangement.iter())
                .all(|(want, got)| want.is_none_or(|h| h == got.height))
        })
        .collect()
}

/// Total number of structure seeds consistent with one pillar seed.
pub const PILLAR_CANDIDATE_COUNT: u64 = 1 << 32;

/// The structure seed implied by an LCG state two steps into `nextLong()`.
///
/// `new Random(seed)` scrambles to `state0`; `nextLong` then steps twice, and
/// the pillar seed reveals 16 bits of that second state. Stepping backwards
/// twice and unscrambling recovers the seed exactly — no search involved.
#[inline]
pub fn structure_seed_from_state2(state2: u64) -> i64 {
    let mut r = JavaRandom::from_state(state2);
    r.previous();
    r.previous();
    ((r.state() ^ MULTIPLIER) & MASK) as i64
}

/// Builds the LCG state whose bits 16..31 are the pillar seed.
#[inline]
pub fn state2_for(pillar_seed: u16, high: u16, low: u16) -> u64 {
    ((high as u64) << 32) | ((pillar_seed as u64) << 16) | (low as u64)
}

/// Every structure seed consistent with a pillar seed, as an indexed lookup.
///
/// `index` runs over `0..2^32`; the high and low 16 free bits are split out of
/// it. This is a bijection, so the 2^32 indices enumerate the candidate set
/// exactly once each.
#[inline]
pub fn structure_seeds_for_pillar_seed(pillar_seed: u16, index: u64) -> i64 {
    let high = (index >> 16) as u16;
    let low = (index & 0xFFFF) as u16;
    structure_seed_from_state2(state2_for(pillar_seed, high, low))
}

/// Everything we know, compiled into a single cheap test over structure seeds.
#[derive(Debug, Default)]
pub struct ConstraintSet {
    slime: Option<slime::SlimeConstraints>,
    bedrock: Vec<BedrockObservation>,
    /// `(region attempt position, tolerance)` per observed structure.
    structures: Vec<StructureConstraint>,
}

#[derive(Debug, Clone)]
struct StructureConstraint {
    region: cubiomes::structures::StructureRegion,
    x: i32,
    z: i32,
    tolerance: i32,
    label: &'static str,
}

impl ConstraintSet {
    pub fn build(
        session: &Session,
        version: Version,
        structure_tolerance: i32,
    ) -> Result<ConstraintSet> {
        let slime = if session.slime.is_empty() {
            None
        } else {
            Some(slime::SlimeConstraints::new(&session.slime)?)
        };

        let mut structures = Vec::new();
        for obs in &session.structures {
            let region = cubiomes::structures::StructureRegion::from_block_position(
                cubiomes::generator::BlockPosition::new(obs.x, obs.z),
                version.mc(),
                obs.structure,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "{} does not generate in {}: {e:?}",
                    structure_label(obs.structure),
                    version.label()
                )
            })?;
            structures.push(StructureConstraint {
                region,
                x: obs.x,
                z: obs.z,
                tolerance: structure_tolerance,
                label: structure_label(obs.structure),
            });
        }

        Ok(ConstraintSet {
            slime,
            bedrock: session.bedrock.clone(),
            structures,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.slime.is_none() && self.bedrock.is_empty() && self.structures.is_empty()
    }

    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(s) = &self.slime {
            out.push(s.describe());
        }
        if !self.bedrock.is_empty() {
            out.push(format!("{} bedrock observation(s)", self.bedrock.len()));
        }
        for s in &self.structures {
            out.push(format!("{} at ({}, {})", s.label, s.x, s.z));
        }
        out
    }

    /// Expected fraction of seeds that survive, for an up-front estimate.
    pub fn survival_fraction(&self) -> f64 {
        let mut keep = 1.0;
        if let Some(s) = &self.slime {
            keep *= s.expected_survivors() / slime::SEED_SPACE as f64;
        }
        if !self.bedrock.is_empty() {
            keep *= bedrock::expected_survivors(&self.bedrock) / (1u64 << 48) as f64;
        }
        for s in &self.structures {
            // A generation attempt lands anywhere in its region; the chance it
            // lands within `tolerance` of the observed spot is the ratio of
            // areas.
            let region = s.region.region_size_blocks() as f64;
            let win = (2.0 * s.tolerance as f64 + 1.0).min(region);
            keep *= (win * win) / (region * region);
        }
        keep
    }

    /// Per-constraint pass/fail for one seed, evaluating all of them.
    ///
    /// [`ConstraintSet::accepts`] short-circuits on the first failure, which is
    /// what makes the sweep fast but tells you nothing about *why* a candidate
    /// was kept or dropped. This runs everything so a near-miss can be shown
    /// as "142 of 147" with the three failures named — usually the fastest way
    /// to spot a mis-typed coordinate.
    pub fn explain(&self, structure_seed: i64) -> Vec<(String, bool)> {
        let mut out = Vec::new();

        if let Some(s) = &self.slime {
            out.extend(s.explain(structure_seed));
        }

        for c in &self.structures {
            let ok = match c.region.get_structure_generation_attempt(structure_seed) {
                Some(pos) => {
                    (pos.x - c.x).abs() <= c.tolerance && (pos.z - c.z).abs() <= c.tolerance
                }
                None => false,
            };
            out.push((format!("{} at ({}, {})", c.label, c.x, c.z), ok));
        }

        if !self.bedrock.is_empty() {
            let seeds = bedrock::layer_seeds(structure_seed);
            for o in &self.bedrock {
                let got = bedrock::is_bedrock(&seeds, o.x, o.y, o.z);
                out.push((
                    format!(
                        "({}, {}, {}) is {}bedrock",
                        o.x,
                        o.y,
                        o.z,
                        if o.is_bedrock { "" } else { "not " }
                    ),
                    got == o.is_bedrock,
                ));
            }
        }

        out
    }

    /// Tests a structure seed against everything, cheapest constraint first.
    #[inline]
    pub fn accepts(&self, structure_seed: i64) -> bool {
        // Slime is a couple of LCG steps; run it before anything heavier.
        if let Some(s) = &self.slime
            && !s.accepts(structure_seed)
        {
            return false;
        }
        for c in &self.structures {
            let Some(pos) = c.region.get_structure_generation_attempt(structure_seed) else {
                return false;
            };
            if (pos.x - c.x).abs() > c.tolerance || (pos.z - c.z).abs() > c.tolerance {
                return false;
            }
        }
        if !self.bedrock.is_empty() && !bedrock::seed_matches_observations(structure_seed, &self.bedrock)
        {
            return false;
        }
        true
    }
}

/// Sweeps the 2^32 seeds implied by a pillar seed, keeping those that satisfy
/// every constraint.
pub fn crack_with_pillars(
    pillar_seed: u16,
    constraints: &ConstraintSet,
    scanned: &AtomicU64,
    cancel: &AtomicBool,
    limit: usize,
) -> Vec<i64> {
    const CHUNK: u64 = 1 << 20;
    let blocks: Vec<u64> = (0..PILLAR_CANDIDATE_COUNT).step_by(CHUNK as usize).collect();

    blocks
        .into_par_iter()
        .map(|start| {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            let end = (start + CHUNK).min(PILLAR_CANDIDATE_COUNT);
            let mut hits = Vec::new();
            for index in start..end {
                let seed = structure_seeds_for_pillar_seed(pillar_seed, index);
                if constraints.accepts(seed) {
                    hits.push(seed);
                    if hits.len() >= limit {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            scanned.fetch_add(end - start, Ordering::Relaxed);
            hits
        })
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        })
}

/// Recovers the full 64-bit world seed from a structure seed using biome data.
///
/// The low 48 bits fix every structure, but biome generation also uses the top
/// 16, so only 65,536 candidates need checking. Each thread reuses one
/// generator via `apply_seed` rather than building 65,536 of them.
pub fn recover_upper_bits(
    version: Version,
    structure_seed: i64,
    biome_obs: &[(i32, i32, cubiomes::enums::BiomeID)],
    sample_y: i32,
) -> Vec<i64> {
    if biome_obs.is_empty() {
        return Vec::new();
    }
    let low = (structure_seed as u64) & MASK;

    (0u64..=0xFFFF)
        .into_par_iter()
        .map_init(
            || WorldGen::overworld(version, structure_seed),
            |world, high| {
                let candidate = ((high << 48) | low) as i64;
                world.apply_seed(candidate);
                let ok = biome_obs
                    .iter()
                    .all(|(x, z, want)| world.biome_at(*x, sample_y, *z).is_ok_and(|b| b == *want));
                if ok { Some(candidate) } else { None }
            },
        )
        .flatten()
        .collect()
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 9 — Multi-Source Seed Cracker");
    ui::note(
        "Feed in whatever you have. Each source is weak alone; together they collapse the space.",
    );

    let version = ui::prompt_version(session)?;

    // Gather whatever the user has, reusing anything earlier modes left behind.
    gather_pillars(session)?;
    gather_structures(session, version)?;
    gather_slime(session)?;
    gather_bedrock(session)?;

    println!();
    let tolerance: i32 = ui::input_default("Structure position tolerance (blocks)", 16)?;
    let constraints = ConstraintSet::build(session, version, tolerance)?;

    ui::note("Constraints in play:");
    if constraints.is_empty() {
        ui::warn("  (none)");
    }
    for line in constraints.describe() {
        println!("    - {line}");
    }

    let pillar_seeds = match &session.pillar_heights {
        Some(h) => pillar_seeds_matching(h),
        None => Vec::new(),
    };

    if pillar_seeds.is_empty() && constraints.is_empty() {
        bail!(
            "nothing to work with — record End pillar heights, or a handful of structures \
             (desert pyramids, igloos, swamp huts, villages) to crack by lifting, or slime \
             chunks or bedrock to narrow an existing candidate list"
        );
    }

    let found = if pillar_seeds.is_empty() {
        ui::note("No End pillar data, so there is no 16-bit shortcut available.");

        // Structures alone are still enough, via bit-lifting: a structure's
        // in-region offset modulo a power of two is fixed by the low bits of
        // the seed, so a cheap sieve replaces the pillar shortcut entirely.
        let lift_obs = crate::lifting::observations_from_session(version, &session.structures);
        let sieve = match &lift_obs {
            Ok(obs) if !obs.is_empty() => crate::lifting::Sieve::new(obs).ok(),
            _ => None,
        };

        match sieve {
            Some(sieve) => run_lifting(session, &lift_obs.expect("checked above"), &sieve)?,
            None => {
                ui::warn("Nor enough liftable structures to crack from structures alone.");
                ui::note(
                    "Either record the End pillar heights, or note a few more desert pyramids, \
                     igloos, swamp huts or villages — those leak low seed bits and let mode 9 \
                     crack with no End trip at all.",
                );
                if session.candidates.is_empty() {
                    return Ok(());
                }
                if !ui::confirm(
                    &format!(
                        "Filter the {} existing candidate seed(s) instead?",
                        session.candidates.len()
                    ),
                    true,
                )? {
                    return Ok(());
                }
                session
                    .candidates
                    .par_iter()
                    .copied()
                    .filter(|s| constraints.accepts(*s))
                    .collect()
            }
        }
    } else {
        ui::success(&format!(
            "{} pillar seed(s) match the observed arrangement.",
            pillar_seeds.len()
        ));
        if pillar_seeds.len() > 16 {
            ui::warn("Measure more pillars to narrow this — each one cuts the work.");
        }

        let expected =
            pillar_seeds.len() as f64 * PILLAR_CANDIDATE_COUNT as f64 * constraints.survival_fraction();
        ui::note(&format!(
            "Sweeping {} x 2^32 candidates; about {expected:.1} expected to survive.",
            pillar_seeds.len()
        ));

        let total = pillar_seeds.len() as u64 * PILLAR_CANDIDATE_COUNT;
        let rate = benchmark(&constraints, pillar_seeds[0]);
        ui::note(&format!(
            "Measured {:.1} million candidates/second across {} threads.",
            rate / 1e6,
            rayon::current_num_threads()
        ));
        ui::warn(&format!(
            "Estimated run time: {}.",
            ui::humanize_duration(total as f64 / rate)
        ));
        if !ui::confirm("Start?", true)? {
            return Ok(());
        }

        let limit: usize = ui::input_default("Stop after this many hits", 32usize)?;
        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let pb = ui::progress_bar(total, "cracking");

        let mut all = Vec::new();
        for ps in &pillar_seeds {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let hits = std::thread::scope(|scope| {
                let (s, c, p) = (&scanned, &cancel, &pb);
                scope.spawn(move || {
                    while !c.load(Ordering::Relaxed) {
                        p.set_position(s.load(Ordering::Relaxed).min(total));
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                });
                let hits = crack_with_pillars(*ps, &constraints, &scanned, &cancel, limit);
                cancel.store(true, Ordering::Relaxed);
                hits
            });
            cancel.store(false, Ordering::Relaxed);
            all.extend(hits);
            if all.len() >= limit {
                break;
            }
        }
        pb.finish_and_clear();
        all
    };

    report(session, version, found)
}

/// Drives the bit-lifting path: sieve the low bits, then sweep the high ones.
fn run_lifting(
    session: &mut Session,
    observations: &[crate::lifting::Observation],
    sieve: &crate::lifting::Sieve,
) -> Result<Vec<i64>> {
    ui::success(&format!(
        "{} of {} structures can drive a low-bit sieve.",
        sieve.liftable_count(),
        observations.len()
    ));
    ui::note(&format!(
        "Sieving the low {} bits of the seed; those structures pin down about {:.0} bits.",
        sieve.bits,
        sieve.information_bits()
    ));
    if sieve.information_bits() < sieve.bits as f64 {
        ui::warn(
            "That is less information than the sieve width, so expect many survivors and a long \
             sweep. More structures will fix it.",
        );
    }

    let pb = ui::spinner("sieving low bits");
    let survivors = sieve.survivors();
    pb.finish_and_clear();

    if survivors.is_empty() {
        bail!(
            "no low-bit pattern fits those structures — one of the positions is wrong. Lifting \
             has no tolerance to spend: each must be the structure's exact origin chunk."
        );
    }
    ui::success(&format!("{} low-bit candidate(s) survived.", survivors.len()));

    let total = sieve.sweep_size(survivors.len());
    let rate = {
        let probe = 500_000u64;
        let t0 = std::time::Instant::now();
        let n = (0..probe)
            .into_par_iter()
            .filter(|i| crate::lifting::seed_matches(*i as i64, observations))
            .count();
        std::hint::black_box(n);
        (probe as f64 / t0.elapsed().as_secs_f64().max(1e-6)).max(1.0)
    };
    ui::note(&format!(
        "Sweeping {total} candidates at about {:.1} million/second.",
        rate / 1e6
    ));
    ui::warn(&format!(
        "Estimated run time: {}.",
        ui::humanize_duration(total as f64 / rate)
    ));
    if !ui::confirm("Start?", true)? {
        return Ok(Vec::new());
    }

    let limit: usize = ui::input_default("Stop after this many hits", 32usize)?;
    let scanned = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);
    let pb = ui::progress_bar(total, "lifting");

    let hits = std::thread::scope(|scope| {
        let (s, c, p) = (&scanned, &cancel, &pb);
        scope.spawn(move || {
            while !c.load(Ordering::Relaxed) {
                p.set_position(s.load(Ordering::Relaxed).min(total));
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
        let hits = sieve.crack(&survivors, &scanned, &cancel, limit);
        cancel.store(true, Ordering::Relaxed);
        hits
    });
    pb.finish_and_clear();

    let _ = session;
    Ok(hits)
}

fn benchmark(constraints: &ConstraintSet, pillar_seed: u16) -> f64 {
    let sample = 2_000_000u64;
    let t0 = std::time::Instant::now();
    let n = (0..sample)
        .into_par_iter()
        .filter(|i| constraints.accepts(structure_seeds_for_pillar_seed(pillar_seed, *i)))
        .count();
    std::hint::black_box(n);
    (sample as f64 / t0.elapsed().as_secs_f64().max(1e-6)).max(1.0)
}

fn gather_pillars(session: &mut Session) -> Result<()> {
    if session.pillar_heights.is_some()
        && ui::confirm("Reuse the End pillar heights already recorded?", true)?
    {
        return Ok(());
    }
    if !ui::confirm("Do you have End pillar data? (the biggest single shortcut)", true)? {
        return Ok(());
    }

    ui::note("Stand at the central End island. The ten pillars sit on a circle of radius 42.");
    ui::note("Give the height (top Y) of each, in this order, or '?' to skip one:");
    for (i, (x, z)) in PILLAR_POSITIONS.iter().enumerate() {
        ui::note(&format!("    {i}: at x={x}, z={z}"));
    }
    ui::note("Valid heights are 76, 79, 82, 85, 88, 91, 94, 97, 100, 103.");

    let line = ui::input_optional("Heights (space separated, 10 values):")?;
    if line.trim().is_empty() {
        return Ok(());
    }

    let mut heights: [Option<i32>; 10] = [None; 10];
    for (i, tok) in line.split_whitespace().take(10).enumerate() {
        if tok == "?" || tok == "-" {
            continue;
        }
        match tok.parse::<i32>() {
            Ok(h) if (76..=103).contains(&h) && (h - 76) % 3 == 0 => heights[i] = Some(h),
            Ok(h) => ui::warn(&format!("{h} is not a valid pillar height; skipping pillar {i}")),
            Err(_) => ui::warn(&format!("could not read {tok:?}; skipping pillar {i}")),
        }
    }

    let known = heights.iter().filter(|h| h.is_some()).count();
    if known == 0 {
        ui::warn("No usable heights.");
        return Ok(());
    }

    // Heights are a permutation, so a repeat means a misreading.
    let mut seen = Vec::new();
    for h in heights.iter().flatten() {
        if seen.contains(h) {
            bail!("height {h} was given twice — the ten pillars all have different heights");
        }
        seen.push(*h);
    }

    ui::success(&format!("{known} pillar height(s) recorded."));
    session.pillar_heights = Some(heights);
    Ok(())
}

fn gather_structures(session: &mut Session, version: Version) -> Result<()> {
    if !session.structures.is_empty()
        && ui::confirm(
            &format!("Reuse the {} structure observation(s)?", session.structures.len()),
            true,
        )?
    {
        return Ok(());
    }
    if !ui::confirm("Add observed structure positions?", true)? {
        return Ok(());
    }

    let mut fresh = Vec::new();
    loop {
        let labels: Vec<String> = STRUCTURES
            .iter()
            .map(|(_, name, _)| name.to_string())
            .chain(std::iter::once("(done)".to_string()))
            .collect();
        let idx = ui::select("Structure type", &labels)?;
        if idx >= STRUCTURES.len() {
            break;
        }
        let (stype, name, _) = STRUCTURES[idx];
        let x: i32 = ui::input(&format!("{name} X"))?;
        let z: i32 = ui::input(&format!("{name} Z"))?;
        fresh.push(StructureObservation {
            structure: stype,
            x,
            z,
        });
        ui::success(&format!("{} observation(s) so far.", fresh.len()));
    }

    if !fresh.is_empty() {
        let _ = version;
        session.structures = fresh;
    }
    Ok(())
}

fn gather_slime(session: &mut Session) -> Result<()> {
    if !session.slime.is_empty() {
        ui::note(&format!(
            "Using the {} slime observation(s) already in this session.",
            session.slime.len()
        ));
        return Ok(());
    }
    if !ui::confirm("Add slime chunk observations?", false)? {
        return Ok(());
    }
    let lines = ui::read_block("Confirmed slime chunks, one 'chunkX chunkZ' per line:")?;
    let mut fresh = Vec::new();
    for line in lines {
        if let Some(p) = ui::parse_coords(&line) {
            fresh.push(SlimeObservation {
                chunk_x: p[0] as i32,
                chunk_z: p[1] as i32,
                is_slime: true,
            });
        }
    }
    session.slime = fresh;
    Ok(())
}

fn gather_bedrock(session: &mut Session) -> Result<()> {
    if !session.bedrock.is_empty() {
        ui::note(&format!(
            "Using the {} bedrock observation(s) already in this session.",
            session.bedrock.len()
        ));
    }
    Ok(())
}

fn report(session: &mut Session, version: Version, found: Vec<i64>) -> Result<()> {
    println!();
    if found.is_empty() {
        ui::warn("No seed satisfies every constraint.");
        ui::note("One observation is probably wrong — a mis-typed coordinate is the usual cause.");
        return Ok(());
    }

    ui::success(&format!("{} structure seed(s):", found.len()));
    for s in found.iter().take(32) {
        println!("    {s}");
    }
    if found.len() > 32 {
        ui::note(&format!("… and {} more", found.len() - 32));
    }

    session.candidates = found.clone();

    // A near miss is the common failure and the hardest to diagnose from a
    // bare list of numbers, so offer the per-constraint breakdown here rather
    // than making the user go and find mode 12.
    if ui::confirm("Show which constraints each result matched?", found.len() <= 8)? {
        let constraints = ConstraintSet::build(session, version, 16)?;
        for seed in found.iter().take(8) {
            let results = constraints.explain(*seed);
            let passed = results.iter().filter(|(_, ok)| *ok).count();
            println!();
            ui::note(&format!("{seed}: {passed}/{} matched", results.len()));
            for (label, ok) in results.iter().filter(|(_, ok)| !*ok) {
                println!(
                    "    {} {label}",
                    crate::theme::bad().apply_to(crate::theme::marks::BAD)
                );
                let _ = ok;
            }
        }
        println!();
        ui::note("Mode 12 explains any seed in full, and suggests what to observe next.");
    }

    println!();
    ui::note(
        "These are structure seeds — the low 48 bits. Structures, slime and bedrock all depend \
         only on those, so 65,536 world seeds share each one. Biome data separates them.",
    );

    if found.len() == 1
        && ui::confirm("Recover the full 64-bit world seed from biome observations?", true)?
    {
        let mut obs = Vec::new();
        ui::note("Enter observations as: X Z biome_name   (one per line)");
        for line in ui::read_block("Biome observations:")? {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 3 {
                ui::warn(&format!("skipping: {line:?}"));
                continue;
            }
            let (Ok(x), Ok(z)) = (toks[0].parse::<i32>(), toks[1].parse::<i32>()) else {
                ui::warn(&format!("skipping unparseable coordinates: {line:?}"));
                continue;
            };
            match crate::terrain::parse_biome(&toks[2..].join("_"), version) {
                Ok(b) => obs.push((x, z, b)),
                Err(e) => ui::warn(&format!("{e}")),
            }
        }

        if obs.is_empty() {
            ui::warn("No usable biome observations; leaving the top 16 bits unresolved.");
            return Ok(());
        }

        let sample_y: i32 = ui::input_default("Sample Y", 63)?;
        let pb = ui::spinner("checking 65,536 upper-bit candidates");
        let worlds = recover_upper_bits(version, found[0], &obs, sample_y);
        pb.finish_and_clear();

        if worlds.is_empty() {
            ui::warn("No world seed matched those biomes — check the coordinates and version.");
        } else {
            ui::success(&format!("{} world seed(s):", worlds.len()));
            for w in worlds.iter().take(16) {
                println!("    {w}");
            }
            if worlds.len() == 1 {
                session.seed = Some(worlds[0]);
                ui::success("Stored as the session seed.");
            } else {
                ui::note("Add more biome observations, further apart, to narrow this.");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pillar_positions_match_the_wiki() {
        // The wiki lists these ten coordinates as seed-independent. Our
        // generated set must be exactly that set.
        let mut ours = PILLAR_POSITIONS.to_vec();
        ours.sort();
        let mut wiki = vec![
            (-42, -1),
            (-34, -25),
            (-34, 24),
            (-13, -40),
            (-13, 39),
            (12, -40),
            (12, 39),
            (33, -25),
            (33, 24),
            (42, 0),
        ];
        wiki.sort();
        assert_eq!(ours, wiki);
    }

    #[test]
    fn pillar_positions_follow_the_radius_42_circle() {
        // Cross-check the coordinates against the formula they came from,
        // rather than trusting a transcribed table alone.
        for (i, (x, z)) in PILLAR_POSITIONS.iter().enumerate() {
            // The game's literal expression, not a tidied equivalent — see the
            // note on PILLAR_POSITIONS for why the difference is visible.
            let angle = 2.0 * (-std::f64::consts::PI + 0.3141592653589793 * i as f64);
            assert_eq!(*x, (42.0 * angle.cos()).floor() as i32, "pillar {i} x");
            assert_eq!(*z, (42.0 * angle.sin()).floor() as i32, "pillar {i} z");
        }

        // Guard the trap directly: the "simplified" angle disagrees at i=5.
        let tidied = (42.0 * (std::f64::consts::PI * 5.0 / 5.0).sin()).floor() as i32;
        assert_eq!(tidied, 0, "the tidied form rounds the other way");
        assert_eq!(PILLAR_POSITIONS[5].1, -1, "pillar 5 must keep the game's -1");
    }

    #[test]
    fn an_arrangement_is_always_a_permutation_of_the_ten_specs() {
        for ps in [0u16, 1, 4919, 30000, u16::MAX] {
            let a = pillar_arrangement(ps);
            let mut heights: Vec<i32> = a.iter().map(|p| p.height).collect();
            heights.sort();
            assert_eq!(heights, vec![76, 79, 82, 85, 88, 91, 94, 97, 100, 103]);

            // Radius and cage must track height exactly as the wiki table says.
            for p in &a {
                let idx = (p.height - 76) / 3;
                assert_eq!(p.radius, 3 + idx / 3, "radius at height {}", p.height);
                assert_eq!(p.caged, idx == 1 || idx == 2, "cage at height {}", p.height);
            }
            assert_eq!(a.iter().filter(|p| p.caged).count(), 2);
        }
    }

    #[test]
    fn observing_an_arrangement_recovers_its_pillar_seed() {
        for ps in [0u16, 7, 1234, 40000, u16::MAX] {
            let a = pillar_arrangement(ps);
            let observed: [Option<i32>; 10] = std::array::from_fn(|i| Some(a[i].height));
            let found = pillar_seeds_matching(&observed);
            assert!(found.contains(&ps), "pillar seed {ps} was not recovered");
        }
    }

    #[test]
    fn partial_pillar_observations_narrow_without_pinning() {
        let a = pillar_arrangement(1234);
        // Only four pillars measured: should still cut the space hard.
        let mut observed: [Option<i32>; 10] = [None; 10];
        for i in [0, 3, 6, 9] {
            observed[i] = Some(a[i].height);
        }
        let found = pillar_seeds_matching(&observed);
        assert!(found.contains(&1234));
        assert!(
            found.len() < 200,
            "four pillars should narrow 65536 a long way, got {}",
            found.len()
        );

        // No observations at all constrains nothing.
        assert!(pillar_seeds_matching(&[None; 10]).is_empty());
    }

    #[test]
    fn the_pillar_shortcut_enumerates_the_true_seed() {
        // The heart of mode 9: from a world seed, take its pillar seed, then
        // confirm the 2^32 enumeration really does contain the original
        // structure seed at the index the maths predicts.
        for world_seed in [1234i64, -99887766, 765906787396911863, 0] {
            let ps = pillar_seed_of(world_seed);

            // Work out the true state two steps into nextLong().
            let mut r = JavaRandom::new(world_seed);
            r.advance();
            let state2 = r.advance();

            // Its bits 16..31 must be exactly the pillar seed.
            assert_eq!(
                ((state2 >> 16) & 0xFFFF) as u16,
                ps,
                "pillar seed is not bits 16..31 of state2 for {world_seed}"
            );

            // Reconstructing from those bits must give the structure seed back.
            let high = ((state2 >> 32) & 0xFFFF) as u16;
            let low = (state2 & 0xFFFF) as u16;
            assert_eq!(state2_for(ps, high, low), state2);

            let index = ((high as u64) << 16) | low as u64;
            let recovered = structure_seeds_for_pillar_seed(ps, index);
            assert_eq!(
                recovered,
                (world_seed as u64 & MASK) as i64,
                "enumeration missed the structure seed for {world_seed}"
            );
        }
    }

    #[test]
    fn the_enumeration_is_a_bijection() {
        // Distinct indices must give distinct seeds, or the sweep would both
        // waste work and risk missing candidates.
        let ps = 4919u16;
        let mut seen = std::collections::HashSet::new();
        for index in 0..5000u64 {
            assert!(
                seen.insert(structure_seeds_for_pillar_seed(ps, index)),
                "index {index} collided"
            );
        }
        // And every produced seed really does have that pillar seed.
        for index in [0u64, 1, 12345, 999_999] {
            let seed = structure_seeds_for_pillar_seed(ps, index);
            assert_eq!(pillar_seed_of(seed), ps);
        }
    }

    #[test]
    fn constraints_accept_the_truth_and_reject_neighbours() {
        let version = Version::V1_21_1;
        let world_seed = 1234i64;
        let structure_seed = (world_seed as u64 & MASK) as i64;

        // Build observations straight out of the generator, the way a player
        // reading coordinates off their own world would.
        let mut world = WorldGen::overworld(version, world_seed);
        let villages = world
            .structures_in_box(cubiomes::enums::StructureType::Village, -2000, -2000, 2000, 2000)
            .unwrap();
        assert!(!villages.is_empty());

        let mut session = Session {
            version: Some(version),
            ..Default::default()
        };
        session.structures = villages
            .iter()
            .take(2)
            .map(|p| StructureObservation {
                structure: cubiomes::enums::StructureType::Village,
                x: p.x,
                z: p.z,
            })
            .collect();
        session.slime = (0..6)
            .map(|i| SlimeObservation {
                chunk_x: i,
                chunk_z: i * 3,
                is_slime: slime::is_slime_chunk(structure_seed, i, i * 3),
            })
            .collect();

        let cs = ConstraintSet::build(&session, version, 16).unwrap();
        assert!(!cs.is_empty());
        assert!(cs.accepts(structure_seed), "the true seed must pass");

        // Nearby seeds should not.
        let mut rejected = 0;
        for delta in 1..=20i64 {
            if !cs.accepts(structure_seed + delta) {
                rejected += 1;
            }
        }
        assert_eq!(rejected, 20, "neighbouring seeds should all fail");
        assert!(cs.survival_fraction() < 1e-6);
    }

    #[test]
    fn upper_bits_are_recovered_from_biome_data() {
        let version = Version::V1_21_1;
        let world_seed: i64 = 0x00A5_0000_1234_5678u64 as i64;
        let structure_seed = (world_seed as u64 & MASK) as i64;
        assert_ne!(world_seed, structure_seed, "test needs non-zero upper bits");

        let world = WorldGen::overworld(version, world_seed);
        let obs: Vec<(i32, i32, cubiomes::enums::BiomeID)> =
            [(0, 0), (1500, -900), (-2400, 3100), (700, 6200)]
                .iter()
                .map(|(x, z)| (*x, *z, world.biome_at(*x, 63, *z).unwrap()))
                .collect();

        let found = recover_upper_bits(version, structure_seed, &obs, 63);
        assert!(
            found.contains(&world_seed),
            "the true world seed was not recovered; got {} candidates",
            found.len()
        );
    }
}
