//! Mode 2 — matching a transcribed terrain patch against a known seed.
//!
//! You know the seed but not where you are. You transcribe what you can see
//! from a screenshot — a grid of biomes, or a grid of approximate surface
//! heights — and this slides that patch over a bounded region looking for
//! where it fits.
//!
//! Two things keep this honest:
//!
//! * **The region must be bounded.** The world is ~60 million blocks across;
//!   at one sample per block that is 3.6e15 positions, which is not a search,
//!   it is a geological era. Modes 6, 8 and 11 exist to produce the box this
//!   mode consumes.
//! * **Heights are approximate.** cubiomes' `mapApproxHeight` estimates the
//!   surface rather than running full terrain generation, so height matching
//!   is done with a tolerance and reported as a ranked list, never as a single
//!   confident answer.

use anyhow::{Result, bail};
use cubiomes::enums::BiomeID;
use rayon::prelude::*;

use crate::grid::ValueGrid;
use crate::session::{BBox, Session};
use crate::ui;
use crate::worldgen::{Version, WorldGen};

/// Refuse searches larger than this many sample positions.
///
/// At roughly a million samples/second this is about a five-minute ceiling,
/// which is the point where the user should narrow the box instead.
pub const MAX_SAMPLES: i64 = 400_000_000;

/// How many blocks one grid cell covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellScale {
    Block = 1,
    Quad = 4,
    Chunk = 16,
}

impl CellScale {
    pub fn blocks(&self) -> i32 {
        *self as i32
    }

    pub fn label(&self) -> &'static str {
        match self {
            CellScale::Block => "1 block per cell (finest, slowest)",
            CellScale::Quad => "4 blocks per cell (cubiomes' native biome scale)",
            CellScale::Chunk => "16 blocks per cell (one chunk, fastest)",
        }
    }
}

/// A transcribed biome patch. `None` cells are unknown and score nothing.
#[derive(Debug, Clone)]
pub struct BiomePattern {
    pub cells: Vec<Vec<Option<BiomeID>>>,
}

impl BiomePattern {
    pub fn width(&self) -> usize {
        self.cells.first().map(|r| r.len()).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.cells.len()
    }

    pub fn known_count(&self) -> usize {
        self.cells.iter().flatten().filter(|c| c.is_some()).count()
    }

    pub fn known_cells(&self) -> impl Iterator<Item = (i32, i32, BiomeID)> + '_ {
        self.cells.iter().enumerate().flat_map(|(row, cells)| {
            cells
                .iter()
                .enumerate()
                .filter_map(move |(col, c)| c.map(|b| (col as i32, row as i32, b)))
        })
    }

    /// Parses whitespace-separated biome names, `?` for unknown.
    ///
    /// Names are matched leniently — case-insensitively, and with spaces or
    /// hyphens treated as underscores — because nobody transcribing a
    /// screenshot should have to remember whether it is `snowy_taiga` or
    /// `Snowy Taiga`.
    pub fn parse(lines: &[String], version: Version) -> Result<BiomePattern> {
        let mut rows: Vec<Vec<Option<BiomeID>>> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let mut row = Vec::new();
            for tok in line.split_whitespace() {
                if tok == "?" || tok == "-" {
                    row.push(None);
                    continue;
                }
                row.push(Some(parse_biome(tok, version)?));
            }
            rows.push(row);
        }

        if rows.is_empty() {
            bail!("the pattern is empty");
        }
        let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for row in &mut rows {
            row.resize(width, None);
        }

        let p = BiomePattern { cells: rows };
        if p.known_count() == 0 {
            bail!("the pattern has no known biomes, so it constrains nothing");
        }
        Ok(p)
    }

    pub fn from_file(path: &str, version: Version) -> Result<BiomePattern> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
        BiomePattern::parse(
            &text.lines().map(|l| l.to_string()).collect::<Vec<_>>(),
            version,
        )
    }
}

