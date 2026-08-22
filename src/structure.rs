//! Mode 6 — turning a structure you can already see into a search box.
//!
//! Mode 2 (the terrain matcher) is an exhaustive scan, so what it costs is the
//! area it has to cover. If the seed is already known and the player can name
//! one structure near them, that structure's real position pins them to within
//! a few hundred blocks — which is an area mode 2 can afford. This mode does
//! the pinning and hands the box over.
//!
//! Every position here comes out of cubiomes via
//! [`WorldGen::structures_in_box`]. The region salts and spacings behind those
//! positions were reworked at 1.13 and have kept moving since (villages went
//! from a 32-chunk to a 34-chunk region grid in 1.18, for instance), so none of
//! them are written out anywhere in this file. Getting one subtly wrong would
//! produce a box that confidently excludes the right answer, which is worse
//! than producing no box at all.

use anyhow::{Result, anyhow, bail};
use cubiomes::enums::{Dimension, StructureType};
use cubiomes::generator::BlockPosition;
use cubiomes::structures::StructureRegion;
use indicatif::ProgressBar;

use crate::session::{BBox, Session, StructureObservation};
use crate::ui;
use crate::worldgen::{STRUCTURES, Version, WorldGen, structure_dimension, structure_label};

/// The horizontal scale between the Overworld and the Nether.
///
/// Vanilla stores this as `DimensionType`'s coordinate scale: 8 for the Nether,
/// 1 for the Overworld and the End.
const NETHER_SCALE: i32 = 8;

/// Hard ceiling on how many structure regions one search may probe.
///
/// Each probe is a cubiomes position derivation plus a biome viability check,
/// and the cheap-looking ones are the trap: buried treasure and mineshafts sit
/// on a *one-chunk* grid, so a radius that takes seconds for villages takes
/// hours for them. Refusing up front beats a spinner that never finishes.
const MAX_REGION_PROBES: i64 = 400_000;

/// Default half-width of the box drawn around a single chosen structure.
///
/// 256 blocks is roughly "the structure is still on screen at render distance
/// 16", which is the situation this mode is built for.
const DEFAULT_PICK_RADIUS: i32 = 256;

/// Default padding added around the box that encloses every result.
const DEFAULT_ALL_MARGIN: i32 = 256;

