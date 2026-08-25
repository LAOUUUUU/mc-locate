//! Population- and decorator-seed maths, and cracking a world seed from a
//! decorator feature.
//!
//! Every feature a chunk decorates with — ore veins, plants, dungeons, geodes —
//! is seeded from the chunk's *population seed*, which in turn is a fixed
//! function of the world seed and the chunk's block coordinates. This is the
//! leak SeedCrackerX exploits, and it is entirely a *world*-data leak: the
//! server sends your client where a feature generated, and the maths run
//! backwards to the seed. Nothing here touches another player.
//!
//! The formulas are taken line-for-line from SeedFinding's canonical
//! `mc_core_java` `ChunkRand`:
//!
//! ```text
//! setPopulationSeed(worldSeed, blockX, blockZ):
//!     setSeed(worldSeed)
//!     a = nextLong() | 1        // 1.13+   (pre-1.13: nextLong()/2*2+1)
//!     b = nextLong() | 1
//!     seed = (blockX*a + blockZ*b) ^ worldSeed
//!     return seed & 2^48-1
//!
//! setDecoratorSeed(populationSeed, index, step):
//!     return (populationSeed + index + 10000*step) & 2^48-1
//! ```
//!
//! `blockX`/`blockZ` are the negative-most block of the chunk, i.e.
//! `chunkX * 16`. Because `setSeed` keeps only the low 48 bits, the population
//! seed depends on nothing above bit 47 of the world seed — so this recovers a
//! *structure seed* (the low 48 bits), exactly like the pillar and bedrock
//! paths, and the same [`crate::lifting`] lift turns it into full world seeds.

use crate::random::{JavaRandom, MASK};
use crate::worldgen::Version;
use cubiomes::enums::MCVersion;

/// Whether a version uses the 1.13+ `| 1` odd-ing rather than the older
/// `nextLong()/2*2+1`. The two agree for non-negative `nextLong` but differ in
/// the low bits when it is negative, so the distinction is load-bearing.
fn uses_or_one(version: Version) -> bool {
    version.at_least(MCVersion::MC_1_13)
}

/// The chunk population seed (its low 48 bits) for a structure seed.
///
/// `chunk_x`/`chunk_z` are chunk coordinates; the maths use `chunk * 16`
/// internally, matching Minecraft's negative-most-block convention.
pub fn population_seed(structure_seed: i64, chunk_x: i32, chunk_z: i32, version: Version) -> u64 {
    let block_x = (chunk_x as i64).wrapping_mul(16);
    let block_z = (chunk_z as i64).wrapping_mul(16);

    let mut r = JavaRandom::new(structure_seed);
    let (a, b) = if uses_or_one(version) {
        (r.next_long() | 1, r.next_long() | 1)
    } else {
        // Java `/` truncates toward zero, which is what mc_core_java relies on;
        // Rust's `/` on i64 does the same, so this is a direct transcription.
        (
            r.next_long() / 2 * 2 + 1,
            r.next_long() / 2 * 2 + 1,
        )
    };

    let seed = block_x
        .wrapping_mul(a)
        .wrapping_add(block_z.wrapping_mul(b))
        ^ structure_seed;
    (seed as u64) & MASK
}

/// The salt for a feature at `index` within generation `step`.
///
/// This is the only version/biome-specific quantity in the chain, and it is
/// deliberately a caller input rather than a hardcoded table: the index is a
/// feature's position in a biome's decoration list and shifts between versions,
/// so baking in a guess is exactly the kind of "looks about right" constant to
/// avoid. `salt = index + 10000 * step`.
pub fn decorator_salt(index: i32, step: i32) -> i32 {
    index + 10000 * step
}

/// The decorator seed (its low 48 bits) — the value `new Random(..)` is seeded
/// with to generate one feature — from a population seed and a salt.
pub fn decorator_seed(population_seed: u64, salt: i32) -> u64 {
    ((population_seed as i64).wrapping_add(salt as i64) as u64) & MASK
}

