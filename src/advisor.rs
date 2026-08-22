//! Mode 12 — deciding what to go and look at next, and explaining why a
//! candidate survived.
//!
//! Every other mode answers "here is what I saw, what does it mean?". This one
//! runs the question backwards: given what you already have, which single
//! observation would eliminate the most seeds?
//!
//! # Two regimes
//!
//! **With a candidate list** (from mode 4, 9 or 1b) the answer is exact. Any
//! proposed observation partitions the current candidates into the ones that
//! would say yes and the ones that would say no. If a split is 50/50 the
//! observation is worth a full bit and halves the list; if 99/1 it is nearly
//! worthless. So the advisor evaluates real candidate seeds against real
//! proposed positions and ranks by the actual split — no modelling assumptions
//! at all. This is the classic decision-tree criterion.
//!
//! **Without one** the space is still 2^48 and there is nothing to partition,
//! so the ranking falls back to the a-priori information content of each
//! observation type: how surprising its outcome is, on average, given the rate
//! at which the game produces it.
//!
//! # Why the a-priori numbers look the way they do
//!
//! A slime chunk is a 1-in-10 event. Learning "yes" is worth `log2(10)` = 3.32
//! bits, but that only happens a tenth of the time; "no" is worth `log2(10/9)`
//! = 0.15 bits and happens the rest. The average is 0.469 bits — which is why
//! fifteen-odd slime chunks are needed and why negatives, though genuinely
//! useful, are weak. Bedrock at y=4 is a 1-in-5 event and averages 0.72 bits
//! per block, but blocks can be read by the dozen from one screenshot, so its
//! yield per minute of play is far higher. End pillars are worth a flat 16
//! bits for a single visit, which is why every route that can start there
//! should.

use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::bedrock;
use crate::multicrack::{ConstraintSet, PILLAR_CANDIDATE_COUNT};
use crate::session::Session;
use crate::slime;
use crate::ui;
use crate::worldgen::{STRUCTURES, Version, structure_config};

/// How costly an observation is to actually go and make.
///
/// Information alone would always send you to the End; this is what stops the
/// advice being useless in practice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// Readable from a screenshot you already have.
    Trivial,
    /// A short walk or a look around.
    Low,
    /// Travel, or waiting for spawns.
    High,
}

impl Effort {
    pub fn label(&self) -> &'static str {
        match self {
            Effort::Trivial => "from a screenshot",
            Effort::Low => "a short trip",
            Effort::High => "travel or waiting",
        }
    }
}

/// One suggested next observation.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub what: String,
    /// Expected information gained, in bits.
    pub bits: f64,
    /// Expected candidates left afterwards, when a candidate list is known.
    pub remaining: Option<f64>,
    pub effort: Effort,
    pub note: Option<String>,
}

/// Expected bits gained and expected survivors, for a yes/no observation that
/// splits `candidates` into `yes` and `total - yes`.
///
/// After the observation you keep whichever branch matched, and each branch is
/// taken with probability proportional to its size. So the expected surviving
/// count is `p*yes + (1-p)*no`, which is `(yes^2 + no^2) / total` — minimised
/// at an even split, exactly as intuition says.
pub fn split_value(total: usize, yes: usize) -> (f64, f64) {
    if total == 0 {
        return (0.0, 0.0);
    }
    let n = total as f64;
    let a = yes as f64;
    let b = n - a;
    let remaining = (a * a + b * b) / n;
    // Expected bits = H(p), the entropy of the outcome.
    let bits = [a, b]
        .iter()
        .filter(|c| **c > 0.0)
        .map(|c| {
            let p = c / n;
            -p * p.log2()
        })
        .sum();
    (bits, remaining)
}

/// Scores a yes/no predicate against the candidate list.
fn score<F>(candidates: &[i64], predicate: F) -> (f64, f64)
where
    F: Fn(i64) -> bool + Sync,
{
    let yes = candidates.par_iter().filter(|s| predicate(**s)).count();
    split_value(candidates.len(), yes)
}

/// Finds the chunk near `(cx, cz)` whose slime status splits the candidates
/// most evenly.
pub fn best_slime_chunk(
    candidates: &[i64],
    centre_chunk_x: i32,
    centre_chunk_z: i32,
    radius: i32,
) -> Option<(i32, i32, f64, f64)> {
    let mut best: Option<(i32, i32, f64, f64)> = None;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (cx, cz) = (centre_chunk_x + dx, centre_chunk_z + dz);
            let offset = slime::slime_offset(cx, cz);
            let (bits, rem) = score(candidates, |s| {
                slime::is_slime_chunk_with_offset(s, offset)
            });
            if best.is_none_or(|(_, _, b, _)| bits > b) {
                best = Some((cx, cz, bits, rem));
            }
        }
    }
    best
}

