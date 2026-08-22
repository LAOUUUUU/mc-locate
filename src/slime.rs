//! Mode 4 — cracking the structure seed from confirmed slime chunks.
//!
//! Slime chunk eligibility is a pure function of the world seed and the chunk
//! coordinates:
//!
//! ```java
//! Random rnd = new Random(
//!     seed + (long) (xPosition * xPosition * 0x4c1906)
//!          + (long) (xPosition * 0x5ac0db)
//!          + (long) (zPosition * zPosition) * 0x4307a7L
//!          + (long) (zPosition * 0x5f24f)
//!     ^ 0x3ad8025f);
//! return rnd.nextInt(10) == 0;
//! ```
//!
//! Two details of that expression are easy to get wrong and both are load
//! bearing:
//!
//! * The trailing `^ 0x3ad8025f` applies to the *whole* sum, because `^` binds
//!   more loosely than `+` in Java. Several summaries of this formula drop the
//!   XOR entirely.
//! * The casts are not uniform. `xPosition * xPosition * 0x4c1906`,
//!   `xPosition * 0x5ac0db` and `zPosition * 0x5f24f` are evaluated in 32-bit
//!   int arithmetic (so they wrap) and only then widened, whereas
//!   `zPosition * zPosition` is widened first and multiplied by `0x4307a7L` in
//!   64-bit. Treating them all the same way gives wrong answers for large
//!   chunk coordinates.
//!
//! Both are pinned down by [`tests::matches_cubiomes_reference`], which checks
//! our implementation against cubiomes' own C `isSlimeChunk` over a wide spread
//! of seeds and coordinates including deliberately large and negative ones.
//!
//! Because `setSeed` masks to 48 bits, slime chunks only ever depend on the
//! low 48 bits of the world seed — so this mode recovers a *structure seed*,
//! which 65,536 different world seeds share.

use anyhow::{Result, bail};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::random::JavaRandom;
use crate::session::{Session, SlimeObservation};
use crate::ui;

/// Size of the structure-seed space.
pub const SEED_SPACE: u64 = 1 << 48;

/// The per-chunk part of the slime hash, which does not depend on the seed and
/// can therefore be precomputed once per observed chunk.
///
/// See the module docs for why the casts differ between the x and z terms.
#[inline]
pub fn slime_offset(chunk_x: i32, chunk_z: i32) -> i64 {
    let x = chunk_x;
    let z = chunk_z;

    let x_sq = x.wrapping_mul(x).wrapping_mul(0x4c1906) as i64;
    let x_lin = x.wrapping_mul(0x5ac0db) as i64;
    let z_sq = (z.wrapping_mul(z) as i64).wrapping_mul(0x4307a7);
    let z_lin = x_lin_zero_check(z);

    x_sq.wrapping_add(x_lin).wrapping_add(z_sq).wrapping_add(z_lin)
}

#[inline]
fn x_lin_zero_check(z: i32) -> i64 {
    z.wrapping_mul(0x5f24f) as i64
}

/// Whether `(chunk_x, chunk_z)` is a slime chunk for this seed.
#[inline]
pub fn is_slime_chunk(seed: i64, chunk_x: i32, chunk_z: i32) -> bool {
    is_slime_chunk_with_offset(seed, slime_offset(chunk_x, chunk_z))
}

/// Hot path: the same test with the per-chunk hash already computed.
#[inline(always)]
pub fn is_slime_chunk_with_offset(seed: i64, offset: i64) -> bool {
    let mixed = seed.wrapping_add(offset) ^ 0x3ad8025f;
    JavaRandom::new(mixed).next_int_bound(10) == 0
}

/// A set of slime observations compiled into a form the brute force can run
/// against cheaply.
#[derive(Debug, Clone)]
pub struct SlimeConstraints {
    /// `(precomputed offset, expected result)`, ordered so the most selective
    /// tests run first.
    tests: Vec<(i64, bool)>,
    positives: usize,
    negatives: usize,
}