/// How many results to print, and to offer individually in the picker.
///
/// A wide village search can return hundreds; a select list that long is not
/// usable, and neither is the printout.
const MAX_LISTED: usize = 40;

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 6 — structure-relative search narrower");
    ui::note("Name a seed and a structure you can see, and this hands mode 2 a box");
    ui::note("small enough for it to scan exhaustively.");

    let seed = ui::prompt_seed(session)?;
    let version = ui::prompt_version(session)?;

    let labels: Vec<String> = STRUCTURES
        .iter()
        .map(|(_, name, dim)| format!("{name}  [{}]", dimension_label(*dim)))
        .collect();
    let choice = ui::select("Structure type", &labels)?;
    let structure = STRUCTURES[choice].0;
    let label = structure_label(structure);
    let dimension = structure_dimension(structure);

    ui::note(&format!(
        "{label} generates in the {}, so that is the dimension being searched.",
        dimension_label(dimension)
    ));
    if dimension != Dimension::DIM_OVERWORLD {
        ui::warn(&format!(
            "Give the centre in {} coordinates — that is the space these positions live in.",
            dimension_label(dimension)
        ));
    }

    let centre_x: i32 = ui::input_default("Search centre X", 0)?;
    let centre_z: i32 = ui::input_default("Search centre Z", 0)?;
    let radius: i32 = ui::input_default("Search radius (blocks)", 2000)?;
    if radius <= 0 {
        bail!("the search radius has to be positive");
    }

    let area = clamp_to_world(BBox::around(centre_x, centre_z, radius), version);
    let mut world = WorldGen::new(version, seed, dimension);

    // This both fetches the region size and settles whether the structure
    // exists in this version at all, before any real work starts.
    let region_blocks = region_size_blocks(&mut world, structure)?;
    let probes = region_probe_count(region_blocks, area);
    if probes > MAX_REGION_PROBES {
        bail!(
            "a {radius}-block radius around ({centre_x}, {centre_z}) spans {probes} {label} \
             regions, and this mode caps a single search at {MAX_REGION_PROBES}. {label} sits \
             on a {region_blocks}-block region grid, so try a radius of {} or less — or search \
             in a couple of smaller passes.",
            suggested_max_radius(region_blocks)
        );
    }

    ui::note(&format!(
        "{label} uses a {region_blocks}-block region grid; that is {probes} region probe(s)."
    ));

    let columns = region_columns(region_blocks, area);
    let pb = ui::progress_bar(columns as u64, "scanning region columns");
    let found = enumerate_structures(&mut world, structure, area, region_blocks, Some(&pb));
    pb.finish_and_clear();
    let mut found = found?;

    if found.is_empty() {
        ui::warn(&format!("No {label} generates in {area}."));
        ui::note(
            "Widen the radius, or double-check the version — spacing and salts move between \
             versions, and a 1.16 search will not find a 1.18 village layout.",
        );
        ui::pause();
        return Ok(());
    }

    sort_nearest_first(&mut found, centre_x, centre_z);
    ui::success(&format!("{} {label}(s) found in {area}", found.len()));
    print_results(&found, centre_x, centre_z, dimension);

    let stored = choose_box(&found, centre_x, centre_z, dimension)?;
    match stored {
        Some(b) => {
            session.search_box = Some(b);
            ui::success(&format!("Search box stored: {b}"));
            ui::note("Mode 2 will offer this box as its default the next time you run it.");
        }
        None => ui::note("No search box stored; the session's existing box (if any) is untouched."),
    }

    offer_to_record(session, &found, structure, label)?;

    ui::pause();
    Ok(())
}

/// Asks the user which result to build a box around, and builds it.
fn choose_box(
    found: &[BlockPosition],
    centre_x: i32,
    centre_z: i32,
    dimension: Dimension,
) -> Result<Option<BBox>> {
    let listed = found.len().min(MAX_LISTED);
    let mut options: Vec<String> = Vec::with_capacity(listed + 2);
    options.push(format!(
        "All {} of them — the tight box enclosing every result",
        found.len()
    ));
    for (i, p) in found.iter().take(listed).enumerate() {
        options.push(format!(
            "#{}: ({}, {}) — {:.0} blocks away",
            i + 1,
            p.x,
            p.z,
            distance(centre_x, centre_z, *p)
        ));
    }
    options.push("Skip — don't produce a box".to_string());

    let pick = ui::select("Narrow around which result?", &options)?;

    let produced = if pick == 0 {
        let margin: i32 = ui::input_default("Margin around the enclosing box", DEFAULT_ALL_MARGIN)?;
        if margin < 0 {
            bail!("the margin cannot be negative");
        }
        enclosing_box(found, margin)
    } else if pick <= listed {
        let p = found[pick - 1];
        let r: i32 = ui::input_default("Box radius around it", DEFAULT_PICK_RADIUS)?;
        if r <= 0 {
            bail!("the box radius has to be positive");
        }
        Some(BBox::around(p.x, p.z, r))
    } else {
        None
    };

    let Some(mut b) = produced else {
        return Ok(None);
    };

    // A box built from a Nether or End structure is in that dimension's own
    // coordinates. Mode 2 reads Overworld terrain, so handing it a raw Nether
    // box would be off by a factor of eight without saying so.
    match dimension {
        Dimension::DIM_NETHER => {
            ui::note("This box is in Nether coordinates, and mode 2 matches Overworld terrain.");
            if ui::confirm("Convert it to Overworld coordinates (x8) before storing?", true)? {
                b = scale_box(b, NETHER_SCALE);
                ui::note("Converted. Note that x8 also multiplies the uncertainty by eight.");
            }
        }
        Dimension::DIM_END => {
            ui::warn(
                "This box is in End coordinates. The End is 1:1 with the Overworld numerically, \
                 but it is not the same terrain — only useful if you are matching End terrain.",
            );
        }
        _ => {}
    }

    Ok(Some(b))
}

