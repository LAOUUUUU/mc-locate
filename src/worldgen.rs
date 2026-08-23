//! Thin wrapper over the `cubiomes` crate (Cubitect's C library).
//!
//! Everything cubiomes already implements correctly — biome generation,
//! structure region salts and spacing, stronghold placement, world spawn — is
//! delegated to it rather than reimplemented here. Structure salts in
//! particular change across versions (notably at 1.13) and hand-rolling them
//! is the single easiest way to produce confidently wrong output.
//!
//! Only the things cubiomes does *not* cover are implemented by hand elsewhere
//! in this crate: nether bedrock ([`crate::bedrock`]), End pillars and slime
//! chunks ([`crate::multicrack`], [`crate::slime`]), and portal maths
//! ([`crate::portal`]).

use anyhow::{Context, Result, bail};
use cubiomes::enums::{BiomeID, Dimension, MCVersion, StructureType};
use cubiomes::generator::{BlockPosition, Cache, Generator, GeneratorFlags, Range, Scale};
use cubiomes::noise::{BiomeNoise, SurfaceNoiseRelease};
use cubiomes::structures::StructureRegion;

/// The Minecraft versions this tool exposes.
///
/// Behaviour changes at several boundaries that matter to us:
/// * **1.13** — structure salts and spacing were reworked.
/// * **1.15** — the server started sending a SHA-256 hashed seed to clients.
/// * **1.16** — nether biome generation moved to 3D noise.
/// * **1.18** — Caves & Cliffs: world height changed, and nether bedrock
///   became seed-dependent (which is what makes mode 1b possible at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Version {
    B1_7,
    B1_8,
    V1_0_0,
    V1_1_0,
    V1_2_5,
    V1_3_2,
    V1_4_7,
    V1_5_2,
    V1_6_4,
    V1_7_10,
    V1_8_9,
    V1_9_4,
    V1_10_2,
    V1_11_2,
    V1_12_2,
    V1_13_2,
    V1_14_4,
    V1_15_2,
    V1_16_1,
    V1_16_5,
    V1_17_1,
    V1_18_2,
    V1_19_2,
    V1_19_4,
    V1_20_6,
    V1_21_1,
    V1_21_3,
    V1_21_4,
    V1_21_5,
    V1_21_6,
    V1_21_9,
    V1_21_11,
    V26_1,
    V26_2,
}

