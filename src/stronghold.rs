//! Mode 10 — locating a stronghold from eye-of-ender throws.
//!
//! # Ring structure
//!
//! Java Edition places 128 strongholds in 8 concentric rings around the world
//! origin. Counts and block-distance bands, from the Minecraft Wiki's
//! "Stronghold" page:
//!
//! | Ring | Count | Distance (blocks) |
//! |-----:|------:|------------------:|
//! |    1 |     3 |     1,280 – 2,816 |
//! |    2 |     6 |     4,352 – 5,888 |
//! |    3 |    10 |     7,424 – 8,960 |
//! |    4 |    15 |   10,496 – 12,032 |
//! |    5 |    21 |   13,568 – 15,104 |
//! |    6 |    28 |   16,640 – 18,176 |
//! |    7 |    36 |   19,712 – 21,248 |
//! |    8 |     9 |   22,784 – 24,320 |
//!
//! Within a ring the strongholds sit at roughly equal angles from a single
//! random start angle, and each is then nudged up to 112 blocks to land in a
//! suitable biome. That biome snapping is why ring maths alone cannot give an
//! exact position — and why, when the seed *is* known, this mode scores
//! cubiomes' real generated positions instead.
//!
//! # Inference
//!
//! Each throw is a ray, not a point: it fixes a direction but not a distance.
//! We treat the measured yaw as a noisy observation of the true bearing,
//! `measured ~ Normal(true, sigma)`, and combine throws by multiplying
//! likelihoods — which is Bayes' theorem with a flat prior over candidates.
//! Two throws from well-separated positions produce a sharply peaked
//! posterior; one throw leaves an arc.
//!
//! This is the same idea as Ninjabrain-Bot but a simpler model. Notably we do
//! **not** model the "the eye points at the *nearest* stronghold" constraint
//! that its advanced mode uses, nor a calibrated per-player error
//! distribution. Results are reported as ranked candidates with percentages,
//! never as a single certainty.

use anyhow::{Result, bail};

use crate::session::Session;
use crate::ui;
use crate::worldgen::{Version, WorldGen};

/// `(stronghold count, inner radius, outer radius)` per ring, in blocks.
pub const RINGS: [(u32, f64, f64); 8] = [
    (3, 1280.0, 2816.0),
    (6, 4352.0, 5888.0),
    (10, 7424.0, 8960.0),
    (15, 10496.0, 12032.0),
    (21, 13568.0, 15104.0),
    (28, 16640.0, 18176.0),
    (36, 19712.0, 21248.0),
    (9, 22784.0, 24320.0),
];

/// Biome snapping can move a stronghold this far from its ring position.
pub const BIOME_SNAP_BLOCKS: f64 = 112.0;

/// Default measurement error, in degrees, for a yaw read off the F3 screen.
///
/// A deliberately conservative stand-in for the calibrated per-player error
/// distribution a dedicated tool like Ninjabrain-Bot fits. It is exposed as a
/// prompt precisely because it is a modelling choice, not a game constant.
pub const DEFAULT_SIGMA_DEG: f64 = 0.5;

/// One eye-of-ender throw, as read from F3 + C.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Throw {
    pub x: f64,
    pub z: f64,
    /// Minecraft yaw in degrees.
    pub yaw: f64,
}

/// A ranked candidate stronghold position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub x: i32,
    pub z: i32,
    /// Normalised posterior probability over the candidate set.
    pub probability: f64,
    /// Distance from the most recent throw, in blocks.
    pub distance: f64,
}

/// The Minecraft yaw, in degrees, that points from `(px, pz)` to `(tx, tz)`.
///
/// Minecraft's convention: yaw 0 faces +Z (south), 90 faces -X (west), 180
/// faces -Z (north), 270 faces +X (east), so the facing vector is
/// `(dx, dz) = (-sin(yaw), cos(yaw))`. Inverting that gives the atan2 below.
pub fn bearing_to(px: f64, pz: f64, tx: f64, tz: f64) -> f64 {
    normalise_deg((-(tx - px)).atan2(tz - pz).to_degrees())
}

