//! Structure-only seed cracking by bit-lifting.
//!
//! This is the path that recovers a structure seed from observed structure
//! positions alone — no End trip, no decorators. It is what SeedCrackerX calls
//! "lifting", and it is not a lattice method: it is an elementary sieve on the
//! low bits, of the Hensel-lifting kind.
//!
//! # Why the low bits leak
//!
//! A structure's in-region offset comes from (cubiomes `getFeatureChunkInRegion`,
//! which mirrors vanilla):
//!
//! ```text
//! seed  = structureSeed + regionX*341873128712 + regionZ*132897987541 + salt
//! state = (seed ^ 0x5DEECE66D)
//! state = (state * 0x5DEECE66D + 0xB) & (2^48 - 1)
//! offsetX = (state >> 17) % chunkRange          // non-power-of-two path
//! state = (state * 0x5DEECE66D + 0xB) & (2^48 - 1)
//! offsetZ = (state >> 17) % chunkRange
//! ```
//!
//! Now suppose `2^j` divides `chunkRange`. Then for any integer `v`,
//! `v % chunkRange ≡ v (mod 2^j)`, so
//!
//! ```text
//! offsetX mod 2^j  ==  (state >> 17) mod 2^j  ==  bits 17..17+j-1 of state
//! ```
//!
//! Multiplication and addition modulo `2^48` never let high bits influence low
//! ones, so bits `0..17+j-1` of `state` depend only on bits `0..17+j-1` of
//! `seed` — and hence only on the low `17 + j` bits of the structure seed. The
//! region and salt terms are known constants, so each observed structure pins
//! down `2j` bits of information about those low bits.
//!
//! So: sieve the `2^(17+j)` possible low-bit patterns against every
//! observation, then for each survivor sweep the remaining `2^(48-17-j)` high
//! bits with a full check. With five or so structures the sieve usually leaves
//! a single candidate and the sweep is ~2^29 — comparable to the End-pillar
//! route in [`crate::multicrack`], but requiring nothing but overworld
//! exploration.
//!
//! # What cannot take part
//!
//! * **Power-of-two `chunkRange`.** Java's `nextInt` takes a different branch
//!   there, `(range * (state >> 17)) >> 31`, which reads the *high* bits. Such
//!   structures are excluded from the sieve (they are still used in the full
//!   check).
//! * **Large structures** (ocean monuments, woodland mansions) average four
//!   `nextInt` calls and halve the result, so the clean `mod 2^j` identity does
//!   not survive. Same treatment: full check only.
//! * **`chunkRange` odd** (e.g. monuments at 27) gives `j = 0` and leaks
//!   nothing this way.
//!
//! All the placement constants come from cubiomes' per-version table via
//! [`crate::worldgen::structure_config`]; none are written down here.

use anyhow::{Result, bail};
use cubiomes::enums::StructureType;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::session::StructureObservation;
use crate::worldgen::{StructureConfig, Version, structure_config, structure_label};

const K: u64 = 0x5DEECE66D;
const B: u64 = 0xB;
const MASK: u64 = (1 << 48) - 1;

/// The two region-coordinate multipliers vanilla mixes into the seed.
const REGION_X_MULT: i64 = 341873128712;
const REGION_Z_MULT: i64 = 132897987541;

/// Where `nextInt`'s result starts in the LCG state: `next(31)` is
/// `state >> (48 - 31)`.
const NEXT_SHIFT: u32 = 17;

/// One observed structure, reduced to the region and in-region chunk offset
/// that actually constrain the seed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    pub structure: StructureType,
    pub region_x: i32,
    pub region_z: i32,
    pub offset_x: i32,
    pub offset_z: i32,
    pub config: StructureConfig,
    pub placement: Placement,
}

impl Observation {
    /// Reduces a block position to a region and in-region chunk offset.
    ///
    /// The position must be the structure's origin — the chunk-aligned corner
    /// cubiomes reports — because lifting has no tolerance to spend: one wrong
    /// chunk eliminates the true seed silently.
    pub fn from_block_position(
        version: Version,
        structure: StructureType,
        x: i32,
        z: i32,
    ) -> Result<Observation> {
        let Some(config) = structure_config(version, structure) else {
            bail!(
                "{} does not generate in {}",
                structure_label(structure),
                version.label()
            );
        };
        let Some(placement) = placement_of(structure, version) else {
            bail!(
                "{} is placed by a code path this cracker does not reproduce, so it cannot be \
                 used as a constraint",
                structure_label(structure)
            );
        };

        let chunk_x = x.div_euclid(16);
        let chunk_z = z.div_euclid(16);
        let region_x = chunk_x.div_euclid(config.region_size);
        let region_z = chunk_z.div_euclid(config.region_size);

        Ok(Observation {
            structure,
            region_x,
            region_z,
            offset_x: chunk_x - region_x * config.region_size,
            offset_z: chunk_z - region_z * config.region_size,
            config,
            placement,
        })
    }