impl Version {
    /// Menu order — newest first, since that is what most users want.
    pub const ALL: [Version; 34] = [
        Version::V26_2,
        Version::V26_1,
        Version::V1_21_11,
        Version::V1_21_9,
        Version::V1_21_6,
        Version::V1_21_5,
        Version::V1_21_4,
        Version::V1_21_3,
        Version::V1_21_1,
        Version::V1_20_6,
        Version::V1_19_4,
        Version::V1_19_2,
        Version::V1_18_2,
        Version::V1_17_1,
        Version::V1_16_5,
        Version::V1_16_1,
        Version::V1_15_2,
        Version::V1_14_4,
        Version::V1_13_2,
        Version::V1_12_2,
        Version::V1_11_2,
        Version::V1_10_2,
        Version::V1_9_4,
        Version::V1_8_9,
        Version::V1_7_10,
        Version::V1_6_4,
        Version::V1_5_2,
        Version::V1_4_7,
        Version::V1_3_2,
        Version::V1_2_5,
        Version::V1_1_0,
        Version::V1_0_0,
        Version::B1_8,
        Version::B1_7,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            // From 2026 Minecraft numbers releases by year rather than 1.x;
            // there is no 1.22, the line runs 1.21.11 -> 26.1 -> 26.2.
            Version::V26_2 => "26.2",
            Version::V26_1 => "26.1",
            Version::V1_21_11 => "1.21.11",
            Version::V1_21_9 => "1.21.9",
            Version::V1_21_6 => "1.21.6",
            Version::V1_21_5 => "1.21.5",
            // cubiomes calls 1.21.4 "1.21 WD" because the constant was written
            // before the Winter Drop shipped and never renamed. It is 1.21.4,
            // "The Garden Awakens", and is labelled that way here.
            Version::V1_21_4 => "1.21.4",
            Version::V1_21_3 => "1.21.3",
            Version::V1_21_1 => "1.21.1",
            Version::V1_20_6 => "1.20.6",
            Version::V1_19_4 => "1.19.4",
            Version::V1_19_2 => "1.19.2",
            Version::V1_18_2 => "1.18.2",
            Version::V1_17_1 => "1.17.1",
            Version::V1_16_5 => "1.16.5",
            Version::V1_16_1 => "1.16.1",
            Version::V1_15_2 => "1.15.2",
            Version::V1_14_4 => "1.14.4",
            Version::V1_13_2 => "1.13.2",
            Version::V1_12_2 => "1.12.2",
            Version::V1_11_2 => "1.11.2",
            Version::V1_10_2 => "1.10.2",
            Version::V1_9_4 => "1.9.4",
            Version::V1_8_9 => "1.8.9",
            Version::V1_7_10 => "1.7.10",
            Version::V1_6_4 => "1.6.4",
            Version::V1_5_2 => "1.5.2",
            Version::V1_4_7 => "1.4.7",
            Version::V1_3_2 => "1.3.2",
            Version::V1_2_5 => "1.2.5",
            Version::V1_1_0 => "1.1",
            Version::V1_0_0 => "1.0",
            Version::B1_8 => "Beta 1.8",
            Version::B1_7 => "Beta 1.7",
        }
    }

    pub fn mc(&self) -> MCVersion {
        match self {
            Version::V26_2 => MCVersion::MC_26_2,
            Version::V26_1 => MCVersion::MC_26_1,
            Version::V1_21_11 => MCVersion::MC_1_21_11,
            Version::V1_21_9 => MCVersion::MC_1_21_9,
            Version::V1_21_6 => MCVersion::MC_1_21_6,
            Version::V1_21_5 => MCVersion::MC_1_21_5,
            Version::V1_21_4 => MCVersion::MC_1_21_WD,
            Version::V1_21_3 => MCVersion::MC_1_21_3,
            Version::V1_21_1 => MCVersion::MC_1_21_1,
            Version::V1_20_6 => MCVersion::MC_1_20_6,
            Version::V1_19_4 => MCVersion::MC_1_19_4,
            Version::V1_19_2 => MCVersion::MC_1_19_2,
            Version::V1_18_2 => MCVersion::MC_1_18_2,
            Version::V1_17_1 => MCVersion::MC_1_17_1,
            Version::V1_16_5 => MCVersion::MC_1_16_5,
            Version::V1_16_1 => MCVersion::MC_1_16_1,
            Version::V1_15_2 => MCVersion::MC_1_15_2,
            Version::V1_14_4 => MCVersion::MC_1_14_4,
            Version::V1_13_2 => MCVersion::MC_1_13_2,
            Version::V1_12_2 => MCVersion::MC_1_12_2,
            Version::V1_11_2 => MCVersion::MC_1_11_2,
            Version::V1_10_2 => MCVersion::MC_1_10_2,
            Version::V1_9_4 => MCVersion::MC_1_9_4,
            Version::V1_8_9 => MCVersion::MC_1_8_9,
            Version::V1_7_10 => MCVersion::MC_1_7_10,
            Version::V1_6_4 => MCVersion::MC_1_6_4,
            Version::V1_5_2 => MCVersion::MC_1_5_2,
            Version::V1_4_7 => MCVersion::MC_1_4_7,
            Version::V1_3_2 => MCVersion::MC_1_3_2,
            Version::V1_2_5 => MCVersion::MC_1_2_5,
            Version::V1_1_0 => MCVersion::MC_1_1_0,
            Version::V1_0_0 => MCVersion::MC_1_0_0,
            Version::B1_8 => MCVersion::MC_B1_8,
            Version::B1_7 => MCVersion::MC_B1_7,
        }
    }

    /// Is this version at least `other`?
    ///
    /// cubiomes' `MCVersion` is declared in release order, so comparing the
    /// discriminants is a valid ordering — and far less error-prone than
    /// maintaining a hand-written match arm per capability.
    pub fn at_least(&self, other: MCVersion) -> bool {
        (self.mc() as i32) >= (other as i32)
    }

    /// 1.18 is the boundary where nether bedrock became seed-dependent and the
    /// world floor dropped to y=-64.
    pub fn is_1_18_plus(&self) -> bool {
        self.at_least(MCVersion::MC_1_18_2)
    }

    /// Beta versions use an entirely different terrain pipeline.
    pub fn is_beta(&self) -> bool {
        !self.at_least(MCVersion::MC_1_0_0)
    }

    /// Can [`WorldGen::surface_heights`] be used?
    ///
    /// The wrapper *panics* rather than erroring for beta, so this must be
    /// checked before calling it — exposing beta in the menu without this turns
    /// a version choice into a crash.
    pub fn supports_height_map(&self) -> bool {
        !self.is_beta()
    }

    /// How many strongholds this version generates.
    ///
    /// The familiar 128-across-8-rings arrangement arrived in 1.9. Before that
    /// there were only three, and mode 10's ring prior does not describe them.
    pub fn stronghold_count(&self) -> usize {
        if self.at_least(MCVersion::MC_1_9_4) { 128 } else { 3 }
    }

    /// Does the documented 8-ring stronghold structure apply?
    pub fn has_stronghold_rings(&self) -> bool {
        self.at_least(MCVersion::MC_1_9_4)
    }

    /// Lowest buildable y in the Overworld.
    pub fn overworld_min_y(&self) -> i32 {
        if self.is_1_18_plus() { -64 } else { 0 }
    }

    /// The nether floor and roof bedrock layers, as (floor_range, roof_range).
    ///
    /// The Nether kept a 0..127 build range even after 1.18 raised the
    /// Overworld floor, so these are version-independent.
    pub fn nether_bedrock_layers(&self) -> (std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>) {
        (0..=4, 122..=127)
    }

    /// Half-width of the world border, in blocks.
    pub fn world_border(&self) -> i32 {
        29_999_984
    }
}