/// Finds the nether bedrock block near `(x, z)` that splits the candidates
/// most evenly, on the informative layer of the chosen surface.
pub fn best_bedrock_block(
    candidates: &[i64],
    centre_x: i32,
    centre_z: i32,
    radius: i32,
    y: i32,
) -> Option<(i32, i32, f64, f64)> {
    let mut best: Option<(i32, i32, f64, f64)> = None;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (x, z) = (centre_x + dx, centre_z + dz);
            let (bits, rem) = score(candidates, |s| {
                bedrock::is_bedrock(&bedrock::layer_seeds(s), x, y, z)
            });
            if best.is_none_or(|(_, _, b, _)| bits > b) {
                best = Some((x, z, bits, rem));
            }
        }
    }
    best
}

/// "a" or "an", so the generated suggestions read like English.
fn article(name: &str) -> &'static str {
    match name.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a' | 'e' | 'i' | 'o' | 'u') => "an",
        _ => "a",
    }
}

/// Average bits from a yes/no observation that comes up "yes" with probability
/// `p`, before any candidates are known.
fn a_priori_bits(p: f64) -> f64 {
    if p <= 0.0 || p >= 1.0 {
        return 0.0;
    }
    -(p * p.log2() + (1.0 - p) * (1.0 - p).log2())
}

/// The ranking used when there is no candidate list to partition.
pub fn a_priori_proposals(version: Version) -> Vec<Proposal> {
    let mut out = vec![
        Proposal {
            what: "End pillar heights (all ten, from the central End island)".to_string(),
            bits: 16.0,
            remaining: None,
            effort: Effort::High,
            note: Some(format!(
                "Fixes the pillar seed outright, leaving {PILLAR_CANDIDATE_COUNT} candidates \
                 — a sweep of minutes rather than days. One visit, and nothing else comes close."
            )),
        },
        Proposal {
            what: "A nether bedrock block at y=4 or y=123".to_string(),
            bits: a_priori_bits(0.2),
            remaining: None,
            effort: Effort::Trivial,
            note: Some(
                "Individually small, but a single screenshot of the floor or roof yields \
                 dozens, and floor and roof use independent layer seeds."
                    .to_string(),
            ),
        },
        Proposal {
            what: "A confirmed slime chunk".to_string(),
            bits: a_priori_bits(0.1),
            remaining: None,
            effort: Effort::High,
            note: Some(
                "A 1-in-10 event, so a 'yes' is worth 3.3 bits but arrives rarely. Confirming \
                 one means waiting for spawns below y=40."
                    .to_string(),
            ),
        },
        Proposal {
            what: "A confirmed NON-slime chunk".to_string(),
            bits: a_priori_bits(0.9),
            remaining: None,
            effort: Effort::High,
            note: Some(
                "Worth recording if you have it, but nine times weaker than a positive and \
                 hard to be sure of — absence of slimes is not proof."
                    .to_string(),
            ),
        },
    ];

    // Structures are worth two nextInt draws each, so their value depends on
    // the per-version chunk range rather than being a fixed number.
    for (stype, name, _) in STRUCTURES.iter().take(12) {
        if let Some(cfg) = structure_config(version, *stype)
            && cfg.chunk_range > 1
        {
            let bits = 2.0 * (cfg.chunk_range as f64).log2();
            let liftable = !cfg.is_power_of_two_range() && cfg.lift_valuation() > 0;
            let spacing_blocks = cfg.region_blocks();

            // Information alone ranks woodland mansions second, which is
            // terrible advice: they sit on an 80-chunk grid, so "go find one"
            // can mean thousands of blocks of travel. Effort tracks the region
            // size so the ranking stays honest about what it is asking for.
            let effort = if spacing_blocks >= 1024 {
                Effort::High
            } else {
                Effort::Low
            };

            let mut note = if liftable {
                format!(
                    "Drives the bit-lifting sieve ({} low bit{}), which is what lets mode 9 \
                     crack with no End trip.",
                    cfg.lift_valuation(),
                    if cfg.lift_valuation() == 1 { "" } else { "s" }
                )
            } else {
                "Usable as a check, but leaks no low bits for the lifting sieve.".to_string()
            };
            note.push_str(&format!(" One per {spacing_blocks} blocks."));

            out.push(Proposal {
                what: format!("The exact origin chunk of {} {}", article(name), name.to_lowercase()),
                bits,
                remaining: None,
                effort,
                note: Some(note),
            });
        }
    }

    // Sort by information, then surface effort so a high-yield/high-cost item
    // cannot masquerade as an easy win.
    out.sort_by(|a, b| b.bits.partial_cmp(&a.bits).unwrap_or(std::cmp::Ordering::Equal));
    out
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 12 — Observation Advisor");

    let choice = ui::select_str(
        "What would you like?",
        &[
            "What should I observe next?",
            "Explain why a candidate seed survives (or does not)",
        ],
    )?;

    if choice == 0 {
        advise(session)
    } else {
        explain(session)
    }
}