/// Recovers a population seed from a decorator seed and its salt.
///
/// The [`crate::reverser`] recovers the decorator seed (the `new Random`
/// argument) from a feature's draws; this undoes the salt to get back to the
/// population seed the whole chunk shares.
pub fn population_from_decorator(decorator_seed: u64, salt: i32) -> u64 {
    ((decorator_seed as i64).wrapping_sub(salt as i64) as u64) & MASK
}

/// Whether a candidate structure seed produces `observed` at this chunk.
///
/// This is the filter form, and it is what makes a decorator observation
/// compose with everything else: a population seed is 48 bits, so a single
/// feature at a known chunk collapses a candidate list (the 2^32 from the End
/// pillars, say) to essentially one survivor — all with the verified forward
/// function above, no inverse required.
pub fn matches_population_seed(
    candidate_structure_seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    observed: u64,
    version: Version,
) -> bool {
    population_seed(candidate_structure_seed, chunk_x, chunk_z, version) == (observed & MASK)
}

use anyhow::{Result, bail};

use crate::session::Session;
use crate::ui;

/// Mode 15 — crack (or narrow) a seed from a decorator feature.
///
/// The verified maths above turn a feature's RNG into the chunk's population
/// seed, a 48-bit value. That composes with any candidate list the session
/// already holds (the End pillars' 2^32, most usefully): a single feature at a
/// known chunk is a 48-bit filter, so it collapses the list to one seed.
pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 15 — Decorator / Population-Seed Crack");
    ui::note(
        "Every decorated feature (ores, plants, dungeons) is seeded from the chunk's \
         population seed — a function of the world seed the server leaks by placing the \
         feature. Recover that seed here and use it to pin down the world seed.",
    );

    let version = ui::prompt_version(session)?;

    let chunk_x: i32 = ui::input("Chunk X (block X >> 4)")?;
    let chunk_z: i32 = ui::input("Chunk Z (block Z >> 4)")?;

    let pop = match ui::select_str(
        "How do you have the feature seed?",
        &[
            "I know the population seed directly",
            "I have a decorator seed and its salt",
            "Recover it from a dungeon (spawner position + floor pattern)",
        ],
    )? {
        0 => ui::input::<i64>("Population seed")? as u64 & MASK,
        1 => {
            let dec: i64 = ui::input("Decorator seed (the new Random argument)")?;
            let salt = ask_salt()?;
            population_from_decorator(dec as u64 & MASK, salt)
        }
        _ => recover_from_dungeon()?,
    };

    ui::success(&format!("Population seed (48-bit): {pop}"));

    if session.candidates.is_empty() {
        ui::warn(
            "This session has no candidate seeds to filter. A population seed narrows an \
             existing list — read the End pillars (mode 9) first to get one, then run this \
             again; a single feature will collapse the 2^32 pillar candidates to one.",
        );
        return Ok(());
    }

    let before = session.candidates.len();
    let survivors: Vec<i64> = session
        .candidates
        .iter()
        .copied()
        .filter(|&s| matches_population_seed(s, chunk_x, chunk_z, pop, version))
        .collect();

    ui::success(&format!(
        "{} of {before} candidate seed(s) produce this feature at ({chunk_x}, {chunk_z}).",
        survivors.len()
    ));
    if survivors.is_empty() {
        ui::warn(
            "None matched. Check the chunk coordinates and the salt — a wrong salt shifts the \
             population seed and eliminates the true seed. The candidates were left unchanged.",
        );
        return Ok(());
    }
    for s in survivors.iter().take(10) {
        println!("    {s}");
    }
    if ui::confirm("Keep only these survivors as the session's candidates?", true)? {
        session.candidates = survivors;
    }
    Ok(())
}