/// A seeded cubiomes generator for one dimension.
pub struct WorldGen {
    generator: Generator,
    version: Version,
    seed: i64,
    dimension: Dimension,
}

impl WorldGen {
    pub fn new(version: Version, seed: i64, dimension: Dimension) -> Self {
        Self {
            generator: Generator::new(version.mc(), seed, dimension, GeneratorFlags::empty()),
            version,
            seed,
            dimension,
        }
    }

    pub fn overworld(version: Version, seed: i64) -> Self {
        Self::new(version, seed, Dimension::DIM_OVERWORLD)
    }

    pub fn nether(version: Version, seed: i64) -> Self {
        Self::new(version, seed, Dimension::DIM_NETHER)
    }

    pub fn end(version: Version, seed: i64) -> Self {
        Self::new(version, seed, Dimension::DIM_END)
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Re-points this generator at a different seed without rebuilding it.
    pub fn apply_seed(&mut self, seed: i64) {
        self.generator.apply_seed(self.dimension, seed);
        self.seed = seed;
    }

    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> Result<BiomeID> {
        self.generator
            .get_biome_at(x, y, z)
            .with_context(|| format!("cubiomes could not resolve a biome at ({x}, {y}, {z})"))
    }

    /// Samples biomes over a rectangle at the given scale, returning the raw
    /// cache. Much faster than calling [`WorldGen::biome_at`] in a loop.
    pub fn biome_rect(
        &self,
        x: i32,
        z: i32,
        size_x: u32,
        size_z: u32,
        y: i32,
        scale: Scale,
    ) -> Result<Cache<'_>> {
        let range = Range {
            scale,
            x: scale.scale_coord(x),
            z: scale.scale_coord(z),
            size_x,
            size_z,
            y,
            size_y: 1,
        };
        Cache::new(&self.generator, range).context("could not allocate a cubiomes biome cache")
    }