fn advise(session: &mut Session) -> Result<()> {
    let version = ui::prompt_version(session)?;

    if session.candidates.is_empty() {
        ui::note("No candidate seeds yet, so the whole 2^48 space is still in play.");
        ui::note("Ranking by how much each kind of observation is worth on average:");
        println!();
        for (i, p) in a_priori_proposals(version).iter().take(8).enumerate() {
            println!("  {:>2}. {:<52} {:>6.2} bits  [{}]", i + 1, p.what, p.bits, p.effort.label());
            if let Some(n) = &p.note {
                ui::note(&format!("      {n}"));
            }
        }
        println!();
        ui::note(
            "48 bits pins a structure seed. Run mode 9 once you have enough, then come back \
             here — with a real candidate list the advice becomes exact rather than average.",
        );
        return Ok(());
    }

    let n = session.candidates.len();
    ui::success(&format!("{n} candidate seed(s) in play ({:.1} bits).", (n as f64).log2()));

    const CAP: usize = 500_000;
    let candidates: Vec<i64> = if n > CAP {
        ui::warn(&format!(
            "Sampling {CAP} of them; the ranking is statistical rather than exact above that."
        ));
        session.candidates.iter().copied().take(CAP).collect()
    } else {
        session.candidates.clone()
    };

    ui::note("Where are you? Suggestions are searched around this point.");
    let px: i32 = ui::input_default("Your X", 0)?;
    let pz: i32 = ui::input_default("Your Z", 0)?;
    let radius: i32 = ui::input_default("Search radius to consider (chunks/blocks)", 8)?;
    if !(0..=64).contains(&radius) {
        bail!("radius must be between 0 and 64");
    }

    let pb = ui::spinner("evaluating candidate observations");
    let mut proposals: Vec<Proposal> = Vec::new();

    if let Some((cx, cz, bits, rem)) =
        best_slime_chunk(&candidates, px.div_euclid(16), pz.div_euclid(16), radius)
    {
        proposals.push(Proposal {
            what: format!("Check whether chunk ({cx}, {cz}) is a slime chunk"),
            bits,
            remaining: Some(rem),
            effort: Effort::High,
            note: Some(format!(
                "Blocks x {}..{}, z {}..{}",
                cx * 16,
                cx * 16 + 15,
                cz * 16,
                cz * 16 + 15
            )),
        });
    }

    for (y, surface) in [(4, "floor"), (123, "roof")] {
        if let Some((x, z, bits, rem)) =
            best_bedrock_block(&candidates, px.div_euclid(8), pz.div_euclid(8), radius, y)
        {
            proposals.push(Proposal {
                what: format!("Look at nether {surface} block ({x}, {y}, {z})"),
                bits,
                remaining: Some(rem),
                effort: Effort::Trivial,
                note: Some("Nether coordinates. Read the whole visible patch while you are there.".to_string()),
            });
        }
    }
    pb.finish_and_clear();

    proposals.sort_by(|a, b| b.bits.partial_cmp(&a.bits).unwrap_or(std::cmp::Ordering::Equal));

    println!();
    ui::success("Best next observations, by how evenly they split your candidates:");
    for (i, p) in proposals.iter().enumerate() {
        println!("  {:>2}. {}", i + 1, p.what);
        match p.remaining {
            Some(rem) => println!(
                "      {:.3} bits  ->  about {:.0} candidates left ({:.1}% eliminated)  [{}]",
                p.bits,
                rem,
                100.0 * (1.0 - rem / candidates.len() as f64),
                p.effort.label()
            ),
            None => println!("      {:.3} bits  [{}]", p.bits, p.effort.label()),
        }
        if let Some(note) = &p.note {
            ui::note(&format!("      {note}"));
        }
    }

    println!();
    if proposals.first().is_some_and(|p| p.bits < 0.01) {
        ui::warn(
            "Nothing nearby distinguishes your candidates — every one of them agrees on this \
             whole area. Move somewhere further away and try again.",
        );
    } else {
        ui::note(
            "A perfectly even split is 1.000 bits and halves the list. Anything close to 0 \
             means your candidates already agree there, so looking would tell you nothing.",
        );
    }
    Ok(())
}