/// The unit facing vector for a yaw, as `(dx, dz)`.
pub fn facing(yaw_deg: f64) -> (f64, f64) {
    let r = yaw_deg.to_radians();
    (-r.sin(), r.cos())
}

/// Wraps to `[0, 360)`.
pub fn normalise_deg(d: f64) -> f64 {
    let m = d % 360.0;
    if m < 0.0 { m + 360.0 } else { m }
}

/// Signed smallest difference between two bearings, in `(-180, 180]`.
pub fn angle_difference(a: f64, b: f64) -> f64 {
    let mut d = (a - b) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d <= -180.0 {
        d += 360.0;
    }
    d
}

/// Which ring a distance from origin falls in, allowing for biome snapping.
pub fn ring_of(distance: f64) -> Option<usize> {
    RINGS.iter().position(|(_, lo, hi)| {
        distance >= lo - BIOME_SNAP_BLOCKS && distance <= hi + BIOME_SNAP_BLOCKS
    })
}

/// Total log-likelihood of a candidate position given every throw.
fn log_likelihood(throws: &[Throw], tx: f64, tz: f64, sigma_deg: f64) -> f64 {
    let mut total = 0.0;
    for t in throws {
        // A throw made from (almost) on top of the target has no meaningful
        // bearing; skip it rather than letting atan2 noise dominate.
        let dist = ((tx - t.x).powi(2) + (tz - t.z).powi(2)).sqrt();
        if dist < 1.0 {
            continue;
        }
        let residual = angle_difference(t.yaw, bearing_to(t.x, t.z, tx, tz));
        total += -0.5 * (residual / sigma_deg).powi(2);
    }
    total
}

/// Do all throws agree on which stronghold is nearest?
///
/// If not, the player crossed a boundary between two strongholds mid-session
/// and the nearest-stronghold constraint cannot be applied.
pub fn throws_agree_on_nearest(version: Version, seed: i64, throws: &[Throw]) -> bool {
    if throws.len() < 2 {
        return true;
    }
    let world = WorldGen::overworld(version, seed);
    let positions = world.strongholds();
    let nearest = |t: &Throw| {
        positions
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = (a.x as f64 - t.x).powi(2) + (a.z as f64 - t.z).powi(2);
                let db = (b.x as f64 - t.x).powi(2) + (b.z as f64 - t.z).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i)
    };
    let first = nearest(&throws[0]);
    throws.iter().all(|t| nearest(t) == first)
}

/// Ranks the seed's real stronghold positions against the throws.
///
/// The accurate path: cubiomes gives the true biome-snapped positions, so the
/// only question is which of 128 known points the throws point at.
pub fn rank_known_strongholds(
    version: Version,
    seed: i64,
    throws: &[Throw],
    sigma_deg: f64,
    nearest_only: bool,
    top_n: usize,
) -> Result<Vec<Candidate>> {
    if throws.is_empty() {
        bail!("at least one throw is needed");
    }
    let world = WorldGen::overworld(version, seed);
    let positions = world.strongholds();

    // An eye of ender points at the *nearest* stronghold, so anything that is
    // not nearest to every throw position could not have produced these
    // readings. Without this a far-off stronghold that happens to sit almost
    // along the same bearing stays a serious rival: at seed 1234 a ring-5
    // stronghold lands within 0.6 degrees of both rays and soaks up a third of
    // the posterior. Applying the constraint removes it outright.
    let viable: Vec<bool> = if nearest_only {
        let nearest_per_throw: Vec<usize> = throws
            .iter()
            .map(|t| {
                let mut best = 0usize;
                let mut best_d = f64::MAX;
                for (i, p) in positions.iter().enumerate() {
                    let d = (p.x as f64 - t.x).powi(2) + (p.z as f64 - t.z).powi(2);
                    if d < best_d {
                        best_d = d;
                        best = i;
                    }
                }
                best
            })
            .collect();
        (0..positions.len())
            .map(|i| nearest_per_throw.iter().all(|n| *n == i))
            .collect()
    } else {
        vec![true; positions.len()]
    };

    let any_viable = viable.iter().any(|v| *v);
    let scored: Vec<(f64, i32, i32)> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // If the throws disagree about which stronghold is nearest the
            // player walked past a boundary between them; rather than return
            // nothing, fall back to scoring everything.
            let ll = if any_viable && !viable[i] {
                f64::NEG_INFINITY
            } else {
                log_likelihood(throws, p.x as f64, p.z as f64, sigma_deg)
            };
            (ll, p.x, p.z)
        })
        .collect();

    Ok(normalise(scored, throws, top_n))
}