/// The name cubiomes gives a biome in a particular version, if it exists there.
///
/// `BiomeID` deliberately implements neither `FromStr` nor `Display`, because
/// several biomes were renamed in 1.18 (`stone_shore` became `stony_shore`,
/// and so on) so the mapping is only meaningful with a version in hand. The
/// safe wrapper's `to_mc_biome_str` asserts on a null return; we call the C
/// function ourselves so an unknown id is an `Option`, not a panic.
pub fn biome_name(b: BiomeID, version: Version) -> Option<&'static str> {
    use cubiomes_sys::num_traits::ToPrimitive;
    let id = b.to_i32()?;
    // SAFETY: `biome2str` is a pure lookup taking two ints and returning either
    // a pointer to a static string or null; we check for null before reading.
    let ptr = unsafe { cubiomes_sys::biome2str(version.mc() as i32, id) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null, and cubiomes returns pointers into static storage.
    unsafe { std::ffi::CStr::from_ptr(ptr) }.to_str().ok()
}

/// Every biome that exists in `version`, as `(name, id)`.
///
/// Built by walking the id space rather than hardcoding a list, so it stays
/// correct as cubiomes adds biomes.
pub fn biome_table(version: Version) -> Vec<(&'static str, BiomeID)> {
    use cubiomes_sys::num_traits::FromPrimitive;
    let mut out = Vec::new();
    for id in 0..512 {
        let Some(b) = BiomeID::from_i32(id) else {
            continue;
        };
        if let Some(name) = biome_name(b, version) {
            out.push((name, b));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out.dedup_by_key(|(n, _)| *n);
    out
}

/// Resolves a biome name, tolerating the spellings people actually type.
pub fn parse_biome(token: &str, version: Version) -> Result<BiomeID> {
    use cubiomes_sys::num_traits::FromPrimitive;

    let normalised = token.trim().to_lowercase().replace([' ', '-'], "_");

    // Numeric ids are accepted too, since some tools print them.
    if let Ok(n) = normalised.parse::<i32>()
        && let Some(b) = BiomeID::from_i32(n)
    {
        return Ok(b);
    }

    for (name, b) in biome_table(version) {
        if name.to_lowercase() == normalised {
            return Ok(b);
        }
    }
    bail!(
        "{token:?} is not a biome in {} — note some biomes were renamed in 1.18",
        version.label()
    )
}

/// One candidate placement of the pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    pub x: i32,
    pub z: i32,
    /// Fraction matched for biomes, or mean absolute error for heights.
    pub score: f64,
}

/// Searches `area` for placements of a biome pattern.
pub fn search_biomes(
    version: Version,
    seed: i64,
    pattern: &BiomePattern,
    area: BBox,
    scale: CellScale,
    sample_y: i32,
    min_fraction: f64,
) -> Result<Vec<Match>> {
    let step = scale.blocks();
    let known: Vec<(i32, i32, BiomeID)> = pattern.known_cells().collect();
    let total_known = known.len() as f64;

    let xs: Vec<i32> = (area.min_x..=area.max_x).step_by(step as usize).collect();
    let zs: Vec<i32> = (area.min_z..=area.max_z).step_by(step as usize).collect();

    let mut found: Vec<Match> = zs
        .par_iter()
        .map_init(
            || WorldGen::overworld(version, seed),
            |world, &oz| {
                let mut local = Vec::new();
                for &ox in &xs {
                    let mut hits = 0.0;
                    // Bail as soon as even a perfect run of the remaining
                    // cells could not reach the threshold.
                    let mut checked = 0.0;
                    for (dx, dz, want) in &known {
                        let bx = ox + dx * step;
                        let bz = oz + dz * step;
                        if let Ok(got) = world.biome_at(bx, sample_y, bz)
                            && got == *want
                        {
                            hits += 1.0;
                        }
                        checked += 1.0;
                        if hits + (total_known - checked) < min_fraction * total_known {
                            break;
                        }
                    }
                    let fraction = hits / total_known;
                    if fraction >= min_fraction {
                        local.push(Match {
                            x: ox,
                            z: oz,
                            score: fraction,
                        });
                    }
                }
                local
            },
        )
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        });

    // Best fraction first, then nearest origin for a stable order.
    found.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.x, a.z).cmp(&(b.x, b.z)))
    });
    Ok(found)
}