    /// The constant part of the region seed: everything except the world seed.
    #[inline]
    pub fn region_constant(&self) -> i64 {
        (self.region_x as i64)
            .wrapping_mul(REGION_X_MULT)
            .wrapping_add((self.region_z as i64).wrapping_mul(REGION_Z_MULT))
            .wrapping_add(self.config.salt as i64)
    }

    /// Can this observation contribute to the low-bit sieve?
    /// Large structures average two draws per axis and shift right, so the
    /// clean `mod 2^j` identity the sieve relies on does not survive.
    pub fn is_liftable(&self) -> bool {
        self.placement == Placement::Feature
            && !self.config.is_power_of_two_range()
            && self.config.lift_valuation() > 0
    }
}

/// How a structure's in-region position is drawn.
///
/// Mirrors the dispatch in cubiomes' `getStructurePos`. Anything not listed
/// here takes a different path entirely — buried treasure is a `nextFloat`
/// rarity roll at a fixed offset, mineshafts have their own generator,
/// decorator-style features go through the population seed — so those are
/// refused rather than silently mis-modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// `getFeaturePos`: one `nextInt(chunkRange)` per axis.
    Feature,
    /// `getLargeStructurePos`: two draws per axis, averaged by a right shift.
    Large,
}

/// The placement family for a structure in a version, or `None` if this module
/// cannot reproduce it.
pub fn placement_of(structure: StructureType, version: Version) -> Option<Placement> {
    use StructureType::*;
    match structure {
        Feature | Desert_Pyramid | Jungle_Temple | Swamp_Hut | Igloo | Village | Ocean_Ruin
        | Shipwreck | Ruined_Portal | Ruined_Portal_N | Ancient_City | Trail_Ruins
        | Trial_Chambers => Some(Placement::Feature),

        // Outposts are feature-placed and then kept or discarded by a separate
        // `nextInt(5)` roll. That roll does not move the structure, so an
        // outpost you can actually see sits exactly at the feature position.
        Outpost => Some(Placement::Feature),

        // From 1.18 these are feature-placed with a biome-style check layered
        // on top; before that they used their own region logic.
        Fortress | Bastion if version.is_1_18_plus() => Some(Placement::Feature),

        Monument | Mansion | End_City => Some(Placement::Large),

        _ => None,
    }
}

/// Reproduces cubiomes' `getFeatureChunkInRegion` for a candidate seed.
///
/// Deliberately a direct port rather than an FFI call: the sweep runs this
/// billions of times, and the constants it needs already came from cubiomes.
/// [`tests::matches_cubiomes_structure_positions`] holds it to the real thing.
#[inline]
fn draw(state: &mut u64, range: u64) -> i32 {
    *state = state.wrapping_mul(K).wrapping_add(B) & MASK;
    if range & (range - 1) != 0 {
        ((*state >> NEXT_SHIFT) % range) as i32
    } else {
        // Java's power-of-two fast path reads the high bits instead.
        ((range.wrapping_mul(*state >> NEXT_SHIFT)) >> 31) as i32
    }
}

/// The in-region chunk offset a candidate seed produces for an observation.
#[inline]
pub fn feature_offset(structure_seed: i64, obs: &Observation) -> (i32, i32) {
    let range = obs.config.chunk_range as u64;
    let seed = (structure_seed.wrapping_add(obs.region_constant())) as u64;
    let mut state = (seed ^ K) & MASK;

    match obs.placement {
        Placement::Feature => (draw(&mut state, range), draw(&mut state, range)),
        Placement::Large => {
            // Two draws per axis, then halved — cubiomes'
            // `getLargeStructureChunkInRegion`. Note the draw order is
            // x, x, z, z, not x, z, x, z.
            let x = draw(&mut state, range) + draw(&mut state, range);
            let z = draw(&mut state, range) + draw(&mut state, range);
            (x >> 1, z >> 1)
        }
    }
}