/// Offers to file the found positions as [`StructureObservation`]s for mode 9.
fn offer_to_record(
    session: &mut Session,
    found: &[BlockPosition],
    structure: StructureType,
    label: &str,
) -> Result<()> {
    ui::note("These positions can also be filed as structure observations for mode 9.");
    ui::note(
        "Be clear on what they are worth, though: they were computed from the seed you just \
         entered, not read off a screen. As evidence about that seed they are circular — they \
         agree with it by construction, so they can never confirm it.",
    );
    ui::note(
        "Their real use runs the other way. Mode 9 tests candidate seeds against stored \
         observations, so a position recorded here can rule out a different seed you are \
         weighing up.",
    );

    if !ui::confirm(
        &format!("Record {} {label} position(s) as observations?", found.len()),
        false,
    )? {
        return Ok(());
    }

    for p in found {
        session.structures.push(StructureObservation {
            structure,
            x: p.x,
            z: p.z,
        });
    }
    ui::success(&format!(
        "{} recorded; the session now holds {} structure observation(s).",
        found.len(),
        session.structures.len()
    ));
    Ok(())
}

fn print_results(found: &[BlockPosition], centre_x: i32, centre_z: i32, dimension: Dimension) {
    let pair_header = match dimension {
        Dimension::DIM_OVERWORLD => "nether (X/8, Z/8)",
        Dimension::DIM_NETHER => "overworld (X*8, Z*8)",
        _ => "",
    };
    println!(
        "{}",
        format!(
            "  {:>4}  {:>10}  {:>10}  {:>9}  {}",
            "#", "X", "Z", "distance", pair_header
        )
        .trim_end()
    );
    for (i, p) in found.iter().take(MAX_LISTED).enumerate() {
        let pair = match travel_pair(dimension, p.x, p.z) {
            Some((px, pz)) => format!("{px}, {pz}"),
            None => "—".to_string(),
        };
        println!(
            "  {:>4}  {:>10}  {:>10}  {:>9.0}  {}",
            i + 1,
            p.x,
            p.z,
            distance(centre_x, centre_z, *p),
            pair
        );
    }
    if found.len() > MAX_LISTED {
        ui::note(&format!(
            "… and {} more; only the nearest {MAX_LISTED} are shown.",
            found.len() - MAX_LISTED
        ));
    }
    if dimension == Dimension::DIM_END {
        ui::note("The End has no coordinate scaling, so there is no paired coordinate to show.");
    }
}

/// The Nether coordinate an Overworld position links to.
///
/// The division **floors**; it does not truncate. Minecraft derives the linked
/// position with `Math.floor(x / coordinateScale)` (the Nether's coordinate
/// scale being 8), so -1290 pairs with -162, not the -161 that C-style integer
/// division would hand back. `div_euclid` matches floor here because the
/// divisor is positive.
///
/// This is the single most repeated off-by-one in portal-linking maths, and it
/// only ever shows up in the negative quadrant — which is exactly where nobody
/// tests.
pub fn nether_pair(x: i32, z: i32) -> (i32, i32) {
    (x.div_euclid(NETHER_SCALE), z.div_euclid(NETHER_SCALE))
}

/// The Overworld coordinate a Nether position links to.
pub fn overworld_pair(x: i32, z: i32) -> (i32, i32) {
    (
        x.saturating_mul(NETHER_SCALE),
        z.saturating_mul(NETHER_SCALE),
    )
}

