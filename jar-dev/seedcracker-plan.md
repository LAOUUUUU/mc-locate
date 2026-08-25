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

**Tier 2 — Read the hashed seed off the connection.**
- Small but high-value: on a server the client receives the hashed seed in the
  login packet. Read it in the mod and export it, so the Tier-1 engine can pin
  the exact world seed automatically. Pairs with manually-typed structure coords
  today and with Tier 3 tomorrow. (Verify the read against a known singleplayer
  world: `hashseed::hashed_seed(realSeed)` must equal the value the client got.)

**Tier 3 — Reading structures in-game (the headline feature).**

This is what turns the tool from "type coordinates" into SCX's "run around and it
cracks itself," and it is the hardest engineering here — the maths are done, this
is detection. The Tier-1 lift needs a structure's **exact origin chunk** (the
chunk-aligned corner cubiomes reports); one wrong chunk silently kills the seed,
so detection has to be exact, not approximate.

- **Per-structure detectors.** The client only has the blocks in its loaded
  chunks, so each structure needs a routine that recognises it from its blocks
  and derives its origin chunk. Start with the small fixed-layout *features*,
  which are the most reliable and are exactly the liftable ones Tier 1 already
  cracks:
  - Desert pyramid — orange/blue terracotta pattern over the hidden TNT room; the
    center maps to a fixed offset from the origin chunk.
  - Igloo — snow-block dome + (sometimes) the basement ladder.
  - Jungle temple — mossy/cobblestone box with levers and the tripwire corridor.
  - Swamp hut — the raised cauldron + crafting-table + mushroom footprint.
  - Shipwreck, Pillager outpost — larger but distinctive.
- **Origin, not "somewhere near."** For each detector, find one unambiguous
  anchor block, then compute the structure's start chunk from the known layout
  offset for that version. Refuse (don't guess) when the loaded chunks don't
  contain enough of the structure to fix the anchor.
- **Free correctness oracle.** In a singleplayer world the mod knows the real
  seed, and cubiomes knows where every structure generates for that seed. So a
  detector can be validated automatically: detect the structure, compare the
  origin it computed to cubiomes' `getStructurePos` — they must match to the
  chunk. This is a real regression test for every detector, on real worlds.
- **Export + crack.** Feed `{type, originX, originZ}` through the existing export
  → watch → lift loop, alongside the Tier-2 hashed seed, and the CLI returns the
  world seed while you explore — on 26.x, which SCX cannot do.
- **Borrow (MIT, verify):** SCX's per-structure finders are the reference for the
  anchor blocks and offsets; re-derive each against the actual structure NBT /
  cubiomes rather than copying, and version-gate with Stonecutter since layouts
  shift between versions.

**Tier 4 — Nether & more decorators.**
- Port the layered-filter full bedrock crack (not just ranged brute force).
- Add the decorators SCX left broken, each re-derived and verified for current
  versions (this is where "more up to date than SCX" is won).

**Tier 5 — Coverage parity across versions.**
- Ensure every source works 1.21 → 26.2 (cubiomes fork already reaches 26.2).
- Verified biome filters where they genuinely help (SCX's biome path is one of
  its "gives wrong data" features — only ship ours once tested).

**Tier 6 — Polish.**
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

## Next concrete step

Tier 1 is done. The next step is **Tier 2 — read the hashed seed off the
connection** in the exporter mod: it is small, self-contained, and it makes the
whole Tier-1 engine usable on a server without typing anything, since the hashed
seed pins the exact world seed from whatever structure/pillar/bedrock candidates
you have. Tier 3 (reading structures) then removes the last manual step.