/// Does this seed place every observed structure exactly where it was seen?
#[inline]
pub fn seed_matches(structure_seed: i64, observations: &[Observation]) -> bool {
    observations.iter().all(|o| {
        let (x, z) = feature_offset(structure_seed, o);
        x == o.offset_x && z == o.offset_z
    })
}

/// A prepared sieve over the low bits of the structure seed.
#[derive(Debug, Clone)]
pub struct Sieve {
    /// How many low bits of the seed the sieve fixes.
    pub bits: u32,
    /// `2^j`, the modulus each observation is checked against.
    pub modulus: u64,
    liftable: Vec<Observation>,
    all: Vec<Observation>,
}

impl Sieve {
    pub fn new(observations: &[Observation]) -> Result<Sieve> {
        if observations.is_empty() {
            bail!("no structure observations were given");
        }

        let liftable: Vec<Observation> =
            observations.iter().copied().filter(|o| o.is_liftable()).collect();

        if liftable.is_empty() {
            bail!(
                "none of those structures can drive a low-bit sieve — they all have a \
                 power-of-two or odd chunk range, or are large structures. Add a desert \
                 pyramid, igloo, swamp hut or village."
            );
        }

        // The sieve can only fix as many bits as the *weakest* observation
        // supports; going further would test bits that observation does not
        // actually determine.
        let j = liftable
            .iter()
            .map(|o| o.config.lift_valuation())
            .min()
            .unwrap_or(0);
        if j == 0 {
            bail!("the structures given leak no low bits");
        }

        Ok(Sieve {
            bits: NEXT_SHIFT + j,
            modulus: 1u64 << j,
            liftable,
            all: observations.to_vec(),
        })
    }

    /// Bits of information the sieve extracts, as a rough guide for the user.
    pub fn information_bits(&self) -> f64 {
        2.0 * self.liftable.len() as f64 * (self.modulus as f64).log2()
    }

    pub fn liftable_count(&self) -> usize {
        self.liftable.len()
    }

    /// Tests one candidate low-bit pattern.
    #[inline]
    fn accepts_low(&self, low: u64) -> bool {
        let m = self.modulus;
        for o in &self.liftable {
            let (x, z) = feature_offset(low as i64, o);
            if (x as u64) % m != (o.offset_x as u64) % m
                || (z as u64) % m != (o.offset_z as u64) % m
            {
                return false;
            }
        }
        true
    }

    /// Every low-bit pattern consistent with the observations.
    pub fn survivors(&self) -> Vec<u64> {
        (0u64..(1u64 << self.bits))
            .into_par_iter()
            .filter(|low| self.accepts_low(*low))
            .collect()
    }