/// The counterpart coordinate in whichever dimension you would travel through.
///
/// `None` for the End, which has no coordinate scaling — an End position is
/// numerically the same in the Overworld, so pairing it says nothing.
pub fn travel_pair(dimension: Dimension, x: i32, z: i32) -> Option<(i32, i32)> {
    match dimension {
        Dimension::DIM_OVERWORLD => Some(nether_pair(x, z)),
        Dimension::DIM_NETHER => Some(overworld_pair(x, z)),
        _ => None,
    }
}

/// This structure's region grid size in blocks, and a version check on the way.
///
/// The size is needed twice before any enumeration starts: to budget the probe
/// count, and to slice the box for the progress bar. The version check is done
/// by asking [`WorldGen::structures_in_box`] for a single-block box, purely
/// because cubiomes' own error for an unsupported structure is a bare
/// `CubiomesError` that says nothing about why — worldgen's wrapper turns it
/// into a sentence naming the structure and the version.
fn region_size_blocks(world: &mut WorldGen, structure: StructureType) -> Result<i32> {
    world.structures_in_box(structure, 0, 0, 0, 0)?;
    let region = StructureRegion::new(0, 0, world.version().mc(), structure).map_err(|e| {
        anyhow!(
            "could not read the region size for {}: {e:?}",
            structure_label(structure)
        )
    })?;
    Ok(region.region_size_blocks())
}

/// Number of structure regions a box overlaps.
fn region_probe_count(region_blocks: i32, area: BBox) -> i64 {
    let rb = region_blocks as i64;
    let cols = region_columns(region_blocks, area);
    let rows = ((area.max_z as i64).div_euclid(rb) - (area.min_z as i64).div_euclid(rb) + 1).max(0);
    cols * rows
}

fn region_columns(region_blocks: i32, area: BBox) -> i64 {
    let rb = region_blocks as i64;
    ((area.max_x as i64).div_euclid(rb) - (area.min_x as i64).div_euclid(rb) + 1).max(0)
}

/// The largest radius that stays inside [`MAX_REGION_PROBES`], for the error
/// message to suggest.
fn suggested_max_radius(region_blocks: i32) -> i32 {
    let regions_per_side = (MAX_REGION_PROBES as f64).sqrt().floor() as i64;
    let span = (regions_per_side / 2) * region_blocks as i64;
    span.clamp(region_blocks as i64, i32::MAX as i64) as i32
}