/// Prompts for a feature salt, as `index + 10000 * step`.
fn ask_salt() -> Result<i32> {
    if ui::confirm("Enter the salt as index + step (rather than a raw number)?", true)? {
        let index: i32 = ui::input("Feature index (position in the biome's decoration list)")?;
        let step: i32 = ui::input("Generation step (0-9)")?;
        let salt = decorator_salt(index, step);
        ui::note(&format!("salt = {index} + 10000*{step} = {salt}"));
        Ok(salt)
    } else {
        Ok(ui::input("Raw salt")?)
    }
}

/// Recovers a population seed from an observed 1.15+ dungeon.
///
/// The draw order is nextInt(16) x, nextInt(16) z, nextInt(256) y, two size
/// rolls, then one nextInt(4) per floor block (0 = cobblestone, else mossy).
/// The lattice reverser recovers the decorator seed from that; the salt then
/// gives the population seed.
fn recover_from_dungeon() -> Result<u64> {
    use crate::reverser::Reverser;

    ui::note("Give the spawner's block position and the 7x7 floor pattern.");
    let sx: i32 = ui::input("Spawner block X")?;
    let _sy: i32 = ui::input("Spawner block Y")?;
    let sz: i32 = ui::input("Spawner block Z")?;
    let y: i32 = ui::input("Spawner block Y again, as the raw nextInt(256) value if known (else Y)")?;

    let ox = sx.rem_euclid(16) as u32;
    let oz = sz.rem_euclid(16) as u32;

    ui::note(
        "Floor: 49 characters row by row, 'c' = cobblestone, 'm' = mossy cobblestone. \
         Blocks you could not see: use '?'.",
    );
    let pattern: String = ui::input("Floor pattern")?;
    let cells: Vec<char> = pattern.chars().filter(|c| !c.is_whitespace()).collect();
    if cells.len() != 49 {
        bail!("expected 49 floor cells, got {}", cells.len());
    }

    let salt = ask_salt()?;

    let mut rev = Reverser::new();
    rev.next_int_eq(16, ox)?;
    rev.next_int_eq(16, oz)?;
    rev.next_int_eq(256, y as u32)?;
    rev.skip(2);
    let mut known = 0usize;
    for c in &cells {
        match c {
            'c' | 'C' => {
                rev.next_int_eq(4, 0)?;
                known += 1;
            }
            'm' | 'M' => {
                rev.next_int_ne(4, 0)?;
                known += 1;
            }
            _ => {
                // Unknown block: skip the draw without constraining it.
                rev.skip(1);
            }
        }
    }
    ui::note(&format!("{known} floor blocks constrain the search."));

    let nodes = rev.estimated_enumeration_nodes();
    if nodes > 5e7 {
        bail!(
            "this observation is too weak to solve quickly (~{nodes:.0e} nodes). Read more of the \
             floor — cobblestone blocks help most."
        );
    }
    ui::note("Solving the lattice — this can take a few seconds.");
    let decorator_seeds = rev.solve()?;
    if decorator_seeds.is_empty() {
        bail!("no decorator seed matched; re-check the spawner position and floor");
    }
    ui::success(&format!("Recovered {} decorator seed(s).", decorator_seeds.len()));

    // With a single decorator seed the population seed is unique; with several,
    // report the first and note the ambiguity.
    let dec = decorator_seeds[0] as u64 & MASK;
    Ok(population_from_decorator(dec, salt))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structure seed and any world seed sharing its low 48 bits must give the
    /// same population seed, since setSeed drops the high bits. This is the
    /// property that makes the crack recover a 48-bit structure seed.
    #[test]
    fn population_seed_ignores_the_high_16_bits() {
        let low: i64 = 0x0000_1234_5678_9ABC & (MASK as i64);
        let with_high: i64 = low | (0x7FFFi64 << 48);
        assert_eq!(
            population_seed(low, 3, -7, Version::V1_18_2),
            population_seed(with_high, 3, -7, Version::V1_18_2),
        );
    }

    /// Transcription check against the canonical formula, computed here by hand
    /// with the same primitives so the arithmetic — operator order, the `| 1`,
    /// the `^`, the mask — is pinned independently of the function under test.
    #[test]
    fn population_seed_matches_a_hand_computation() {
        let seed: i64 = 123_456_789;
        let (cx, cz) = (10, -4);

        let mut r = JavaRandom::new(seed);
        let a = r.next_long() | 1;
        let b = r.next_long() | 1;
        let expected = (((cx as i64 * 16).wrapping_mul(a)
            + (cz as i64 * 16).wrapping_mul(b))
            ^ seed) as u64
            & MASK;

        assert_eq!(population_seed(seed, cx, cz, Version::V1_18_2), expected);
    }

    #[test]
    fn decorator_seed_round_trips_through_the_salt() {
        let pop = population_seed(42, 1, 1, Version::V1_18_2);
        let salt = decorator_salt(3, 7); // 3 + 70000
        assert_eq!(salt, 70003);
        let dec = decorator_seed(pop, salt);
        assert_eq!(population_from_decorator(dec, salt), pop);
    }

    #[test]
    fn the_filter_accepts_the_true_seed_and_rejects_others() {
        let truth: i64 = 0x0000_ABCD_1234_5678 & (MASK as i64);
        let (cx, cz) = (-12, 30);
        let pop = population_seed(truth, cx, cz, Version::V1_18_2);

        assert!(matches_population_seed(truth, cx, cz, pop, Version::V1_18_2));
        // A different structure seed almost certainly misses a 48-bit target.
        assert!(!matches_population_seed(truth ^ 1, cx, cz, pop, Version::V1_18_2));
    }

    /// End-to-end through the real RNG and the lattice reverser: from a structure
    /// seed, build a dungeon's decorator draws exactly as the game would, recover
    /// the decorator seed, undo the salt, and confirm it lands on the population
    /// seed the forward function predicts. The reverser leg uses real JavaRandom
    /// output, so this is not circular — it independently confirms the decorator
    /// level and its salt arithmetic.
    #[test]
    #[ignore = "runs the ~seconds lattice solve; exercised explicitly"]
    fn decorator_chain_round_trips_via_the_reverser() {
        use crate::reverser::Reverser;

        let truth: i64 = 0x0000_1357_9BDF_2468 & (MASK as i64);
        let (cx, cz) = (5, 9);
        let salt = decorator_salt(9, 3);

        let pop = population_seed(truth, cx, cz, Version::V1_18_2);
        let dec = decorator_seed(pop, salt);

        // A dungeon draws: nextInt(16) x, nextInt(16) z, nextInt(256) y,
        // skip(2), then 49 nextInt(4) floor blocks — the query the reverser
        // is built to solve.
        let mut r = JavaRandom::new(dec as i64);
        let ox = r.next_int_bound(16) as u32;
        let oz = r.next_int_bound(16) as u32;
        let y = r.next_int_bound(256) as u32;
        r.next_int_bound(2);
        r.next_int_bound(2);
        let floor: Vec<i32> = (0..49).map(|_| r.next_int_bound(4)).collect();

        let mut rev = Reverser::new();
        rev.next_int_eq(16, ox).unwrap();
        rev.next_int_eq(16, oz).unwrap();
        rev.next_int_eq(256, y).unwrap();
        rev.skip(2);
        for v in &floor {
            if *v == 0 {
                rev.next_int_eq(4, 0).unwrap();
            } else {
                rev.next_int_ne(4, 0).unwrap();
            }
        }

        let found = rev.solve().unwrap();
        assert!(found.contains(&(dec as i64)), "reverser did not recover the decorator seed");
        assert_eq!(population_from_decorator(dec, salt), pop);
        assert!(matches_population_seed(truth, cx, cz, pop, Version::V1_18_2));
    }
}