fn explain(session: &mut Session) -> Result<()> {
    let version = ui::prompt_version(session)?;
    let tolerance: i32 = ui::input_default("Structure position tolerance (blocks)", 16)?;
    let constraints = ConstraintSet::build(session, version, tolerance)?;

    if constraints.is_empty() {
        bail!("no constraints recorded yet — collect some observations in modes 1b, 4, 6 or 9 first");
    }

    let seed = if !session.candidates.is_empty()
        && ui::confirm(
            &format!("Explain the first of the {} session candidates?", session.candidates.len()),
            true,
        )? {
        session.candidates[0]
    } else {
        ui::prompt_seed_value("Seed to explain")?
    };

    let results = constraints.explain(seed);
    let passed = results.iter().filter(|(_, ok)| *ok).count();

    println!();
    ui::success(&format!("Seed {seed}: {passed} of {} constraints matched.", results.len()));
    println!();
    for (label, ok) in &results {
        if *ok {
            println!("  \x1b[32m✓\x1b[0m {label}");
        } else {
            println!("  \x1b[31m✗\x1b[0m {label}");
        }
    }

    println!();
    if passed == results.len() {
        ui::success("This seed is consistent with everything you have recorded.");
    } else if results.len() - passed <= 2 {
        ui::warn(&format!(
            "A near miss — only {} constraint(s) failed. That pattern usually means a \
             mis-typed coordinate in those specific observations rather than a wrong seed.",
            results.len() - passed
        ));
    } else {
        ui::note("Not a near miss; this seed is simply not the one.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SlimeObservation;

    #[test]
    fn an_even_split_is_worth_exactly_one_bit() {
        let (bits, rem) = split_value(100, 50);
        assert!((bits - 1.0).abs() < 1e-12);
        assert!((rem - 50.0).abs() < 1e-9, "an even split should halve the list");
    }

    #[test]
    fn a_useless_observation_is_worth_nothing() {
        // Every candidate agrees, so looking tells you nothing and eliminates
        // nobody.
        let (bits, rem) = split_value(100, 100);
        assert_eq!(bits, 0.0);
        assert!((rem - 100.0).abs() < 1e-9);

        let (bits, rem) = split_value(100, 0);
        assert_eq!(bits, 0.0);
        assert!((rem - 100.0).abs() < 1e-9);

        assert_eq!(split_value(0, 0), (0.0, 0.0));
    }

    #[test]
    fn lopsided_splits_are_worth_less_than_even_ones() {
        let (even, even_rem) = split_value(1000, 500);
        let (skew, skew_rem) = split_value(1000, 50);
        assert!(skew < even, "a 5/95 split should beat nothing but lose to 50/50");
        assert!(skew > 0.0);
        assert!(skew_rem > even_rem, "a lopsided split leaves more behind");
    }

    #[test]
    fn the_advisor_picks_a_genuinely_discriminating_chunk() {
        // Build a candidate list, ask for the best chunk to check, then verify
        // the recommendation really does split it as promised.
        let candidates: Vec<i64> = (0..4000i64).map(|i| i.wrapping_mul(2_654_435_761)).collect();
        let (cx, cz, bits, rem) = best_slime_chunk(&candidates, 0, 0, 4).unwrap();

        let yes = candidates
            .iter()
            .filter(|s| slime::is_slime_chunk(**s, cx, cz))
            .count();
        let (check_bits, check_rem) = split_value(candidates.len(), yes);
        assert!((bits - check_bits).abs() < 1e-9, "reported bits should match reality");
        assert!((rem - check_rem).abs() < 1e-6);

        // Slime chunks are 1-in-10, so the best of 81 nearby chunks should
        // still be a real split rather than a degenerate one.
        assert!(bits > 0.3, "expected a usable split, got {bits} bits");
        assert!(rem < candidates.len() as f64);
    }

    #[test]
    fn bedrock_advice_also_matches_reality() {
        let candidates: Vec<i64> = (0..1500i64).map(|i| i.wrapping_mul(6_364_136_223)).collect();
        let (x, z, bits, _) = best_bedrock_block(&candidates, 0, 0, 3, 4).unwrap();
        let yes = candidates
            .iter()
            .filter(|s| bedrock::is_bedrock(&bedrock::layer_seeds(**s), x, 4, z))
            .count();
        let (check, _) = split_value(candidates.len(), yes);
        assert!((bits - check).abs() < 1e-9);
    }

    #[test]
    fn a_priori_ranking_puts_pillars_first_and_negatives_last() {
        let props = a_priori_proposals(Version::V1_21_1);
        assert!(props[0].what.contains("End pillar"), "pillars should rank first");
        assert!((props[0].bits - 16.0).abs() < 1e-9);

        let last = props.last().unwrap();
        assert!(
            last.what.contains("NON-slime"),
            "a negative slime observation should rank last, got {:?}",
            last.what
        );
        // A 1-in-10 event carries more than a 9-in-10 one.
        assert!(a_priori_bits(0.1) > a_priori_bits(0.9) || (a_priori_bits(0.1) - a_priori_bits(0.9)).abs() < 1e-12);
        assert!((a_priori_bits(0.2) - 0.7219).abs() < 1e-3, "bedrock at y=4 should be ~0.72 bits");
        assert!((a_priori_bits(0.1) - 0.4690).abs() < 1e-3, "a slime chunk should be ~0.47 bits");
    }

    #[test]
    fn far_apart_structures_are_not_advertised_as_a_short_trip() {
        let props = a_priori_proposals(Version::V1_21_1);
        let mansion = props
            .iter()
            .find(|p| p.what.contains("woodland mansion"))
            .expect("mansions should be proposed");
        // Mansions carry a lot of information but sit on an 80-chunk grid, so
        // the advice must not imply they are nearby.
        assert!(mansion.bits > 9.0, "mansions should still rank as informative");
        assert_eq!(
            mansion.effort,
            Effort::High,
            "an 80-chunk grid is not a short trip"
        );

        let village = props
            .iter()
            .find(|p| p.what.contains("village"))
            .expect("villages should be proposed");
        assert_eq!(village.effort, Effort::Low);
    }

    #[test]
    fn suggestions_read_as_english() {
        assert_eq!(article("Ocean monument"), "an");
        assert_eq!(article("Igloo"), "an");
        assert_eq!(article("Village"), "a");
        assert_eq!(article("Desert pyramid"), "a");

        let props = a_priori_proposals(Version::V1_21_1);
        for p in &props {
            assert!(!p.what.contains("a ocean"), "article agreement: {}", p.what);
            assert!(!p.what.contains("a igloo"), "article agreement: {}", p.what);
            if let Some(n) = &p.note {
                assert!(!n.contains("1 low bits"), "pluralisation: {n}");
            }
        }
    }

    #[test]
    fn explain_names_every_constraint_and_flags_the_failures() {
        let version = Version::V1_21_1;
        let secret: i64 = 1234;
        let mut session = Session {
            version: Some(version),
            ..Default::default()
        };
        // Five true observations plus one deliberately wrong.
        session.slime = (0..5)
            .map(|i| SlimeObservation {
                chunk_x: i,
                chunk_z: i * 2,
                is_slime: slime::is_slime_chunk(secret, i, i * 2),
            })
            .collect();
        session.slime.push(SlimeObservation {
            chunk_x: 99,
            chunk_z: 99,
            is_slime: !slime::is_slime_chunk(secret, 99, 99),
        });

        let cs = ConstraintSet::build(&session, version, 16).unwrap();
        let results = cs.explain(secret);
        assert_eq!(results.len(), 6, "every constraint should be reported, not just the first failure");

        let failed: Vec<&String> = results.iter().filter(|(_, ok)| !*ok).map(|(l, _)| l).collect();
        assert_eq!(failed.len(), 1, "exactly the planted error should fail");
        assert!(failed[0].contains("99, 99"), "the failure should name itself: {failed:?}");

        // And accepts() agrees with the summary.
        assert!(!cs.accepts(secret));
    }
}