    /// Full sweep: for each surviving low pattern, try every high-bit
    /// completion and keep the seeds that place every structure correctly.
    pub fn crack(
        &self,
        survivors: &[u64],
        scanned: &AtomicU64,
        cancel: &AtomicBool,
        limit: usize,
    ) -> Vec<i64> {
        let high_count = 1u64 << (48 - self.bits);
        const CHUNK: u64 = 1 << 18;

        let jobs: Vec<(u64, u64)> = survivors
            .iter()
            .flat_map(|low| {
                (0..high_count)
                    .step_by(CHUNK as usize)
                    .map(move |start| (*low, start))
            })
            .collect();

        jobs.into_par_iter()
            .map(|(low, start)| {
                if cancel.load(Ordering::Relaxed) {
                    return Vec::new();
                }
                let end = (start + CHUNK).min(high_count);
                let mut hits = Vec::new();
                for high in start..end {
                    let seed = ((high << self.bits) | low) as i64;
                    if seed_matches(seed, &self.all) {
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

    /// Total candidates the full sweep will visit.
    pub fn sweep_size(&self, survivors: usize) -> u64 {
        survivors as u64 * (1u64 << (48 - self.bits))
    }
}

/// Builds observations from the session's structure list.
pub fn observations_from_session(
    version: Version,
    raw: &[StructureObservation],
) -> Result<Vec<Observation>> {
    raw.iter()
        .map(|o| Observation::from_block_position(version, o.structure, o.x, o.z))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::WorldGen;

    const VERSION: Version = Version::V1_21_1;

    /// Harvests real structures out of a seed, as a player walking the world
    /// would record them.
    fn harvest(seed: i64, structure: StructureType, radius: i32) -> Vec<Observation> {
        let mut world = WorldGen::overworld(VERSION, seed);
        world
            .structures_in_box(structure, -radius, -radius, radius, radius)
            .unwrap()
            .into_iter()
            .map(|p| Observation::from_block_position(VERSION, structure, p.x, p.z).unwrap())
            .collect()
    }

    #[test]
    fn matches_cubiomes_structure_positions() {
        // Our hot-loop port must agree with cubiomes exactly, or the sweep
        // rejects the true seed. Checked across structures, versions and both
        // signs of region coordinate.
        for version in [
            Version::V1_12_2,
            Version::V1_16_5,
            Version::V1_18_2,
            Version::V1_21_1,
        ] {
            for structure in [
                StructureType::Village,
                StructureType::Desert_Pyramid,
                StructureType::Swamp_Hut,
                StructureType::Igloo,
                StructureType::Shipwreck,
                StructureType::Outpost,
                // The large-structure path: two draws per axis, halved.
                StructureType::Monument,
                StructureType::Mansion,
            ] {
                let Some(config) = structure_config(version, structure) else {
                    continue;
                };
                for seed in [0i64, 1, 1234, -99887766, 765906787396911863] {
                    let mut world = WorldGen::overworld(version, seed);
                    let span = config.region_blocks() * 2;
                    let real = world
                        .structures_in_box(structure, -span, -span, span, span)
                        .unwrap();
                    for p in real {
                        let obs =
                            Observation::from_block_position(version, structure, p.x, p.z).unwrap();
                        let (x, z) = feature_offset(seed, &obs);
                        assert_eq!(
                            (x, z),
                            (obs.offset_x, obs.offset_z),
                            "disagreed with cubiomes for {structure:?} in {} at ({}, {}) seed {seed}",
                            version.label(),
                            p.x,
                            p.z
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn both_placement_families_are_covered_by_the_cross_check() {
        // Guards against the cross-check silently degrading to feature-only.
        assert_eq!(
            placement_of(StructureType::Monument, VERSION),
            Some(Placement::Large)
        );
        assert_eq!(
            placement_of(StructureType::Village, VERSION),
            Some(Placement::Feature)
        );
        // Fortress placement moved to the feature path at 1.18.
        assert_eq!(
            placement_of(StructureType::Fortress, Version::V1_21_1),
            Some(Placement::Feature)
        );
        assert_eq!(placement_of(StructureType::Fortress, Version::V1_16_5), None);
        // Paths we deliberately refuse.
        assert_eq!(placement_of(StructureType::Treasure, VERSION), None);
        assert_eq!(placement_of(StructureType::Mineshaft, VERSION), None);
    }

    #[test]
    fn only_the_low_48_bits_matter() {
        let obs = harvest(1234, StructureType::Desert_Pyramid, 12000);
        assert!(!obs.is_empty(), "no desert pyramids found to test with");
        for k in 1..5i64 {
            assert!(
                seed_matches(1234i64.wrapping_add(k << 48), &obs),
                "upper bits should not affect structure placement"
            );
        }
    }

    #[test]
    fn the_sieve_keeps_the_true_low_bits() {
        let seed: i64 = 0x0000_ABCD_1234_5678u64 as i64 & ((1 << 48) - 1);
        let mut obs = harvest(seed, StructureType::Desert_Pyramid, 6000);
        obs.extend(harvest(seed, StructureType::Igloo, 6000));
        obs.extend(harvest(seed, StructureType::Swamp_Hut, 6000));
        assert!(obs.len() >= 3, "need a few structures, got {}", obs.len());

        let sieve = Sieve::new(&obs).unwrap();
        assert_eq!(sieve.bits, 17 + 3, "chunk range 24 should lift three bits");
        assert_eq!(sieve.modulus, 8);

        let survivors = sieve.survivors();
        let true_low = (seed as u64) & ((1 << sieve.bits) - 1);
        assert!(
            survivors.contains(&true_low),
            "sieve dropped the true low bits ({} survivors)",
            survivors.len()
        );
        // With 2*3 = 6 mod-8 constraints (18 bits) over a 20-bit space, the
        // survivor set should be a small fraction of it.
        assert!(
            survivors.len() < (1 << sieve.bits) / 100,
            "sieve barely narrowed anything: {} of {}",
            survivors.len(),
            1u64 << sieve.bits
        );
    }

    #[test]
    fn a_known_seed_is_recovered_from_structures_alone() {
        // The whole point of this module: no pillars, no decorators, just
        // structures. The sweep is restricted to the true seed's high-bit
        // neighbourhood so the test stays fast; `crack` itself is unchanged.
        let seed: i64 = 0x0000_1357_9BDF_2468u64 as i64 & ((1 << 48) - 1);
        let mut obs = harvest(seed, StructureType::Desert_Pyramid, 8000);
        obs.extend(harvest(seed, StructureType::Igloo, 8000));
        obs.extend(harvest(seed, StructureType::Swamp_Hut, 8000));
        obs.extend(harvest(seed, StructureType::Village, 5000));
        assert!(obs.len() >= 4);

        assert!(seed_matches(seed, &obs), "the true seed must match its own structures");

        let sieve = Sieve::new(&obs).unwrap();
        let true_low = (seed as u64) & ((1 << sieve.bits) - 1);
        let survivors = sieve.survivors();
        assert!(survivors.contains(&true_low));

        // Sweep a slice of the high bits around the true value.
        let true_high = (seed as u64) >> sieve.bits;
        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let mut found = Vec::new();
        for high in true_high.saturating_sub(20_000)..(true_high + 20_000) {
            let candidate = ((high << sieve.bits) | true_low) as i64;
            if seed_matches(candidate, &obs) {
                found.push(candidate);
            }
        }
        std::hint::black_box((&scanned, &cancel));
        assert!(
            found.contains(&seed),
            "structures alone did not recover the seed; found {found:?}"
        );
        assert_eq!(found.len(), 1, "expected a unique seed, got {found:?}");
    }

    #[test]
    fn the_crack_sweep_finds_a_seed_in_its_slice() {
        // Exercises `crack` itself (parallel, chunked, cancellation) rather
        // than just the predicate, on one low-bit survivor.
        let seed: i64 = 0x0000_0000_0BAD_F00Du64 as i64;
        let mut obs = harvest(seed, StructureType::Desert_Pyramid, 8000);
        obs.extend(harvest(seed, StructureType::Igloo, 8000));
        assert!(obs.len() >= 2);

        let sieve = Sieve::new(&obs).unwrap();
        let true_low = (seed as u64) & ((1 << sieve.bits) - 1);
        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let hits = sieve.crack(&[true_low], &scanned, &cancel, 4096);
        assert!(
            hits.contains(&seed),
            "the sweep missed the seed on its own low-bit pattern"
        );
        assert!(scanned.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn unliftable_structure_sets_are_refused_with_an_explanation() {
        // Ocean monuments have chunk range 27 — odd, so no low bits leak.
        let obs = harvest(1234, StructureType::Monument, 20000);
        assert!(!obs.is_empty());
        let err = Sieve::new(&obs).unwrap_err().to_string();
        assert!(err.contains("low-bit sieve"), "unhelpful error: {err}");

        assert!(Sieve::new(&[]).is_err());
    }

    #[test]
    fn large_structures_are_excluded_from_the_sieve_but_kept_for_the_check() {
        let seed = 4242i64;
        let mut obs = harvest(seed, StructureType::Desert_Pyramid, 6000);
        let monuments = harvest(seed, StructureType::Monument, 6000);
        obs.extend(monuments.iter().copied());

        let sieve = Sieve::new(&obs).unwrap();
        assert_eq!(
            sieve.liftable_count(),
            obs.len() - monuments.len(),
            "monuments should not be counted as liftable"
        );
        // But they still constrain the full check.
        assert!(seed_matches(seed, &obs));
        assert!(!monuments.is_empty() && !monuments[0].is_liftable());
    }

    #[test]
    fn observations_reduce_block_positions_correctly() {
        // Negative coordinates are the classic trap: region and offset must
        // both use floor division.
        let obs =
            Observation::from_block_position(VERSION, StructureType::Desert_Pyramid, -1600, -1600)
                .unwrap();
        let size = obs.config.region_size;
        assert!(obs.offset_x >= 0 && obs.offset_x < size);
        assert!(obs.offset_z >= 0 && obs.offset_z < size);
        assert_eq!((-1600i32).div_euclid(16).div_euclid(size), obs.region_x);

        assert!(
            Observation::from_block_position(
                Version::V1_8_9,
                StructureType::Ancient_City,
                0,
                0
            )
            .is_err()
        );
    }
}