/// Searches `area` for placements of a height pattern, ranked by mean absolute
/// error in blocks.
pub fn search_heights(
    version: Version,
    seed: i64,
    pattern: &ValueGrid,
    area: BBox,
    scale: CellScale,
    tolerance: f64,
) -> Result<Vec<Match>> {
    let step = scale.blocks();
    let known: Vec<(i32, i32, f64)> = pattern.known_cells().collect();
    if known.is_empty() {
        bail!("the height pattern has no known values");
    }
    let n = known.len() as f64;

    let xs: Vec<i32> = (area.min_x..=area.max_x).step_by(step as usize).collect();
    let zs: Vec<i32> = (area.min_z..=area.max_z).step_by(step as usize).collect();

    let mut found: Vec<Match> = zs
        .par_iter()
        .map_init(
            || WorldGen::overworld(version, seed),
            |world, &oz| {
                let mut local = Vec::new();
                for &ox in &xs {
                    let mut total_err = 0.0;
                    let mut ok = true;
                    for (dx, dz, want) in &known {
                        let bx = ox + dx * step;
                        let bz = oz + dz * step;
                        let Ok(got) = world.surface_height_at(bx, bz) else {
                            ok = false;
                            break;
                        };
                        total_err += (got as f64 - want).abs();
                        // Give up early once the running mean cannot recover.
                        if total_err / n > tolerance {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        let mae = total_err / n;
                        if mae <= tolerance {
                            local.push(Match {
                                x: ox,
                                z: oz,
                                score: mae,
                            });
                        }
                    }
                }
                local
            },
        )
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        });

    // Lowest error first for heights.
    found.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.x, a.z).cmp(&(b.x, b.z)))
    });
    Ok(found)
}