    /// Approximate surface height over a rectangle, in blocks.
    ///
    /// Coordinates and sizes are at cubiomes' 1:4 noise scale, so `quad_x = 128`
    /// means block x = 512. The returned vector is row-major with
    /// `heights[iz * size_x + ix]`.
    ///
    /// This is `mapApproxHeight`, the same estimate Cubiomes Viewer draws its
    /// terrain preview from — good enough to match the *shape* of a ridgeline
    /// from a screenshot, but not a block-exact heightmap. Mode 2 compares it
    /// with a tolerance for exactly that reason.
    pub fn surface_heights(
        &self,
        quad_x: i32,
        quad_z: i32,
        size_x: u32,
        size_z: u32,
    ) -> Result<Vec<f32>> {
        // The wrapper panics for beta rather than returning an error, so this
        // has to be caught here or a menu choice becomes a crash.
        if !self.version.supports_height_map() {
            bail!(
                "{} has no surface height approximation in cubiomes — use a biome pattern \
                 instead of a height one",
                self.version.label()
            );
        }
        let noise: BiomeNoise = SurfaceNoiseRelease::new(self.dimension, self.seed).into();
        self.generator
            .approx_surface_noise(quad_x, quad_z, size_x, size_z, &noise)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "cubiomes could not approximate surface height at quad ({quad_x}, {quad_z})"
                )
            })
    }

    /// Approximate surface height at a single block position.
    pub fn surface_height_at(&self, x: i32, z: i32) -> Result<f32> {
        let v = self.surface_heights(x.div_euclid(4), z.div_euclid(4), 1, 1)?;
        v.first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("empty heightmap response"))
    }

    /// Every stronghold in this seed, in generation order.
    ///
    /// These are the *real* positions, including the biome snapping that ring
    /// maths alone cannot predict — which is exactly why mode 10 uses them as
    /// its candidate set rather than computing the rings itself.
    ///
    /// This drives cubiomes' C iterator directly rather than using the safe
    /// wrapper's `Generator::strongholds()`, because that wrapper has an
    /// off-by-one that silently drops the first stronghold and yields 127 of
    /// the 128. Losing the innermost ring's first entry would quietly bias
    /// mode 10's posterior, so it is worth the small unsafe block.
    pub fn strongholds(&self) -> Vec<BlockPosition> {
        use std::mem::MaybeUninit;

        let mut iter: MaybeUninit<cubiomes_sys::StrongholdIter> = MaybeUninit::uninit();

        // SAFETY: `initFirstStronghold` fully initialises the iterator from a
        // version tag and a seed; both are plain values with no invariants.
        unsafe {
            cubiomes_sys::initFirstStronghold(
                iter.as_mut_ptr(),
                self.version.mc() as i32,
                self.seed as u64,
            );
        }
        // SAFETY: initialised by the call above.
        let mut iter = unsafe { iter.assume_init() };

        let mut out = Vec::with_capacity(128);
        loop {
            // `nextStronghold` resolves a stronghold into `iter.pos` and
            // returns the number remaining *including* the one it just wrote.
            // A return of 0 therefore means it produced nothing and `iter.pos`
            // must not be read. (The header's wording suggests "after this
            // one"; the observed behaviour is inclusive, which is also what
            // the safe wrapper's `- 1` fudge is compensating for.)
            //
            // SAFETY: `iter` is initialised and the generator outlives this
            // call; we hold `&self`, so nothing else can mutate it meanwhile.
            let remaining =
                unsafe { cubiomes_sys::nextStronghold(&mut iter, self.generator.as_ptr()) };
            if remaining <= 0 {
                break;
            }
            out.push(BlockPosition::new(iter.pos.x, iter.pos.z));
            // Pre-1.9 worlds have three strongholds, not 128; the cap has to
            // follow the version or the loop runs past the real count.
            if out.len() >= self.version.stronghold_count() {
                break;
            }
        }
        out
    }

    /// All generated structures of one type whose region overlaps the given
    /// block-coordinate bounding box.
    ///
    /// A "generation attempt" is a position cubiomes derives from the seed and
    /// the region coordinates; it only becomes a real structure if the biome
    /// there is viable, which is what the verification step checks.
    pub fn structures_in_box(
        &mut self,
        structure: StructureType,
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
    ) -> Result<Vec<BlockPosition>> {
        let probe = StructureRegion::new(0, 0, self.version.mc(), structure).map_err(|e| {
            anyhow::anyhow!(
                "{} does not generate in {}: {e:?}",
                structure_label(structure),
                self.version.label()
            )
        })?;
        let region_blocks = probe.region_size_blocks();

        let r_min_x = min_x.div_euclid(region_blocks);
        let r_max_x = max_x.div_euclid(region_blocks);
        let r_min_z = min_z.div_euclid(region_blocks);
        let r_max_z = max_z.div_euclid(region_blocks);

        let mut found = Vec::new();
        for rx in r_min_x..=r_max_x {
            for rz in r_min_z..=r_max_z {
                let region = StructureRegion::new(rx, rz, self.version.mc(), structure)
                    .map_err(|e| anyhow::anyhow!("bad structure region: {e:?}"))?;
                if let Some(pos) = self.generator.try_generate_structure_in_region(region)
                    && pos.x >= min_x
                    && pos.x <= max_x
                    && pos.z >= min_z
                    && pos.z <= max_z
                {
                    found.push(pos);
                }
            }
        }
        Ok(found)
    }
}

