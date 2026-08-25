# Plan: a seed cracker that beats SeedCrackerX

The goal is not "another in-game cracker." It is to take what mc-locate already
does better — exact math, verified constants, current versions, more sources —
and close the few gaps where SeedCrackerX (SCX) is still ahead, so the combined
tool is strictly more capable.

## Honest starting point

**What mc-locate already has that SCX does not (or does worse):**

- **End-pillar shortcut** — enumerate exactly 2³² structure seeds from one visit.
- **Nether bedrock cracking** (1.18+, floor + roof). SCX has *none* — its README
  hands you off to a separate `Nether_Bedrock_Cracker`.
- **A tested lattice reverser** (LLL + Fincke–Pohst) for decorator RNG, with a
  guard that refuses hopeless queries instead of grinding for 1000 s.
- **Decorator / population-seed crack** (mode 15), maths transcribed from the
  canonical `mc_core_java`, round-trip tested through the real RNG.
- **Multi-source combiner** — mix pillars, bedrock, slime, structures, eyes.
- **Current versions** — the exporter mod spans 1.21 → 26.2; SCX lags and, by its
  own README, has features that "aren't updated and can give wrong data."
- **A hybrid architecture** — a native Rust engine for the heavy exact math, fed
  live by the Fabric mod through the auto-import loop.

**What SCX still does that we do not** (updated after auditing the code):

- ~~Liftable structure cracking~~ — **already present.** `lifting.rs` cracks a
  structure seed from structure positions by bit-lifting, with salts pulled from
  cubiomes' `getStructureConfig` (never hand-rolled), covering all six SCX
  structures *and more* (Village, Ancient City, Trial Chambers, 1.18+
  Fortress/Bastion). Tested end to end.
- ~~Hashed-seed disambiguation~~ — **done** (`hashseed.rs`), verified against the
  real Guava the game ships.
- **In-game structure detection** — it draws an outline when it finds a structure
  and cracks automatically. We capture bedrock/pillars/eyes but not structures.
  (Tier 2.)
- **A mature in-game GUI** and progress/bit accounting.

## The thesis

Beat SCX on **correctness, coverage, and current versions**, not on being a
flashier in-game mod. Concretely: make the Rust CLI a complete structure-seed
engine (the SCX core, reimplemented and verified), keep the mod as the live
capture front-end, and add the two things that turn "narrow the candidates" into
"here is your seed": the structure salt tables and hashed-seed disambiguation.

## Licensing note (important)

SCX is **MIT**, so its code *may* be reused with attribution — but the plan is to
**reimplement the techniques and verify every constant against primary sources**,
not copy files. Salts and formulas are facts (not copyrightable); code is. Where
we lean on SCX's specific tables, credit it in a NOTICE file. This keeps
mc-locate's MIT licence clean and honours the "never ship a guessed constant"
rule that has already caught real bugs.

## What to borrow, and from where

| Technique | Source to study | Note |
|---|---|---|
| Structure salts / bit counts | SCX (MIT) + cubiomes | Verify each salt against cubiomes' `getStructurePos`; do not trust one source |
| Population / decorator seed maths | SeedFinding `mc_core_java` | Already done in `decorator.rs` |
| Hashed-seed → world seed | SCX + Minecraft source | `hashedSeed = sha256(worldSeed) as long`; verify against a known world |
| Full 2⁴⁸ Nether bedrock crack | `19MisterX98/Nether_Bedrock_Cracker` | We only have ranged brute force; port the layered filter tree |
| Hard lattice cases (dim ~6) | LattiCG | Exact-rational LP where our Fincke–Pohst is weak |
| Structure outline detection in-game | SCX | For the mod's live capture of structures |

## Roadmap (tiers, biggest ROI first)

**Tier 1 — Structure-seed engine in the CLI (the SCX core). ✅ DONE.**
- ✅ Salts for every liftable structure come from cubiomes' `getStructureConfig`,
  not a hand-maintained table — the audit found this already built in `lifting.rs`
  and tested (`a_known_seed_is_recovered_from_structures_alone`).
- ✅ Structure positions feed the lift + multi-source sieve (mode 9).
- ✅ **Hashed-seed disambiguation** (`hashseed.rs`, mode 9): structure-seed
  candidates + the hashed seed → the exact world seed. Decoys contribute nothing,
  so the structures need not pin the structure seed uniquely on their own. This
  was the one genuinely missing piece, and it is the highest-leverage one — it
  turns every source's 65 536-way lift into a single answer, no biome data, and
  works on versions past cubiomes.
- Outcome (now real): structure coordinates + the hashed seed → one world seed,
  entirely in the CLI, verified three ways.

**Tier 2 — Live structure capture in the mod.**
- Detect a structure's origin from loaded chunks (start piece / bounding box),
  read its liftable position, export it. Reuses the export/watch loop already
  built, so the CLI cracks as you explore — SCX's headline experience, but with
  our engine behind it and on 26.x.
- Read the **hashed seed** off the client connection and export it too.

**Tier 3 — Nether & more decorators.**
- Port the layered-filter full bedrock crack (not just ranged brute force).
- Add the decorators SCX left broken, each re-derived and verified for current
  versions (this is where "more up to date than SCX" is won).

**Tier 4 — Coverage parity across versions.**
- Ensure every source works 1.21 → 26.2 (cubiomes fork already reaches 26.2).
- Verified biome filters where they genuinely help (SCX's biome path is one of
  its "gives wrong data" features — only ship ours once tested).

**Tier 5 — Polish.**
- In-game bit accounting / HUD (how close am I), a data view, and docs.

## Honest risks and limits

- **Structure outline detection is real work** — SCX has years of it; Tier 2 is
  the hardest engineering, not the maths.
- **Salts are version- and biome-specific** — the verification burden is the
  point, and the reason not to rush a table in.
- **Server-side leaks can be patched** — Paper's `feature-seeds` and friends
  randomise exactly these bits. On a hardened server, nothing here works, and
  that is by design, not a bug to route around. Same ceiling SCX hits.
- **We can't out-crack physics** — if the server never sends the leak, there is
  nothing to reverse.

## First concrete step

Tier 1's hashed-seed disambiguation, because it multiplies the value of every
source we already have: today a structure/pillar/bedrock crack leaves 65 536
world seeds; the hashed seed picks the one. It is small, self-contained, and
verifiable against any known world. Everything else in Tier 1 builds on the
lift + sieve that already exist.