/// Number of sample positions a search would visit.
pub fn sample_count(area: BBox, scale: CellScale) -> i64 {
    let step = scale.blocks() as i64;
    let w = (area.width() + step - 1) / step;
    let d = (area.depth() + step - 1) / step;
    w * d
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 2 — Overworld Terrain Shape Matcher");
    ui::note("Give it a seed and a patch of what you can see; it finds where that patch fits.");

    let seed = ui::prompt_seed(session)?;
    let version = ui::prompt_version(session)?;

    let kind = ui::select_str(
        "What kind of pattern have you transcribed?",
        &[
            "Biome grid (biome names, '?' for unknown)",
            "Height grid (numbers, '?' for unknown)",
        ],
    )?;

    let scale_idx = ui::select_str(
        "How many blocks does one grid cell cover?",
        &[
            CellScale::Quad.label(),
            CellScale::Block.label(),
            CellScale::Chunk.label(),
        ],
    )?;
    let scale = match scale_idx {
        0 => CellScale::Quad,
        1 => CellScale::Block,
        _ => CellScale::Chunk,
    };

    let area = ui::prompt_bbox(session, "terrain search")?;
    let samples = sample_count(area, scale);
    if samples > MAX_SAMPLES {
        bail!(
            "that region needs {samples} sample positions, over the {MAX_SAMPLES} cap — narrow \
             it (mode 6 from a known structure, mode 11 from a nether coordinate, or mode 8 from \
             a heading) or choose a coarser cell scale"
        );
    }
    ui::note(&format!("{samples} candidate positions to check."));

    let top_n: usize = ui::input_default("How many results to show", 10usize)?;

    let results = if kind == 0 {
        let source = ui::select_str("Where is the pattern?", &["Load from a file", "Type/paste it"])?;
        let pattern = if source == 0 {
            let path: String = ui::input("File path")?;
            BiomePattern::from_file(&path, version)?
        } else {
            ui::note("One row per line, biome names separated by spaces, '?' for unknown.");
            BiomePattern::parse(&ui::read_block("Pattern:")?, version)?
        };
        ui::note(&format!(
            "{}x{} pattern, {} known cells.",
            pattern.width(),
            pattern.height(),
            pattern.known_count()
        ));
        let min_fraction: f64 = ui::input_default("Minimum match fraction", 0.9)?;
        let sample_y: i32 = ui::input_default("Sample Y (63 is sea level)", 63)?;

        let pb = ui::spinner("matching biomes");
        let r = search_biomes(version, seed, &pattern, area, scale, sample_y, min_fraction)?;
        pb.finish_and_clear();
        r
    } else {
        let source = ui::select_str("Where is the pattern?", &["Load from a file", "Type/paste it"])?;
        let pattern = if source == 0 {
            let path: String = ui::input("File path")?;
            ValueGrid::from_file(&path)?
        } else {
            ui::note("One row per line, heights separated by spaces, '?' for unknown.");
            ValueGrid::parse(&ui::read_block("Pattern:")?)?
        };
        ui::note(&format!(
            "{}x{} pattern, {} known values.",
            pattern.width(),
            pattern.height(),
            pattern.known_count()
        ));
        ui::warn(
            "Heights come from cubiomes' approximate surface estimate, not full terrain \
             generation — treat matches as candidates and keep the tolerance generous.",
        );
        let tolerance: f64 = ui::input_default("Tolerance (mean absolute error, blocks)", 3.0)?;

        let pb = ui::spinner("matching heights");
        let r = search_heights(version, seed, &pattern, area, scale, tolerance)?;
        pb.finish_and_clear();
        r
    };

    println!();
    if results.is_empty() {
        ui::warn("Nothing matched in that region.");
        ui::note(
            "Try a lower match fraction or a wider tolerance, mark uncertain cells as '?', or \
             widen the search box.",
        );
        return Ok(());
    }

    let total = results.len();
    let shown = total.min(top_n);
    ui::success(&format!(
        "{total} candidate placement(s); showing the top {shown}:"
    ));
    for (i, m) in results.iter().take(shown).enumerate() {
        let detail = if kind == 0 {
            format!("{:.0}% of cells match", m.score * 100.0)
        } else {
            format!("mean error {:.2} blocks", m.score)
        };
        println!("  {:>2}. X {:>8}, Z {:>8}   {detail}", i + 1, m.x, m.z);
    }

    if total > 1 {
        ui::warn(&format!(
            "{total} placements fit. Ties are listed in coordinate order, which carries no \
             meaning — transcribe a larger patch to disambiguate rather than trusting the first."
        ));
    }

    if ui::confirm("Store the best match as the session search box?", true)? {
        let best = results[0];
        session.search_box = Some(BBox::around(best.x, best.z, 256));
        ui::success("Stored.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: i64 = 1234;
    const VERSION: Version = Version::V1_21_1;

    /// Lifts a real biome patch out of the generator at a known offset.
    fn harvest_biomes(ox: i32, oz: i32, w: i32, h: i32, step: i32) -> BiomePattern {
        let world = WorldGen::overworld(VERSION, SEED);
        let cells = (0..h)
            .map(|dz| {
                (0..w)
                    .map(|dx| world.biome_at(ox + dx * step, 63, oz + dz * step).ok())
                    .collect()
            })
            .collect();
        BiomePattern { cells }
    }

    #[test]
    fn a_harvested_biome_patch_is_found_where_it_came_from() {
        let (ox, oz) = (1024, -2048);
        let pattern = harvest_biomes(ox, oz, 6, 6, 4);
        let area = BBox::around(ox, oz, 128);
        let hits =
            search_biomes(VERSION, SEED, &pattern, area, CellScale::Quad, 63, 1.0).unwrap();
        assert!(!hits.is_empty(), "the patch was not found at all");
        assert!(
            hits.iter().any(|m| m.x == ox && m.z == oz),
            "expected ({ox}, {oz}) among the hits, got {hits:?}"
        );
        assert!((hits[0].score - 1.0).abs() < 1e-9, "top hit should be exact");
    }

    #[test]
    fn unknown_cells_are_skipped_rather_than_counted_against_a_match() {
        let (ox, oz) = (1024, -2048);
        let mut pattern = harvest_biomes(ox, oz, 5, 5, 4);
        // Blank out a few cells; the true placement must still score 100%.
        pattern.cells[0][0] = None;
        pattern.cells[2][3] = None;
        pattern.cells[4][1] = None;
        assert_eq!(pattern.known_count(), 22);

        let area = BBox::around(ox, oz, 64);
        let hits =
            search_biomes(VERSION, SEED, &pattern, area, CellScale::Quad, 63, 1.0).unwrap();
        assert!(hits.iter().any(|m| m.x == ox && m.z == oz));
    }

    #[test]
    fn a_harvested_height_patch_is_found_within_tolerance() {
        let (ox, oz) = (512, 512);
        let world = WorldGen::overworld(VERSION, SEED);
        let lines: Vec<String> = (0..4)
            .map(|dz| {
                (0..4)
                    .map(|dx| {
                        format!(
                            "{:.1}",
                            world.surface_height_at(ox + dx * 4, oz + dz * 4).unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let pattern = ValueGrid::parse(&lines).unwrap();

        let area = BBox::around(ox, oz, 48);
        let hits = search_heights(VERSION, SEED, &pattern, area, CellScale::Quad, 1.0).unwrap();
        assert!(
            hits.iter().any(|m| m.x == ox && m.z == oz),
            "did not rediscover the height patch origin; got {hits:?}"
        );
        assert!(hits[0].score < 1.0, "top hit should have low error");
    }

    #[test]
    fn heights_outside_tolerance_do_not_match() {
        let (ox, oz) = (512, 512);
        let world = WorldGen::overworld(VERSION, SEED);
        // Same patch, shifted 40 blocks up: within a wide tolerance nothing
        // should match, and it certainly should not match at the true origin.
        let lines: Vec<String> = (0..3)
            .map(|dz| {
                (0..3)
                    .map(|dx| {
                        format!(
                            "{:.1}",
                            world.surface_height_at(ox + dx * 4, oz + dz * 4).unwrap() + 40.0
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        let pattern = ValueGrid::parse(&lines).unwrap();
        let area = BBox::around(ox, oz, 32);
        let hits = search_heights(VERSION, SEED, &pattern, area, CellScale::Quad, 3.0).unwrap();
        assert!(
            !hits.iter().any(|m| m.x == ox && m.z == oz),
            "a 40-block offset should not match within a 3-block tolerance"
        );
    }

    #[test]
    fn biome_names_are_parsed_leniently() {
        assert_eq!(parse_biome("plains", VERSION).unwrap(), BiomeID::plains);
        assert_eq!(parse_biome("Plains", VERSION).unwrap(), BiomeID::plains);
        assert_eq!(parse_biome("snowy taiga", VERSION).unwrap(), BiomeID::snowy_taiga);
        assert_eq!(parse_biome("snowy-taiga", VERSION).unwrap(), BiomeID::snowy_taiga);
        assert!(parse_biome("definitely_not_a_biome", VERSION).is_err());
        // Numeric ids work as well, since some tools print those.
        assert_eq!(parse_biome("1", VERSION).unwrap(), BiomeID::plains);
    }

    #[test]
    fn the_biome_table_is_version_aware() {
        // stone_shore was renamed stony_shore in 1.18, which is exactly why
        // BiomeID has no version-free FromStr.
        assert!(parse_biome("stony_shore", Version::V1_21_1).is_ok());
        assert!(parse_biome("stone_shore", Version::V1_16_5).is_ok());

        // The tables differ in *naming*, not availability: cubiomes' biome2str
        // maps an id to whatever that version calls it, and still names ids the
        // version never generates. So assert on the rename, not on membership.
        let modern = biome_table(Version::V1_21_1);
        let legacy = biome_table(Version::V1_16_5);
        assert!(!modern.is_empty() && !legacy.is_empty());
        assert!(modern.iter().any(|(n, _)| *n == "stony_shore"));
        assert!(legacy.iter().any(|(n, _)| *n == "stone_shore"));
        assert!(
            !modern.iter().any(|(n, _)| *n == "stone_shore"),
            "1.21 should use the post-1.18 name only"
        );
    }

    #[test]
    fn empty_and_all_unknown_patterns_are_rejected() {
        assert!(BiomePattern::parse(&[], VERSION).is_err());
        assert!(BiomePattern::parse(&["? ? ?".to_string()], VERSION).is_err());
    }

    #[test]
    fn oversized_regions_are_counted_correctly() {
        let huge = BBox::around(0, 0, 30_000);
        assert!(sample_count(huge, CellScale::Block) > MAX_SAMPLES);
        // The same region is affordable once cells cover a chunk each.
        assert!(sample_count(huge, CellScale::Chunk) < MAX_SAMPLES);

        let small = BBox::around(0, 0, 10);
        assert_eq!(sample_count(small, CellScale::Block), 21 * 21);
    }
}
