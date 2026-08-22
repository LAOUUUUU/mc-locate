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

use anyhow::{Context, Result};
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
    V1_8_9,
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
}

impl Version {
    /// Menu order — newest first, since that is what most users want.
    pub const ALL: [Version; 14] = [
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
        Version::V1_8_9,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Version::V1_8_9 => "1.8.9",
            Version::V1_12_2 => "1.12.2",
            Version::V1_13_2 => "1.13.2",
            Version::V1_14_4 => "1.14.4",
            Version::V1_15_2 => "1.15.2",
            Version::V1_16_1 => "1.16.1",
            Version::V1_16_5 => "1.16.5",
            Version::V1_17_1 => "1.17.1",
            Version::V1_18_2 => "1.18.2",
            Version::V1_19_2 => "1.19.2",
            Version::V1_19_4 => "1.19.4",
            Version::V1_20_6 => "1.20.6",
            Version::V1_21_1 => "1.21.1",
            Version::V1_21_3 => "1.21.3",
        }
    }

    pub fn mc(&self) -> MCVersion {
        match self {
            Version::V1_8_9 => MCVersion::MC_1_8_9,
            Version::V1_12_2 => MCVersion::MC_1_12_2,
            Version::V1_13_2 => MCVersion::MC_1_13_2,
            Version::V1_14_4 => MCVersion::MC_1_14_4,
            Version::V1_15_2 => MCVersion::MC_1_15_2,
            Version::V1_16_1 => MCVersion::MC_1_16_1,
            Version::V1_16_5 => MCVersion::MC_1_16_5,
            Version::V1_17_1 => MCVersion::MC_1_17_1,
            Version::V1_18_2 => MCVersion::MC_1_18_2,
            Version::V1_19_2 => MCVersion::MC_1_19_2,
            Version::V1_19_4 => MCVersion::MC_1_19_4,
            Version::V1_20_6 => MCVersion::MC_1_20_6,
            Version::V1_21_1 => MCVersion::MC_1_21_1,
            Version::V1_21_3 => MCVersion::MC_1_21_3,
        }
    }

    /// 1.18 is the boundary where nether bedrock became seed-dependent and the
    /// world floor dropped to y=-64.
    pub fn is_1_18_plus(&self) -> bool {
        !matches!(
            self,
            Version::V1_8_9
                | Version::V1_12_2
                | Version::V1_13_2
                | Version::V1_14_4
                | Version::V1_15_2
                | Version::V1_16_1
                | Version::V1_16_5
                | Version::V1_17_1
        )
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
            if out.len() >= 128 {
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