/// Every structure of one type inside `area`, enumerated one region column at
/// a time so a progress bar has something to tick.
///
/// Splitting the box on region boundaries returns exactly the same set as one
/// whole-box call, because a generation attempt always lands inside its own
/// region — so each region belongs to exactly one column, and the per-column
/// block filter is the original filter restricted to that column. The
/// `slicing_by_region_column_matches_one_whole_box_call` test is what keeps
/// that assumption honest.
fn enumerate_structures(
    world: &mut WorldGen,
    structure: StructureType,
    area: BBox,
    region_blocks: i32,
    progress: Option<&ProgressBar>,
) -> Result<Vec<BlockPosition>> {
    let rb = region_blocks as i64;
    let r_min_x = (area.min_x as i64).div_euclid(rb);
    let r_max_x = (area.max_x as i64).div_euclid(rb);

    let mut found = Vec::new();
    for rx in r_min_x..=r_max_x {
        let lo = clamp_i32(rx * rb);
        let hi = clamp_i32(rx * rb + rb - 1);
        let strip_min_x = area.min_x.max(lo);
        let strip_max_x = area.max_x.min(hi);
        found.extend(world.structures_in_box(
            structure,
            strip_min_x,
            area.min_z,
            strip_max_x,
            area.max_z,
        )?);
        if let Some(pb) = progress {
            pb.inc(1);
        }
    }
    Ok(found)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn distance_sq(cx: i32, cz: i32, p: BlockPosition) -> i64 {
    let dx = p.x as i64 - cx as i64;
    let dz = p.z as i64 - cz as i64;
    dx * dx + dz * dz
}

fn distance(cx: i32, cz: i32, p: BlockPosition) -> f64 {
    (distance_sq(cx, cz, p) as f64).sqrt()
}

/// Sorts nearest-first, comparing squared distances so no float rounding gets
/// a say, and tie-breaking on the coordinates so the listing is stable.
fn sort_nearest_first(found: &mut [BlockPosition], cx: i32, cz: i32) {
    found.sort_by_key(|p| (distance_sq(cx, cz, *p), p.x, p.z));
}

/// The tight box around every position, grown by `margin` on all four sides.
fn enclosing_box(positions: &[BlockPosition], margin: i32) -> Option<BBox> {
    let first = positions.first()?;
    let mut b = BBox {
        min_x: first.x,
        min_z: first.z,
        max_x: first.x,
        max_z: first.z,
    };
    for p in positions {
        b.min_x = b.min_x.min(p.x);
        b.max_x = b.max_x.max(p.x);
        b.min_z = b.min_z.min(p.z);
        b.max_z = b.max_z.max(p.z);
    }
    Some(BBox {
        min_x: b.min_x.saturating_sub(margin),
        min_z: b.min_z.saturating_sub(margin),
        max_x: b.max_x.saturating_add(margin),
        max_z: b.max_z.saturating_add(margin),
    })
}

/// Multiplies every corner by `factor`, for converting a Nether box into
/// Overworld coordinates.
fn scale_box(b: BBox, factor: i32) -> BBox {
    BBox {
        min_x: b.min_x.saturating_mul(factor),
        min_z: b.min_z.saturating_mul(factor),
        max_x: b.max_x.saturating_mul(factor),
        max_z: b.max_z.saturating_mul(factor),
    }
}

/// Clips a box to the world border, so a silly radius cannot inflate the probe
/// count with regions that contain no world.
fn clamp_to_world(area: BBox, version: Version) -> BBox {
    let border = version.world_border();
    BBox {
        min_x: area.min_x.clamp(-border, border),
        min_z: area.min_z.clamp(-border, border),
        max_x: area.max_x.clamp(-border, border),
        max_z: area.max_z.clamp(-border, border),
    }
}

fn dimension_label(d: Dimension) -> &'static str {
    match d {
        Dimension::DIM_OVERWORLD => "Overworld",
        Dimension::DIM_NETHER => "Nether",
        Dimension::DIM_END => "End",
        _ => "unknown dimension",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_VERSION: Version = Version::V1_21_1;
    const TEST_SEED: i64 = 1234;

    fn villages_within(radius: i32) -> (BBox, Vec<BlockPosition>) {
        let area = BBox::around(0, 0, radius);
        let mut world = WorldGen::new(TEST_VERSION, TEST_SEED, Dimension::DIM_OVERWORLD);
        let rb = region_size_blocks(&mut world, StructureType::Village).unwrap();
        let found =
            enumerate_structures(&mut world, StructureType::Village, area, rb, None).unwrap();
        (area, found)
    }

    #[test]
    fn villages_are_found_and_every_one_lands_inside_the_box() {
        let (area, found) = villages_within(3000);
        assert!(
            !found.is_empty(),
            "expected at least one village within 3000 blocks of origin on seed {TEST_SEED}"
        );
        for p in &found {
            assert!(
                p.x >= area.min_x && p.x <= area.max_x && p.z >= area.min_z && p.z <= area.max_z,
                "({}, {}) escaped the requested box {area}",
                p.x,
                p.z
            );
        }
    }

    #[test]
    fn results_are_ordered_nearest_first() {
        // Synthetic first, so the ordering is checked even if the seed's real
        // layout ever changes underneath us.
        let mut synthetic = vec![
            BlockPosition::new(900, 0),
            BlockPosition::new(-100, 0),
            BlockPosition::new(0, 300),
        ];
        sort_nearest_first(&mut synthetic, 0, 0);
        assert_eq!(
            synthetic,
            vec![
                BlockPosition::new(-100, 0),
                BlockPosition::new(0, 300),
                BlockPosition::new(900, 0),
            ]
        );

        // Then the real thing, from the centre the user would have typed.
        let (_, mut found) = villages_within(3000);
        assert!(found.len() >= 2, "need two or more results to test ordering");
        sort_nearest_first(&mut found, 0, 0);
        let dists: Vec<i64> = found.iter().map(|p| distance_sq(0, 0, *p)).collect();
        assert!(
            dists.windows(2).all(|w| w[0] <= w[1]),
            "results are not nearest-first: {dists:?}"
        );
    }

    #[test]
    fn nether_pairing_floors_rather_than_truncating() {
        // The classic off-by-one: -1290 / 8 is -161.25, and Minecraft floors.
        assert_eq!(nether_pair(-1290, -1290), (-162, -162));
        // Rust's `/` truncates toward zero, which is the wrong answer here.
        assert_eq!(-1290 / 8, -161);
        assert_ne!(nether_pair(-1290, 0).0, -1290 / 8);

        assert_eq!(nether_pair(1290, 1290), (161, 161));
        assert_eq!(nether_pair(0, 0), (0, 0));
        assert_eq!(nether_pair(-1, -8), (-1, -1));
        assert_eq!(nether_pair(-9, -16), (-2, -2));
        assert_eq!(nether_pair(7, -7), (0, -1));

        // And the reverse direction, plus the End's lack of one.
        assert_eq!(overworld_pair(-162, 63), (-1296, 504));
        assert_eq!(
            travel_pair(Dimension::DIM_OVERWORLD, -1290, -1290),
            Some((-162, -162))
        );
        assert_eq!(
            travel_pair(Dimension::DIM_NETHER, -162, -162),
            Some((-1296, -1296))
        );
        assert_eq!(travel_pair(Dimension::DIM_END, -1290, -1290), None);
    }

    #[test]
    fn a_structure_that_predates_the_version_is_an_error() {
        // Ancient cities arrived with the Deep Dark in 1.19, and cubiomes has
        // no structure config for them before MC_1_19_2. Asking for them in
        // 1.8.9 has to fail loudly — an empty list would read as "none nearby",
        // which is a different and much more misleading answer.
        let mut world = WorldGen::new(Version::V1_8_9, TEST_SEED, Dimension::DIM_OVERWORLD);
        assert!(
            region_size_blocks(&mut world, StructureType::Ancient_City).is_err(),
            "expected an error for an ancient city in 1.8.9"
        );

        let mut world = WorldGen::new(Version::V1_8_9, TEST_SEED, Dimension::DIM_OVERWORLD);
        let err = world
            .structures_in_box(StructureType::Ancient_City, -500, -500, 500, 500)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Ancient city") && err.contains("1.8.9"),
            "the error should name the structure and version, got: {err}"
        );

        // Sanity check the other side of the boundary: 1.19.2 is fine.
        let mut world = WorldGen::new(Version::V1_19_2, TEST_SEED, Dimension::DIM_OVERWORLD);
        assert!(region_size_blocks(&mut world, StructureType::Ancient_City).is_ok());
    }

    #[test]
    fn slicing_by_region_column_matches_one_whole_box_call() {
        // enumerate_structures splits the box on region boundaries so the
        // progress bar can move. That is only equivalent because a generation
        // attempt always lands inside its own region; this test is the guard
        // on that assumption, across three different region grid sizes.
        let area = BBox::around(137, -409, 800); // deliberately not region-aligned
        for structure in [
            StructureType::Village,
            StructureType::Shipwreck,
            StructureType::Mineshaft,
        ] {
            let mut world = WorldGen::new(TEST_VERSION, TEST_SEED, Dimension::DIM_OVERWORLD);
            let rb = region_size_blocks(&mut world, structure).unwrap();
            let sliced = enumerate_structures(&mut world, structure, area, rb, None).unwrap();
            let whole = world
                .structures_in_box(structure, area.min_x, area.min_z, area.max_x, area.max_z)
                .unwrap();

            let mut a: Vec<(i32, i32)> = sliced.iter().map(|p| (p.x, p.z)).collect();
            let mut b: Vec<(i32, i32)> = whole.iter().map(|p| (p.x, p.z)).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(
                a,
                b,
                "sliced and whole-box enumeration disagree for {}",
                structure_label(structure)
            );
        }
    }

    #[test]
    fn the_enclosing_box_covers_every_position_plus_the_margin() {
        let ps = vec![
            BlockPosition::new(-100, 40),
            BlockPosition::new(300, -250),
            BlockPosition::new(0, 0),
        ];
        let b = enclosing_box(&ps, 32).unwrap();
        assert_eq!((b.min_x, b.max_x, b.min_z, b.max_z), (-132, 332, -282, 72));
        for p in &ps {
            assert!(p.x >= b.min_x && p.x <= b.max_x && p.z >= b.min_z && p.z <= b.max_z);
        }
        // A zero margin still has to contain the extremes themselves.
        let tight = enclosing_box(&ps, 0).unwrap();
        assert_eq!((tight.min_x, tight.max_x), (-100, 300));
        assert!(enclosing_box(&[], 32).is_none());
    }

    #[test]
    fn absurd_radii_are_refused_before_any_work_starts() {
        // Mineshafts sit on a one-chunk grid, so +/-50k blocks is millions of
        // probes — the case the cap exists for.
        let big = BBox::around(0, 0, 50_000);
        assert!(region_probe_count(16, big) > MAX_REGION_PROBES);
        // Villages over the same span are a 34-chunk grid and are affordable.
        assert!(region_probe_count(544, big) <= MAX_REGION_PROBES);
        assert!(suggested_max_radius(16) < 50_000);
        assert!(suggested_max_radius(544) > 50_000);

        // The count itself: a box spanning exactly two regions each way.
        let two_by_two = BBox {
            min_x: 0,
            min_z: 0,
            max_x: 1000,
            max_z: 1000,
        };
        assert_eq!(region_columns(544, two_by_two), 2);
        assert_eq!(region_probe_count(544, two_by_two), 4);
        // Negative coordinates must floor, not truncate, or the column left of
        // the axis gets counted twice.
        let across_origin = BBox {
            min_x: -1,
            min_z: -1,
            max_x: 1,
            max_z: 1,
        };
        assert_eq!(region_probe_count(544, across_origin), 4);
    }

    #[test]
    fn boxes_are_clipped_to_the_world_border_and_scale_by_eight() {
        let border = TEST_VERSION.world_border();
        let huge = clamp_to_world(BBox::around(0, 0, i32::MAX), TEST_VERSION);
        assert_eq!((huge.min_x, huge.max_x), (-border, border));

        let nether = BBox::around(-162, 63, 256);
        let overworld = scale_box(nether, NETHER_SCALE);
        assert_eq!(overworld.min_x, (-162 - 256) * 8);
        assert_eq!(overworld.max_z, (63 + 256) * 8);
    }

    #[test]
    fn nether_structures_are_searched_in_the_nether() {
        // Fortresses only exist in DIM_NETHER, so building the generator for
        // the wrong dimension would silently return nothing. This checks the
        // dimension lookup the mode relies on, and that the search works.
        assert_eq!(
            structure_dimension(StructureType::Fortress),
            Dimension::DIM_NETHER
        );
        let mut world = WorldGen::new(TEST_VERSION, TEST_SEED, Dimension::DIM_NETHER);
        let rb = region_size_blocks(&mut world, StructureType::Fortress).unwrap();
        let area = BBox::around(0, 0, 1500);
        let found =
            enumerate_structures(&mut world, StructureType::Fortress, area, rb, None).unwrap();
        assert!(
            !found.is_empty(),
            "expected a nether fortress within 1500 blocks of the nether origin"
        );
    }
}