/// A structure's per-version placement configuration, straight from cubiomes.
///
/// These are the numbers that must never be hand-rolled: the salt and the
/// region grid changed at 1.13 and again for individual structures since, and
/// a stale constant produces confident nonsense. `getStructureConfig` is
/// cubiomes' own per-version table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructureConfig {
    /// Mixed into the region seed, unique per structure type.
    pub salt: i32,
    /// Region grid size, in chunks.
    pub region_size: i32,
    /// The bound passed to `nextInt` when picking the in-region offset.
    pub chunk_range: i32,
    pub rarity: f32,
}

impl StructureConfig {
    /// Region size in blocks.
    pub fn region_blocks(&self) -> i32 {
        self.region_size * 16
    }

    /// The 2-adic valuation of `chunk_range` — how many low bits of the
    /// `nextInt` result are determined by the low bits of the seed.
    ///
    /// This is what makes bit-lifting work: for `2^j | chunk_range`,
    /// `nextInt(chunk_range) mod 2^j` equals `(state >> 17) mod 2^j`, which
    /// depends only on the low `17 + j` bits of the seed.
    pub fn lift_valuation(&self) -> u32 {
        if self.chunk_range <= 0 {
            return 0;
        }
        self.chunk_range.trailing_zeros()
    }

    /// Whether `nextInt` takes its power-of-two fast path for this range.
    ///
    /// That branch reads the *high* bits of the state, so such structures
    /// cannot take part in a low-bit sieve.
    pub fn is_power_of_two_range(&self) -> bool {
        self.chunk_range > 0 && (self.chunk_range & (self.chunk_range - 1)) == 0
    }
}