impl SlimeConstraints {
    pub fn new(observations: &[SlimeObservation]) -> Result<Self> {
        if observations.is_empty() {
            bail!("no slime observations were given");
        }

        // A chunk claimed to be both slime and not-slime can never be
        // satisfied, and would otherwise just burn hours returning nothing.
        for (i, a) in observations.iter().enumerate() {
            for b in &observations[i + 1..] {
                if a.chunk_x == b.chunk_x && a.chunk_z == b.chunk_z && a.is_slime != b.is_slime {
                    bail!(
                        "chunk ({}, {}) is listed both as a slime chunk and as not a slime chunk",
                        a.chunk_x,
                        a.chunk_z
                    );
                }
            }
        }

        let mut tests: Vec<(i64, bool)> = observations
            .iter()
            .map(|o| (slime_offset(o.chunk_x, o.chunk_z), o.is_slime))
            .collect();

        // Positive observations reject 90% of seeds each; negatives only 10%.
        // Running positives first means almost every candidate dies on the
        // first test, which roughly halves total work.
        tests.sort_by_key(|(_, is_slime)| !*is_slime);

        let positives = observations.iter().filter(|o| o.is_slime).count();
        let negatives = observations.len() - positives;

        Ok(Self {
            tests,
            positives,
            negatives,
        })
    }

    #[inline(always)]
    pub fn accepts(&self, seed: i64) -> bool {
        for (offset, expected) in &self.tests {
            if is_slime_chunk_with_offset(seed, *offset) != *expected {
                return false;
            }
        }
        true
    }

    pub fn len(&self) -> usize {
        self.tests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tests.is_empty()
    }

    /// Expected number of surviving structure seeds across the whole 48-bit
    /// space, assuming the tests behave independently.
    ///
    /// Each positive keeps 1/10 of seeds and each negative keeps 9/10, so this
    /// tells the user up front whether their observation set can actually
    /// isolate a seed or will just return a huge candidate list.
    pub fn expected_survivors(&self) -> f64 {
        SEED_SPACE as f64 * 0.1f64.powi(self.positives as i32) * 0.9f64.powi(self.negatives as i32)
    }

    pub fn describe(&self) -> String {
        format!(
            "{} confirmed slime chunk(s) + {} confirmed non-slime chunk(s)",
            self.positives, self.negatives
        )
    }
}