/// Ranks candidate positions when the seed is unknown, using the ring prior.
///
/// Rather than sweeping the whole 24,320-block disc, this walks a narrow fan
/// around the first throw's bearing — `±4 sigma`, beyond which the likelihood
/// is negligible — and keeps only points that land inside a ring band. That
/// makes the search cheap without changing the answer in any region that
/// carries meaningful probability.
pub fn rank_ring_candidates(
    throws: &[Throw],
    sigma_deg: f64,
    innermost_only: bool,
    top_n: usize,
) -> Result<Vec<Candidate>> {
    let Some(first) = throws.first() else {
        bail!("at least one throw is needed");
    };

    let fan = 4.0 * sigma_deg;
    // One chunk of arc at the far ring is about 0.04 degrees; stepping finer
    // than the reporting resolution buys nothing.
    let angle_step = (sigma_deg / 4.0).max(0.01);
    let distance_step = 16.0;

    let steps = ((2.0 * fan) / angle_step).ceil() as i64;

    // Which is the innermost ring the nominal ray actually reaches?
    let innermost = {
        let (dx, dz) = facing(first.yaw);
        let max_d = RINGS[RINGS.len() - 1].2 + BIOME_SNAP_BLOCKS;
        let mut found = RINGS.len() - 1;
        let mut d = 0.0;
        while d <= max_d {
            let (x, z) = (first.x + dx * d, first.z + dz * d);
            if let Some(r) = ring_of((x * x + z * z).sqrt()) {
                found = r;
                break;
            }
            d += 16.0;
        }
        found
    };

    let mut scored: Vec<(f64, i32, i32)> = Vec::new();

    for i in 0..=steps {
        let yaw = first.yaw - fan + (i as f64) * angle_step;
        let (dx, dz) = facing(yaw);
        let max_d = RINGS[RINGS.len() - 1].2 + BIOME_SNAP_BLOCKS;
        let mut d = 0.0;
        while d <= max_d {
            let x = first.x + dx * d;
            let z = first.z + dz * d;
            let from_origin = (x * x + z * z).sqrt();
            if let Some(ring) = ring_of(from_origin) {
                // The eye points at the nearest stronghold, which from
                // anywhere near spawn is in the innermost ring the ray
                // crosses. Without the seed we cannot verify that, so it is a
                // stated assumption rather than a fact — and it breaks if you
                // have already travelled tens of thousands of blocks out.
                if innermost_only && ring > innermost {
                    d += distance_step;
                    continue;
                }
                // Snap to chunk centres so duplicate hits collapse.
                let cx = (x / 16.0).floor() as i32 * 16 + 8;
                let cz = (z / 16.0).floor() as i32 * 16 + 8;
                scored.push((
                    log_likelihood(throws, cx as f64, cz as f64, sigma_deg),
                    cx,
                    cz,
                ));
            }
            d += distance_step;
        }
    }

    if scored.is_empty() {
        bail!(
            "that throw does not cross any stronghold ring — check the yaw sign (F3 yaw is \
             negative for east) and that X/Z are not swapped"
        );
    }

    // Collapse duplicates from overlapping fan rays, keeping the best score.
    scored.sort_by_key(|a| (a.1, a.2));
    scored.dedup_by(|a, b| {
        if a.1 == b.1 && a.2 == b.2 {
            b.0 = b.0.max(a.0);
            true
        } else {
            false
        }
    });

    Ok(normalise(scored, throws, top_n))
}