/// Looks up a structure's placement configuration for a version.
///
/// Returns `None` when the structure does not generate in that version.
pub fn structure_config(version: Version, structure: StructureType) -> Option<StructureConfig> {
    use std::mem::MaybeUninit;

    let mut conf: MaybeUninit<cubiomes_sys::StructureConfig> = MaybeUninit::uninit();
    // SAFETY: `getStructureConfig` fills the out-parameter and returns non-zero
    // on success; both inputs are plain enum tags.
    let ok = unsafe {
        cubiomes_sys::getStructureConfig(
            structure as i32,
            version.mc() as i32,
            conf.as_mut_ptr(),
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: initialised by the call above.
    let conf = unsafe { conf.assume_init() };
    Some(StructureConfig {
        salt: conf.salt,
        region_size: conf.regionSize as i32,
        chunk_range: conf.chunkRange as i32,
        rarity: conf.rarity,
    })
}

/// Structure types offered in the menus, with the dimension each lives in.
pub const STRUCTURES: &[(StructureType, &str, Dimension)] = &[
    (StructureType::Village, "Village", Dimension::DIM_OVERWORLD),
    (StructureType::Desert_Pyramid, "Desert pyramid", Dimension::DIM_OVERWORLD),
    (StructureType::Jungle_Temple, "Jungle temple", Dimension::DIM_OVERWORLD),
    (StructureType::Swamp_Hut, "Swamp hut", Dimension::DIM_OVERWORLD),
    (StructureType::Igloo, "Igloo", Dimension::DIM_OVERWORLD),
    (StructureType::Ocean_Ruin, "Ocean ruin", Dimension::DIM_OVERWORLD),
    (StructureType::Shipwreck, "Shipwreck", Dimension::DIM_OVERWORLD),
    (StructureType::Monument, "Ocean monument", Dimension::DIM_OVERWORLD),
    (StructureType::Mansion, "Woodland mansion", Dimension::DIM_OVERWORLD),
    (StructureType::Outpost, "Pillager outpost", Dimension::DIM_OVERWORLD),
    (StructureType::Ruined_Portal, "Ruined portal", Dimension::DIM_OVERWORLD),
    (StructureType::Ancient_City, "Ancient city", Dimension::DIM_OVERWORLD),
    (StructureType::Treasure, "Buried treasure", Dimension::DIM_OVERWORLD),
    (StructureType::Mineshaft, "Mineshaft", Dimension::DIM_OVERWORLD),
    (StructureType::Trail_Ruins, "Trail ruins", Dimension::DIM_OVERWORLD),
    (StructureType::Trial_Chambers, "Trial chambers", Dimension::DIM_OVERWORLD),
    (StructureType::Fortress, "Nether fortress", Dimension::DIM_NETHER),
    (StructureType::Bastion, "Bastion remnant", Dimension::DIM_NETHER),
    (StructureType::Ruined_Portal_N, "Ruined portal (nether)", Dimension::DIM_NETHER),
    (StructureType::End_City, "End city", Dimension::DIM_END),
    (StructureType::End_Gateway, "End gateway", Dimension::DIM_END),
];

pub fn structure_label(s: StructureType) -> &'static str {
    STRUCTURES
        .iter()
        .find(|(t, _, _)| *t == s)
        .map(|(_, name, _)| *name)
        .unwrap_or("structure")
}

pub fn structure_dimension(s: StructureType) -> Dimension {
    STRUCTURES
        .iter()
        .find(|(t, _, _)| *t == s)
        .map(|(_, _, d)| *d)
        .unwrap_or(Dimension::DIM_OVERWORLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_boundaries() {
        assert!(!Version::V1_17_1.is_1_18_plus());
        assert!(Version::V1_18_2.is_1_18_plus());
        assert_eq!(Version::V1_17_1.overworld_min_y(), 0);
        assert_eq!(Version::V1_18_2.overworld_min_y(), -64);
    }

    #[test]
    fn every_version_is_listed_once_and_maps_somewhere_distinct() {
        assert_eq!(Version::ALL.len(), 34);

        let mut labels: Vec<&str> = Version::ALL.iter().map(|v| v.label()).collect();
        let n = labels.len();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), n, "duplicate version labels");

        let mut mcs: Vec<i32> = Version::ALL.iter().map(|v| v.mc() as i32).collect();
        mcs.sort();
        mcs.dedup();
        assert_eq!(mcs.len(), n, "two entries map to the same cubiomes version");
    }

    #[test]
    fn the_menu_is_ordered_newest_first() {
        // Users pick from the top; a list that is not sorted makes that a lie.
        let ords: Vec<i32> = Version::ALL.iter().map(|v| v.mc() as i32).collect();
        for pair in ords.windows(2) {
            assert!(pair[0] > pair[1], "ALL is not descending: {ords:?}");
        }
        assert_eq!(Version::ALL[0], Version::V26_2);
        assert_eq!(Version::ALL[Version::ALL.len() - 1], Version::B1_7);
    }

    #[test]
    fn capability_flags_match_the_real_boundaries() {
        assert!(Version::B1_7.is_beta() && Version::B1_8.is_beta());
        assert!(!Version::V1_0_0.is_beta());
        assert!(!Version::B1_8.supports_height_map());
        assert!(Version::V1_0_0.supports_height_map());

        // The 128-stronghold, 8-ring arrangement arrived in 1.9.
        assert_eq!(Version::V1_8_9.stronghold_count(), 3);
        assert_eq!(Version::V1_9_4.stronghold_count(), 128);
        assert!(!Version::V1_8_9.has_stronghold_rings());
        assert!(Version::V1_9_4.has_stronghold_rings());
    }

    #[test]
    fn the_backend_reaches_current_minecraft() {
        // The point of vendoring the maintained fork. If this regresses, the
        // patch in Cargo.toml has stopped taking effect.
        assert_eq!(Version::ALL[0].label(), "26.2");
        for v in [Version::V1_21_5, Version::V1_21_11, Version::V26_1, Version::V26_2] {
            let world = WorldGen::overworld(v, 1234);
            assert!(
                world.biome_at(0, 63, 0).is_ok(),
                "{} could not generate a biome",
                v.label()
            );
            assert_eq!(world.strongholds().len(), 128, "{}", v.label());
        }
    }

    #[test]
    fn twenty_six_two_really_generates_differently_from_its_predecessor() {
        // A version that silently aliased an older one would pass every other
        // test while generating the wrong world.
        //
        // The difference is *underground*: 26.2 ("Chaos Cubed") adds
        // sulfur_caves, biome id 187. Surface biomes are unchanged, so an
        // earlier version of this test sampled y=63, found nothing, and looked
        // like a broken backend. y=10 is where it shows.
        const SULFUR_CAVES: i32 = 187;

        let sample = |v: Version, y: i32| {
            let w = WorldGen::overworld(v, 4242);
            (0..60)
                .map(|i| w.biome_at(i * 256, y, i * 192).map(|b| b as i32).unwrap_or(-1))
                .collect::<Vec<i32>>()
        };
        assert_ne!(
            sample(Version::V1_21_4, 10),
            sample(Version::V26_2, 10),
            "26.2 generates identically to 1.21.4 underground — is the patch active?"
        );

        // And the new biome is actually reachable there, and only there.
        let modern = WorldGen::overworld(Version::V26_2, 4242);
        let legacy = WorldGen::overworld(Version::V1_21_4, 4242);
        let mut modern_hits = 0;
        for i in 0..300 {
            let (x, z) = (i * 128, i * 97);
            if modern.biome_at(x, 10, z).map(|b| b as i32).unwrap_or(-1) == SULFUR_CAVES {
                modern_hits += 1;
            }
            assert_ne!(
                legacy.biome_at(x, 10, z).map(|b| b as i32).unwrap_or(-1),
                SULFUR_CAVES,
                "1.21.4 must never produce a 26.2 biome"
            );
        }
        assert!(modern_hits > 0, "sulfur_caves should be reachable in 26.2");
    }

    #[test]
    fn beta_refuses_the_height_map_instead_of_panicking() {
        // The wrapper panics here; this must be an Err with an explanation.
        let world = WorldGen::overworld(Version::B1_8, 1234);
        let err = world.surface_heights(0, 0, 4, 4).unwrap_err().to_string();
        assert!(err.contains("Beta 1.8"), "unhelpful: {err}");
        assert!(err.contains("biome pattern"), "should suggest the alternative: {err}");
    }

    #[test]
    fn old_versions_generate_and_report_the_right_stronghold_count() {
        for v in [Version::B1_8, Version::V1_2_5, Version::V1_8_9] {
            let world = WorldGen::overworld(v, 1234);
            // Biomes must work on every exposed version.
            assert!(world.biome_at(0, 63, 0).is_ok(), "{} biome lookup failed", v.label());

            let sh = world.strongholds();
            assert_eq!(
                sh.len(),
                v.stronghold_count(),
                "{} produced {} strongholds",
                v.label(),
                sh.len()
            );
        }
        assert_eq!(WorldGen::overworld(Version::V1_21_4, 1234).strongholds().len(), 128);
    }

    #[test]
    fn generator_is_deterministic() {
        let a = WorldGen::overworld(Version::V1_21_1, 12345);
        let b = WorldGen::overworld(Version::V1_21_1, 12345);
        assert_eq!(a.biome_at(0, 64, 0).unwrap(), b.biome_at(0, 64, 0).unwrap());
    }

    #[test]
    fn strongholds_match_the_documented_ring_structure() {
        // The wiki documents 128 strongholds across 8 rings, the innermost
        // ring holding 3 at 1280..2816 blocks from origin. If cubiomes and
        // that description disagree, mode 10's prior is built on sand.
        let world = WorldGen::overworld(Version::V1_21_1, 1234);
        let sh = world.strongholds();
        assert_eq!(sh.len(), 128, "expected 128 strongholds, got {}", sh.len());

        let mut dists: Vec<f64> = sh
            .iter()
            .map(|p| ((p.x as f64).powi(2) + (p.z as f64).powi(2)).sqrt())
            .collect();
        dists.sort_by(|a, b| a.partial_cmp(b).unwrap());

        // Ring 1: three strongholds, all inside the documented band (with a
        // little slack for biome snapping, which moves them up to 112 blocks).
        for d in &dists[..3] {
            assert!(
                (1280.0 - 112.0..=2816.0 + 112.0).contains(d),
                "ring-1 stronghold at distance {d} is outside 1280..2816"
            );
        }
        assert!(
            dists[3] > 4352.0 - 112.0,
            "4th stronghold should already be in ring 2"
        );
    }

    #[test]
    fn surface_heights_are_plausible_block_levels() {
        // 1.18 raised the world; the pre/post ranges should differ clearly,
        // which also confirms the version is actually reaching the generator.
        let modern = WorldGen::overworld(Version::V1_21_1, 1234);
        let hs = modern.surface_heights(128, 128, 32, 32).unwrap();
        assert_eq!(hs.len(), 32 * 32);
        assert!(
            hs.iter().all(|h| (-64.0..=320.0).contains(h)),
            "heights outside the 1.18+ world range"
        );

        let legacy = WorldGen::overworld(Version::V1_16_5, 1234);
        let hs_old = legacy.surface_heights(128, 128, 32, 32).unwrap();
        assert!(
            hs_old.iter().all(|h| (0.0..=256.0).contains(h)),
            "heights outside the pre-1.18 world range"
        );

        // Single-point lookup should agree with the rectangle it sits in.
        let one = modern.surface_height_at(512, 512).unwrap();
        assert!((one - hs[0]).abs() < 1e-3, "{one} vs {}", hs[0]);
    }

    #[test]
    fn structure_configs_come_from_cubiomes_and_track_the_version() {
        // The documented legacy village salt, which did *not* change at 1.13.
        for v in [Version::V1_12_2, Version::V1_16_5, Version::V1_21_1] {
            assert_eq!(structure_config(v, StructureType::Village).unwrap().salt, 10387312);
        }

        // What 1.13 actually changed for villages is nothing; what changed is
        // that the temple family stopped sharing one salt. Before 1.13 desert
        // pyramids, swamp huts and igloos were a single "Feature" type on salt
        // 14357617; afterwards they split.
        let old_hut = structure_config(Version::V1_12_2, StructureType::Swamp_Hut).unwrap();
        let old_igloo = structure_config(Version::V1_12_2, StructureType::Igloo).unwrap();
        let old_pyramid = structure_config(Version::V1_12_2, StructureType::Desert_Pyramid).unwrap();
        assert_eq!(old_hut.salt, old_pyramid.salt);
        assert_eq!(old_igloo.salt, old_pyramid.salt);

        let new_hut = structure_config(Version::V1_13_2, StructureType::Swamp_Hut).unwrap();
        let new_igloo = structure_config(Version::V1_13_2, StructureType::Igloo).unwrap();
        assert_ne!(new_hut.salt, old_hut.salt, "swamp hut salt should split at 1.13");
        assert_ne!(new_hut.salt, new_igloo.salt, "the temple salts should differ after 1.13");

        // Village spacing widened at 1.18 (32/24 region/range -> 34/26).
        let pre = structure_config(Version::V1_17_1, StructureType::Village).unwrap();
        let post = structure_config(Version::V1_18_2, StructureType::Village).unwrap();
        assert_eq!((pre.region_size, pre.chunk_range), (32, 24));
        assert_eq!((post.region_size, post.chunk_range), (34, 26));
        assert_eq!(post.region_blocks(), 34 * 16);

        // A structure that did not exist yet has no config at all.
        assert!(structure_config(Version::V1_8_9, StructureType::Ancient_City).is_none());
        assert!(structure_config(Version::V1_12_2, StructureType::Shipwreck).is_none());
    }

    #[test]
    fn lift_valuation_matches_the_chunk_range() {
        // 24 = 8*3 so three low bits are liftable; 20 = 4*5 gives two.
        let probe = |r: i32| StructureConfig {
            salt: 0,
            region_size: 32,
            chunk_range: r,
            rarity: 0.0,
        };
        assert_eq!(probe(24).lift_valuation(), 3);
        assert_eq!(probe(20).lift_valuation(), 2);
        assert_eq!(probe(16).lift_valuation(), 4);
        assert!(probe(16).is_power_of_two_range());
        assert!(!probe(24).is_power_of_two_range());
    }

    #[test]
    fn structures_are_found_and_land_in_the_requested_box() {
        let mut world = WorldGen::overworld(Version::V1_21_1, 1234);
        let found = world
            .structures_in_box(StructureType::Village, -3000, -3000, 3000, 3000)
            .unwrap();
        assert!(!found.is_empty(), "expected at least one village near spawn");
        for p in &found {
            assert!((-3000..=3000).contains(&p.x) && (-3000..=3000).contains(&p.z));
        }
    }
}