/// Brute-forces structure seeds in `[start, end)` that satisfy every constraint.
///
/// `cancel` lets the caller stop early; `scanned` is bumped so a progress bar
/// can be driven from another thread.
pub fn crack_range(
    constraints: &SlimeConstraints,
    start: u64,
    end: u64,
    scanned: &AtomicU64,
    cancel: &AtomicBool,
    limit: usize,
) -> Vec<i64> {
    const CHUNK: u64 = 1 << 20;

    let blocks: Vec<u64> = (start..end).step_by(CHUNK as usize).collect();

    blocks
        .into_par_iter()
        .map(|block_start| {
            if cancel.load(Ordering::Relaxed) {
                return Vec::new();
            }
            let block_end = (block_start + CHUNK).min(end);
            let mut hits = Vec::new();
            for s in block_start..block_end {
                if constraints.accepts(s as i64) {
                    hits.push(s as i64);
                    if hits.len() >= limit {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            scanned.fetch_add(block_end - block_start, Ordering::Relaxed);
            hits
        })
        .reduce(Vec::new, |mut a, b| {
            a.extend(b);
            a
        })
}

/// Filters an existing candidate list instead of scanning the whole space.
pub fn filter_candidates(constraints: &SlimeConstraints, candidates: &[i64]) -> Vec<i64> {
    candidates
        .par_iter()
        .copied()
        .filter(|s| constraints.accepts(*s))
        .collect()
}

/// Measures how fast this machine can test candidates, so the mode can quote a
/// realistic completion time instead of starting a multi-hour run silently.
pub fn benchmark(constraints: &SlimeConstraints) -> f64 {
    let sample: u64 = 4_000_000;
    let threads = rayon::current_num_threads() as f64;
    let start = std::time::Instant::now();
    let counted = (0..sample)
        .into_par_iter()
        .filter(|s| constraints.accepts(*s as i64))
        .count();
    std::hint::black_box(counted);
    let elapsed = start.elapsed().as_secs_f64().max(1e-6);
    // Rate across all cores, which is what the real run will achieve.
    (sample as f64 / elapsed).max(threads)
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 4 — Slime Chunk Seed Cracker");
    ui::note(
        "Enter chunk coordinates (not block coordinates). In game: F3 + G draws chunk borders, \
         and the F3 screen's 'Chunk:' line gives the chunk you are standing in.",
    );
    ui::note(
        "Negative observations — chunks you are confident are NOT slime chunks — are worth \
         entering too, though each one is far weaker than a positive.",
    );

    collect_observations(session)?;

    let constraints = SlimeConstraints::new(&session.slime)?;
    println!();
    ui::note(&format!("Using {}.", constraints.describe()));

    let expected = constraints.expected_survivors();
    if expected > 5000.0 {
        ui::warn(&format!(
            "These observations are expected to leave roughly {expected:.0} candidate seeds \
             across the whole space — not a unique answer."
        ));
        ui::note("15+ confirmed slime chunks is the usual rule of thumb for convergence.");
        if !ui::confirm("Continue anyway?", false)? {
            return Ok(());
        }
    } else {
        ui::note(&format!(
            "Expected survivors across the full space: about {expected:.1}."
        ));
    }

    // Filtering an existing candidate list is enormously cheaper than a fresh
    // scan, so offer it whenever mode 9 or an earlier run left something behind.
    let choices = if session.candidates.is_empty() {
        vec![
            "Scan the full 48-bit structure seed space".to_string(),
            "Scan a range of the seed space".to_string(),
        ]
    } else {
        vec![
            format!(
                "Filter the {} candidate seed(s) already in this session",
                session.candidates.len()
            ),
            "Scan the full 48-bit structure seed space".to_string(),
            "Scan a range of the seed space".to_string(),
        ]
    };
    let offset = if session.candidates.is_empty() { 0 } else { 1 };
    let choice = ui::select("How should the search run?", &choices)?;

    let found = if offset == 1 && choice == 0 {
        let hits = filter_candidates(&constraints, &session.candidates);
        ui::success(&format!(
            "{} of {} candidates satisfy the slime observations.",
            hits.len(),
            session.candidates.len()
        ));
        hits
    } else {
        let full = choice == offset;
        let (start, end) = if full {
            (0u64, SEED_SPACE)
        } else {
            let start: u64 = ui::input_default("Start of range (0 .. 281474976710655)", 0u64)?;
            let end: u64 =
                ui::input_default("End of range (exclusive)", (start + (1 << 32)).min(SEED_SPACE))?;
            if end <= start || end > SEED_SPACE {
                bail!("range must satisfy 0 <= start < end <= {SEED_SPACE}");
            }
            (start, end)
        };

        let rate = benchmark(&constraints);
        let total = end - start;
        let est = total as f64 / rate;
        println!();
        ui::note(&format!(
            "Measured {:.0} million candidate seeds/second on {} threads.",
            rate / 1e6,
            rayon::current_num_threads()
        ));
        ui::warn(&format!(
            "Scanning {} seeds will take about {}.",
            total,
            ui::humanize_duration(est)
        ));
        if est > 300.0 && !ui::confirm("Start the scan?", false)? {
            ui::note(
                "Tip: mode 9's End-pillar shortcut fixes 16 bits of the seed, which cuts this \
                 search to a 2^32 space that finishes in minutes.",
            );
            return Ok(());
        }

        let limit: usize = ui::input_default("Stop after this many hits", 64usize)?;

        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let pb = ui::progress_bar(total, "cracking");

        let hits = std::thread::scope(|scope| {
            let scanned_ref = &scanned;
            let cancel_ref = &cancel;
            let pb_ref = &pb;
            scope.spawn(move || {
                while !cancel_ref.load(Ordering::Relaxed) {
                    let done = scanned_ref.load(Ordering::Relaxed);
                    pb_ref.set_position(done.min(total));
                    if done >= total {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            });
            let hits = crack_range(&constraints, start, end, &scanned, &cancel, limit);
            cancel.store(true, Ordering::Relaxed);
            hits
        });

        pb.finish_and_clear();
        hits
    };

    report(session, found)
}

fn collect_observations(session: &mut Session) -> Result<()> {
    if !session.slime.is_empty()
        && ui::confirm(
            &format!(
                "Reuse the {} slime observation(s) already in this session?",
                session.slime.len()
            ),
            true,
        )?
    {
        return Ok(());
    }

    let is_slime_first = ui::select_str(
        "What are you entering first?",
        &[
            "Confirmed slime chunks",
            "Confirmed NON-slime chunks",
        ],
    )? == 0;

    let mut fresh = Vec::new();
    for (round, is_slime) in [(0, is_slime_first), (1, !is_slime_first)] {
        let label = if is_slime { "slime" } else { "NON-slime" };
        if round == 1 && !ui::confirm(&format!("Also enter confirmed {label} chunks?"), is_slime)? {
            continue;
        }
        let lines = ui::read_block(&format!(
            "Enter confirmed {label} chunk coordinates, one 'chunkX chunkZ' pair per line:"
        ))?;
        for line in lines {
            let Some(parts) = ui::parse_coords(&line) else {
                ui::warn(&format!("skipping unparseable line: {line:?}"));
                continue;
            };
            fresh.push(SlimeObservation {
                chunk_x: parts[0] as i32,
                chunk_z: parts[1] as i32,
                is_slime,
            });
        }
    }

    if fresh.is_empty() {
        bail!("no usable chunk coordinates were entered");
    }
    session.slime = fresh;
    Ok(())
}

fn report(session: &mut Session, found: Vec<i64>) -> Result<()> {
    println!();
    if found.is_empty() {
        ui::warn("No structure seed satisfies all of those observations in the range searched.");
        ui::note(
            "If you searched the full space, at least one observation is probably wrong — \
             a mis-read chunk coordinate is the usual cause.",
        );
        return Ok(());
    }

    ui::success(&format!("{} structure seed(s) found:", found.len()));
    for s in found.iter().take(32) {
        println!("    {s}");
    }
    if found.len() > 32 {
        ui::note(&format!("… and {} more", found.len() - 32));
    }

    println!();
    ui::note(
        "These are structure seeds (the low 48 bits). Every one of them is shared by 65,536 \
         full world seeds, which differ only in their top 16 bits — biome-dependent features \
         are needed to pin those down. Mode 9 does that step.",
    );

    if ui::confirm("Store these as session candidates for mode 9?", true)? {
        session.candidates = found.clone();
    }
    if found.len() == 1 && ui::confirm("Set this as the session seed?", true)? {
        session.seed = Some(found[0]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground truth generated by compiling cubiomes' own C `isSlimeChunk`
    /// (finders.h) with `clang -O2 -fwrapv` and recording its answers.
    ///
    /// Two things are worth knowing about this table.
    ///
    /// First, the C function is `static inline`, so bindgen does not export it
    /// and we cannot call it at runtime. Capturing its output as a fixed table
    /// gives the same independent check without a build-time C shim.
    ///
    /// Second — and this is why `-fwrapv` is not optional — cubiomes computes
    /// `chunkX * chunkX * 0x4c1906` in `int`, which overflows for
    /// |chunkX| >= 21 and is therefore undefined behaviour in C. Built at
    /// `-O2` without `-fwrapv`, clang exploits that UB and produces a value
    /// 2^32 away from the wrapped one, disagreeing with the game for almost
    /// every chunk outside spawn. Java has no such licence: JLS 15.17.1
    /// defines int overflow as two's-complement wrapping, and the widening
    /// cast sign-extends. `-fwrapv` gives the C the same semantics as Java,
    /// and at `-O0` it already agrees. Our Rust uses `wrapping_mul`, so it
    /// matches Java — and hence the game — at every optimisation level.
    const CUBIOMES_VECTORS: &[(i64, i32, i32, bool)] = &[
        (0i64, 0, 0, false),
        (0i64, 1, 1, false),
        (0i64, -1, -1, false),
        (0i64, 7, -13, true),
        (0i64, -13, 7, false),
        (0i64, 100, 100, false),
        (0i64, -100, -100, false),
        (0i64, 1875, -1875, false),
        (0i64, 46000, 46000, false),
        (0i64, -46000, 46000, false),
        (0i64, 1000000, -1000000, false),
        (0i64, 1073741823, -1073741824, false),
        (1i64, 0, 0, false),
        (1i64, 1, 1, false),
        (1i64, -1, -1, false),
        (1i64, 7, -13, true),
        (1i64, -13, 7, true),
        (1i64, 100, 100, true),
        (1i64, -100, -100, false),
        (1i64, 1875, -1875, false),
        (1i64, 46000, 46000, false),
        (1i64, -46000, 46000, false),
        (1i64, 1000000, -1000000, false),
        (1i64, 1073741823, -1073741824, false),
        (-1i64, 0, 0, false),
        (-1i64, 1, 1, false),
        (-1i64, -1, -1, false),
        (-1i64, 7, -13, false),
        (-1i64, -13, 7, false),
        (-1i64, 100, 100, false),
        (-1i64, -100, -100, true),
        (-1i64, 1875, -1875, true),
        (-1i64, 46000, 46000, false),
        (-1i64, -46000, 46000, false),
        (-1i64, 1000000, -1000000, false),
        (-1i64, 1073741823, -1073741824, false),
        (42i64, 0, 0, false),
        (42i64, 1, 1, false),
        (42i64, -1, -1, false),
        (42i64, 7, -13, false),
        (42i64, -13, 7, false),
        (42i64, 100, 100, false),
        (42i64, -100, -100, false),
        (42i64, 1875, -1875, false),
        (42i64, 46000, 46000, true),
        (42i64, -46000, 46000, false),
        (42i64, 1000000, -1000000, true),
        (42i64, 1073741823, -1073741824, false),
        (123456789i64, 0, 0, false),
        (123456789i64, 1, 1, false),
        (123456789i64, -1, -1, false),
        (123456789i64, 7, -13, false),
        (123456789i64, -13, 7, false),
        (123456789i64, 100, 100, false),
        (123456789i64, -100, -100, false),
        (123456789i64, 1875, -1875, false),
        (123456789i64, 46000, 46000, false),
        (123456789i64, -46000, 46000, true),
        (123456789i64, 1000000, -1000000, false),
        (123456789i64, 1073741823, -1073741824, false),
        (-987654321i64, 0, 0, false),
        (-987654321i64, 1, 1, true),
        (-987654321i64, -1, -1, false),
        (-987654321i64, 7, -13, false),
        (-987654321i64, -13, 7, false),
        (-987654321i64, 100, 100, false),
        (-987654321i64, -100, -100, false),
        (-987654321i64, 1875, -1875, false),
        (-987654321i64, 46000, 46000, false),
        (-987654321i64, -46000, 46000, false),
        (-987654321i64, 1000000, -1000000, true),
        (-987654321i64, 1073741823, -1073741824, false),
        (765906787396911863i64, 0, 0, false),
        (765906787396911863i64, 1, 1, false),
        (765906787396911863i64, -1, -1, false),
        (765906787396911863i64, 7, -13, true),
        (765906787396911863i64, -13, 7, false),
        (765906787396911863i64, 100, 100, false),
        (765906787396911863i64, -100, -100, false),
        (765906787396911863i64, 1875, -1875, false),
        (765906787396911863i64, 46000, 46000, false),
        (765906787396911863i64, -46000, 46000, false),
        (765906787396911863i64, 1000000, -1000000, false),
        (765906787396911863i64, 1073741823, -1073741824, false),
        (9223372036854775807i64, 0, 0, false),
        (9223372036854775807i64, 1, 1, false),
        (9223372036854775807i64, -1, -1, false),
        (9223372036854775807i64, 7, -13, false),
        (9223372036854775807i64, -13, 7, false),
        (9223372036854775807i64, 100, 100, false),
        (9223372036854775807i64, -100, -100, true),
        (9223372036854775807i64, 1875, -1875, true),
        (9223372036854775807i64, 46000, 46000, false),
        (9223372036854775807i64, -46000, 46000, false),
        (9223372036854775807i64, 1000000, -1000000, false),
        (9223372036854775807i64, 1073741823, -1073741824, false),
        (-9223372036854775808i64, 0, 0, false),
        (-9223372036854775808i64, 1, 1, false),
        (-9223372036854775808i64, -1, -1, false),
        (-9223372036854775808i64, 7, -13, true),
        (-9223372036854775808i64, -13, 7, false),
        (-9223372036854775808i64, 100, 100, false),
        (-9223372036854775808i64, -100, -100, false),
        (-9223372036854775808i64, 1875, -1875, false),
        (-9223372036854775808i64, 46000, 46000, false),
        (-9223372036854775808i64, -46000, 46000, false),
        (-9223372036854775808i64, 1000000, -1000000, false),
        (-9223372036854775808i64, 1073741823, -1073741824, false),
    ];

    #[test]
    fn matches_cubiomes_reference() {
        // Deliberately includes large and negative chunk coordinates, because
        // that is exactly where the int-vs-long cast differences in the
        // formula start to matter.
        for (seed, cx, cz, expected) in CUBIOMES_VECTORS {
            assert_eq!(
                is_slime_chunk(*seed, *cx, *cz),
                *expected,
                "disagreed with cubiomes at seed {seed}, chunk ({cx}, {cz})"
            );
        }
        assert_eq!(CUBIOMES_VECTORS.len(), 108);
        assert!(
            CUBIOMES_VECTORS.iter().any(|v| v.3),
            "the oracle table should contain at least one slime chunk"
        );
    }

    #[test]
    fn the_squared_term_wraps_like_java_not_like_optimised_c() {
        // Regression guard for the UB described above. At chunkX = 100 the
        // term `100 * 100 * 0x4c1906` overflows int; Java wraps it to
        // -1668187552 and sign-extends. If someone "fixes" slime_offset by
        // widening before the multiply, this catches it.
        let expected_x_sq = (100i32.wrapping_mul(100).wrapping_mul(0x4c1906)) as i64;
        assert_eq!(expected_x_sq, -1668187552);
        assert_eq!(slime_offset(100, 100), 42894254648);
    }

    #[test]
    fn only_the_low_48_bits_of_the_seed_matter() {
        // setSeed masks to 48 bits, so two world seeds sharing a structure seed
        // must produce identical slime chunks. Mode 4's whole framing as a
        // *structure* seed cracker depends on this.
        let base = 123456789i64;
        for k in 1..8i64 {
            let shifted = base.wrapping_add(k << 48);
            for (cx, cz) in [(0, 0), (5, -9), (-77, 31)] {
                assert_eq!(
                    is_slime_chunk(base, cx, cz),
                    is_slime_chunk(shifted, cx, cz),
                    "upper bits changed the result at k={k}"
                );
            }
        }
    }

    #[test]
    fn roughly_one_chunk_in_ten_is_a_slime_chunk() {
        let mut hits = 0;
        let total = 40_000;
        for i in 0..total {
            let cx = (i % 200) - 100;
            let cz = (i / 200) - 100;
            if is_slime_chunk(987654321, cx, cz) {
                hits += 1;
            }
        }
        let rate = hits as f64 / total as f64;
        assert!(
            (0.085..0.115).contains(&rate),
            "slime chunk rate was {rate}, expected about 0.1"
        );
    }

    #[test]
    fn a_known_seed_is_recovered_from_its_own_slime_chunks() {
        // End-to-end proof that the cracker works: take a seed, harvest its
        // real slime chunks, then search a window containing it and check the
        // seed comes back.
        let secret: i64 = 0x0000_1234_5678_9ABC;
        let mut obs = Vec::new();
        let mut cx = 0;
        let mut cz = 0;
        while obs.iter().filter(|o: &&SlimeObservation| o.is_slime).count() < 12 {
            let is_slime = is_slime_chunk(secret, cx, cz);
            if is_slime || obs.len() % 3 == 0 {
                obs.push(SlimeObservation {
                    chunk_x: cx,
                    chunk_z: cz,
                    is_slime,
                });
            }
            cx += 1;
            if cx > 60 {
                cx = 0;
                cz += 1;
            }
            assert!(cz < 200, "ran out of search area harvesting slime chunks");
        }

        let constraints = SlimeConstraints::new(&obs).unwrap();
        assert!(constraints.accepts(secret));

        let scanned = AtomicU64::new(0);
        let cancel = AtomicBool::new(false);
        let start = secret as u64 - 300_000;
        let end = secret as u64 + 300_000;
        let hits = crack_range(&constraints, start, end, &scanned, &cancel, 64);
        assert!(
            hits.contains(&secret),
            "cracker missed the seed it was given observations for; found {hits:?}"
        );
    }

    #[test]
    fn contradictory_observations_are_rejected() {
        let obs = vec![
            SlimeObservation { chunk_x: 3, chunk_z: 4, is_slime: true },
            SlimeObservation { chunk_x: 3, chunk_z: 4, is_slime: false },
        ];
        assert!(SlimeConstraints::new(&obs).is_err());
        assert!(SlimeConstraints::new(&[]).is_err());
    }

    #[test]
    fn positives_are_tested_before_negatives() {
        let obs = vec![
            SlimeObservation { chunk_x: 1, chunk_z: 1, is_slime: false },
            SlimeObservation { chunk_x: 2, chunk_z: 2, is_slime: true },
        ];
        let c = SlimeConstraints::new(&obs).unwrap();
        assert!(c.tests[0].1, "the positive constraint should be tested first");
    }

    #[test]
    fn expected_survivors_shrinks_with_more_positives() {
        let one = SlimeConstraints::new(&[SlimeObservation {
            chunk_x: 0,
            chunk_z: 0,
            is_slime: true,
        }])
        .unwrap();
        let two = SlimeConstraints::new(&[
            SlimeObservation { chunk_x: 0, chunk_z: 0, is_slime: true },
            SlimeObservation { chunk_x: 1, chunk_z: 0, is_slime: true },
        ])
        .unwrap();
        assert!((one.expected_survivors() / two.expected_survivors() - 10.0).abs() < 1e-6);
    }
}