/// Converts log-likelihoods into a normalised posterior and ranks them.
fn normalise(scored: Vec<(f64, i32, i32)>, throws: &[Throw], top_n: usize) -> Vec<Candidate> {
    let max = scored
        .iter()
        .map(|(l, _, _)| *l)
        .fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        return Vec::new();
    }

    // Subtract the max before exponentiating, or every term underflows to zero.
    let weights: Vec<f64> = scored.iter().map(|(l, _, _)| (l - max).exp()).collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }

    let last = throws.last().copied();
    let mut out: Vec<Candidate> = scored
        .iter()
        .zip(&weights)
        // A zero weight means the candidate was ruled out entirely (the
        // nearest-stronghold constraint sets its log-likelihood to -inf).
        // Listing it at 0.0% would imply it is still in the running.
        .filter(|(_, w)| **w > 0.0)
        .map(|((_, x, z), w)| Candidate {
            x: *x,
            z: *z,
            probability: w / total,
            distance: last
                .map(|t| ((*x as f64 - t.x).powi(2) + (*z as f64 - t.z).powi(2)).sqrt())
                .unwrap_or(0.0),
        })
        .collect();

    out.sort_by(|a, b| {
        b.probability
            .partial_cmp(&a.probability)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(top_n);
    out
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 10 — Stronghold Ring Triangulator");
    ui::note("Throw an eye of ender, then press F3 + C and read your position and yaw.");
    ui::note("One throw narrows it to an arc; two from well-separated spots pin it down.");

    let mut throws: Vec<Throw> = Vec::new();
    loop {
        println!();
        ui::note(&format!("Throw {}", throws.len() + 1));
        let x: f64 = ui::input("  Player X")?;
        let z: f64 = ui::input("  Player Z")?;
        let yaw: f64 = ui::input("  Yaw (F3 'Facing' angle, negative for east)")?;
        throws.push(Throw { x, z, yaw });

        if throws.len() >= 2
            && !ui::confirm("Add another throw?", false)?
        {
            break;
        }
        if throws.len() == 1 && !ui::confirm("Add a second throw? (strongly recommended)", true)? {
            break;
        }
    }

    if throws.len() >= 2 {
        // Two throws from nearly the same spot give nearly the same ray, and
        // the intersection becomes wildly sensitive to measurement error.
        let a = throws[0];
        let b = throws[throws.len() - 1];
        let sep = ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt();
        if sep < 100.0 {
            ui::warn(&format!(
                "Your throws are only {sep:.0} blocks apart. Move at least ~400 blocks \
                 perpendicular to the first throw before the second, or the result will be \
                 very sensitive to a small angle error."
            ));
        }
    }

    let sigma: f64 = ui::input_default("Angle measurement error, sigma in degrees", DEFAULT_SIGMA_DEG)?;
    if sigma <= 0.0 {
        bail!("sigma must be positive");
    }
    let top_n: usize = ui::input_default("How many candidates to show", 5usize)?;

    // The eight-ring layout is a 1.9 thing. Before that a world had three
    // strongholds placed differently, and RINGS does not describe them.
    if let Some(v) = session.version
        && !v.has_stronghold_rings()
    {
        ui::warn(&format!(
            "{} generates only {} stronghold(s), not the 128 across 8 rings this mode's prior \
             assumes. Ring estimates will be wrong; use the seed-based path if you can.",
            v.label(),
            v.stronghold_count()
        ));
    }

    let nearest_only = ui::confirm(
        "Assume the eye pointed at the NEAREST stronghold? (true unless you have travelled very far out)",
        true,
    )?;

    let use_seed = session.seed.is_some()
        && ui::confirm(
            "A seed is known for this session — score the real generated strongholds instead of \
             the ring prior? (much more accurate)",
            true,
        )?;

    let candidates = if use_seed {
        let version = ui::prompt_version(session)?;
        let seed = session.seed.expect("checked above");
        if nearest_only && !throws_agree_on_nearest(version, seed, &throws) {
            ui::warn(
                "Your throws do not agree on which stronghold is nearest — you crossed a \
                 boundary between two of them. Scoring every stronghold instead.",
            );
        }
        rank_known_strongholds(version, seed, &throws, sigma, nearest_only, top_n)?
    } else {
        rank_ring_candidates(&throws, sigma, nearest_only, top_n)?
    };

    println!();
    if candidates.is_empty() {
        ui::warn("No candidate survived. Check the yaw sign and that X/Z are not swapped.");
        return Ok(());
    }

    ui::success("Ranked candidates:");
    for (i, c) in candidates.iter().enumerate() {
        let (nx, nz) = crate::portal::overworld_to_nether(c.x, c.z);
        println!(
            "  {:>2}. X {:>7}, Z {:>7}   {:>5.1}%   {:>6.0} blocks away   (nether {nx}, {nz})",
            i + 1,
            c.x,
            c.z,
            c.probability * 100.0,
            c.distance
        );
    }

    println!();
    if throws.len() == 1 {
        ui::warn(
            "One throw only fixes a direction. These percentages rank points along an arc — \
             they are not a location. Throw again from a few hundred blocks to the side.",
        );
    }
    if !use_seed {
        ui::note(
            "Without a seed these are ring-prior estimates: real strongholds are nudged up to \
             112 blocks by biome snapping, which this model does not predict. Dig within about \
             a chunk of the top candidate and expect to search.",
        );
    }

    let best = candidates[0];
    if ui::confirm("Store the top candidate as the session search box?", true)? {
        session.search_box = Some(crate::session::BBox::around(best.x, best.z, 128));
        ui::success("Stored.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn facing_matches_minecrafts_yaw_convention() {
        // yaw 0 = +Z (south), 90 = -X (west), 180 = -Z (north), 270 = +X (east)
        let (dx, dz) = facing(0.0);
        assert!(close(dx, 0.0, 1e-9) && close(dz, 1.0, 1e-9), "yaw 0 -> +Z");
        let (dx, dz) = facing(90.0);
        assert!(close(dx, -1.0, 1e-9) && close(dz, 0.0, 1e-9), "yaw 90 -> -X");
        let (dx, dz) = facing(180.0);
        assert!(close(dx, 0.0, 1e-9) && close(dz, -1.0, 1e-9), "yaw 180 -> -Z");
        let (dx, dz) = facing(270.0);
        assert!(close(dx, 1.0, 1e-9) && close(dz, 0.0, 1e-9), "yaw 270 -> +X");
    }

    #[test]
    fn bearing_inverts_facing() {
        // Walking one block along `facing(yaw)` must read back as that yaw.
        for yaw in [0.0, 37.5, 90.0, 175.0, 200.0, 300.0, 359.0] {
            let (dx, dz) = facing(yaw);
            let got = bearing_to(0.0, 0.0, dx * 1000.0, dz * 1000.0);
            assert!(
                close(angle_difference(got, yaw), 0.0, 1e-6),
                "yaw {yaw} came back as {got}"
            );
        }
    }

    #[test]
    fn angle_difference_wraps_the_short_way() {
        assert!(close(angle_difference(10.0, 350.0), 20.0, 1e-9));
        assert!(close(angle_difference(350.0, 10.0), -20.0, 1e-9));
        assert!(close(angle_difference(0.0, 180.0), 180.0, 1e-9));
        assert!(close(normalise_deg(-90.0), 270.0, 1e-9));
        assert!(close(normalise_deg(450.0), 90.0, 1e-9));
    }

    #[test]
    fn ring_membership_matches_the_documented_bands() {
        assert_eq!(ring_of(2000.0), Some(0));
        assert_eq!(ring_of(5000.0), Some(1));
        assert_eq!(ring_of(23000.0), Some(7));
        // Between rings there is nothing.
        assert_eq!(ring_of(3500.0), None);
        assert_eq!(ring_of(100.0), None);
        // Biome snapping widens each band a little.
        assert_eq!(ring_of(1280.0 - 50.0), Some(0));
        assert_eq!(ring_of(1280.0 - 200.0), None);
        assert_eq!(RINGS.iter().map(|(n, _, _)| n).sum::<u32>(), 128);
    }

    /// The stronghold an eye of ender thrown here would actually point at.
    fn nearest_stronghold(version: Version, seed: i64, x: f64, z: f64) -> (i32, i32) {
        let world = WorldGen::overworld(version, seed);
        let p = world
            .strongholds()
            .into_iter()
            .min_by(|a, b| {
                let da = (a.x as f64 - x).powi(2) + (a.z as f64 - z).powi(2);
                let db = (b.x as f64 - x).powi(2) + (b.z as f64 - z).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("a seed always has strongholds");
        (p.x, p.z)
    }

    /// Picks a second throw position that shares a nearest stronghold with the
    /// first, offset perpendicular to the sight line the way a player actually
    /// triangulates. Returns `(second position, shared target)`.
    fn second_throw_position(
        version: Version,
        seed: i64,
        a: (f64, f64),
    ) -> ((f64, f64), (i32, i32)) {
        let target = nearest_stronghold(version, seed, a.0, a.1);
        let bearing = bearing_to(a.0, a.1, target.0 as f64, target.1 as f64);
        // Perpendicular to the sight line is where a second throw gains the
        // most information.
        for dist in [400.0, 600.0, 800.0, 300.0, 1000.0] {
            for sign in [1.0, -1.0] {
                let (dx, dz) = facing(bearing + 90.0 * sign);
                let b = (a.0 + dx * dist, a.1 + dz * dist);
                if nearest_stronghold(version, seed, b.0, b.1) == target {
                    return (b, target);
                }
            }
        }
        panic!("no perpendicular offset shared a nearest stronghold with {a:?}");
    }

    #[test]
    fn two_exact_throws_identify_the_real_stronghold() {
        // The end-to-end check. The target has to be the stronghold an eye
        // would really point at, not just any of the 128 — aiming at an
        // arbitrary one describes a throw the game would never produce.
        let version = Version::V1_21_1;
        let seed = 1234;
        let a = (120.0, -300.0);
        let (b, target) = second_throw_position(version, seed, a);

        let throws = vec![
            Throw { x: a.0, z: a.1, yaw: bearing_to(a.0, a.1, target.0 as f64, target.1 as f64) },
            Throw { x: b.0, z: b.1, yaw: bearing_to(b.0, b.1, target.0 as f64, target.1 as f64) },
        ];
        assert!(throws_agree_on_nearest(version, seed, &throws));

        let ranked = rank_known_strongholds(version, seed, &throws, 0.5, true, 5).unwrap();
        assert_eq!((ranked[0].x, ranked[0].z), target, "the true stronghold did not rank first");
        assert!(
            ranked[0].probability > 0.99,
            "expected near-certainty, got {}",
            ranked[0].probability
        );
    }

    #[test]
    fn noisy_throws_still_rank_the_real_stronghold_first() {
        let version = Version::V1_21_1;
        let seed = 987654;
        let a = (-200.0, 150.0);
        let (b, target) = second_throw_position(version, seed, a);

        // Half a degree off in opposite directions — a realistic bad pair.
        let throws = vec![
            Throw {
                x: a.0,
                z: a.1,
                yaw: bearing_to(a.0, a.1, target.0 as f64, target.1 as f64) + 0.5,
            },
            Throw {
                x: b.0,
                z: b.1,
                yaw: bearing_to(b.0, b.1, target.0 as f64, target.1 as f64) - 0.5,
            },
        ];
        let ranked = rank_known_strongholds(version, seed, &throws, 1.0, false, 5).unwrap();
        assert_eq!((ranked[0].x, ranked[0].z), target);
    }

    #[test]
    fn the_ring_prior_puts_the_target_close_to_the_top_without_a_seed() {
        // Same setup, but pretending we do not know the seed. Ring maths cannot
        // reproduce biome snapping, so we only require the best candidate to
        // land near the truth rather than exactly on it.
        let world = WorldGen::overworld(Version::V1_21_1, 1234);
        let target = world.strongholds()[0];

        let a = (100.0, 100.0);
        let b = (-500.0, 800.0);
        let throws = vec![
            Throw {
                x: a.0,
                z: a.1,
                yaw: bearing_to(a.0, a.1, target.x as f64, target.z as f64),
            },
            Throw {
                x: b.0,
                z: b.1,
                yaw: bearing_to(b.0, b.1, target.x as f64, target.z as f64),
            },
        ];
        let ranked = rank_ring_candidates(&throws, 0.5, true, 5).unwrap();
        let best = ranked[0];
        let err = (((best.x - target.x) as f64).powi(2) + ((best.z - target.z) as f64).powi(2)).sqrt();
        assert!(
            err < 200.0,
            "ring estimate was {err:.0} blocks off ({}, {}) vs ({}, {})",
            best.x,
            best.z,
            target.x,
            target.z
        );
    }

    #[test]
    fn the_nearest_constraint_sharpens_the_posterior() {
        // A far stronghold that happens to sit almost along the same bearing
        // is a serious rival on angles alone. Because an eye always points at
        // the nearest one, that rival is physically impossible — and applying
        // the constraint should measurably concentrate the posterior.
        let version = Version::V1_21_1;
        let seed = 1234;
        let a = (120.0, -300.0);
        let (b, target) = second_throw_position(version, seed, a);
        let throws = vec![
            Throw { x: a.0, z: a.1, yaw: bearing_to(a.0, a.1, target.0 as f64, target.1 as f64) },
            Throw { x: b.0, z: b.1, yaw: bearing_to(b.0, b.1, target.0 as f64, target.1 as f64) },
        ];

        let without = rank_known_strongholds(version, seed, &throws, 0.5, false, 5).unwrap();
        let with = rank_known_strongholds(version, seed, &throws, 0.5, true, 5).unwrap();

        assert_eq!((with[0].x, with[0].z), target);
        assert!(
            with[0].probability >= without[0].probability,
            "the constraint should never make the answer less certain ({} vs {})",
            with[0].probability,
            without[0].probability
        );
        // With the constraint on, exactly one stronghold is viable.
        assert_eq!(with.len(), 1, "only the nearest stronghold should survive");
    }

    #[test]
    fn disagreeing_throws_fall_back_instead_of_returning_nothing() {
        // Two throws from opposite corners of the world have different nearest
        // strongholds; the constraint is unsatisfiable, and the honest
        // response is to rank everything rather than return an empty list.
        let version = Version::V1_21_1;
        let seed = 1234;
        let throws = vec![
            Throw { x: 0.0, z: 0.0, yaw: 45.0 },
            Throw { x: 200_000.0, z: 200_000.0, yaw: 200.0 },
        ];
        assert!(!throws_agree_on_nearest(version, seed, &throws));
        let ranked = rank_known_strongholds(version, seed, &throws, 5.0, true, 5).unwrap();
        assert!(!ranked.is_empty(), "fallback should still produce candidates");
    }

    #[test]
    fn probabilities_sum_to_one_and_bad_input_is_refused() {
        let throws = vec![Throw { x: 0.0, z: 0.0, yaw: 45.0 }];
        let ranked = rank_ring_candidates(&throws, 0.5, true, 1000).unwrap();
        let total: f64 = ranked.iter().map(|c| c.probability).sum();
        assert!(total > 0.0 && total <= 1.0 + 1e-9, "total was {total}");

        assert!(rank_ring_candidates(&[], 0.5, true, 5).is_err());
        assert!(rank_known_strongholds(Version::V1_21_1, 1, &[], 0.5, true, 5).is_err());
    }

    #[test]
    fn a_single_throw_leaves_a_spread_rather_than_a_point() {
        // The honest behaviour: one throw must not look like a determination.
        let throws = vec![Throw { x: 0.0, z: 0.0, yaw: 30.0 }];
        let ranked = rank_ring_candidates(&throws, 0.5, true, 20).unwrap();
        assert!(ranked.len() > 1);
        assert!(
            ranked[0].probability < 0.5,
            "one throw should not concentrate probability, got {}",
            ranked[0].probability
        );
    }
}
