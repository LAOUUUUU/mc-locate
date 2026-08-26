<div align="center">

# mc-locate

**Reverse-engineer Minecraft Java Edition seeds and coordinates
from limited in-game observations**

[![CI](https://github.com/LAOUUUUU/mc-locate/actions/workflows/ci.yml/badge.svg)](https://github.com/LAOUUUUU/mc-locate/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/LAOUUUUU/mc-locate?color=brightgreen&label=release)](https://github.com/LAOUUUUU/mc-locate/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/LAOUUUUU/mc-locate/total?color=blue&label=downloads)](https://github.com/LAOUUUUU/mc-locate/releases)
[![Stars](https://img.shields.io/github/stars/LAOUUUUU/mc-locate?style=flat&color=yellow)](https://github.com/LAOUUUUU/mc-locate/stargazers)
[![Visitors](https://visitor-badge.laobi.icu/badge?page_id=LAOUUUUU.mc-locate&left_text=visitors)](https://github.com/LAOUUUUU/mc-locate)

![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-lightgrey)
![Rust](https://img.shields.io/badge/rust-2024%20edition-orange?logo=rust&logoColor=white)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![Minecraft](https://img.shields.io/badge/Minecraft-Beta%201.7%20%E2%86%92%2026.2-62B47A)

[**Download**](https://github.com/LAOUUUUU/mc-locate/releases/latest) &nbsp;·&nbsp;
[Install](#install) &nbsp;·&nbsp;
[The modes](#the-modes) &nbsp;·&nbsp;
[What is verified](#what-is-verified-and-against-what)

</div>

---

Point it at whatever you can see — a bedrock pattern, a slime chunk, a village,
an eye-of-ender throw, a screenshot with the F3 overlay open — and it works
backwards to the seed or the coordinate that produced it.

It all rests on one fact: Java worldgen is driven by `java.util.Random`, a
48-bit linear congruential generator. It is not cryptographic, its state is
small, and because the multiplier is odd it is **invertible** — you can step it
backwards as cheaply as forwards. A handful of independent observations is often
enough to collapse the space to a single seed.

### At a glance

|  |  |
|---|---|
| **15 modes** | seed cracking (incl. decorator/population-seed), coordinate recovery, Bayesian triangulation, screenshot OCR, live log watching |
| **293 tests** | every RNG formula checked against an independent source, not from memory |
| **34 versions** | Beta 1.7 through 26.2 — current Minecraft |
| **3 platforms** | one universal macOS binary, Linux x86_64, Windows x86_64 |
| **No setup** | no Rust, no Java, no Minecraft install needed to run it |

## Install

Prebuilt binaries for each release are on the
[Releases page](../../releases). No Rust toolchain needed — download, unpack,
run. It is a **terminal program**: there is no window to double-click on macOS
or Linux; you run it from a shell and drive it with the arrow keys.

### macOS

`mc-locate-macos-universal.tar.gz` is a universal binary — the same download
works on both Apple Silicon and Intel Macs. In Terminal:

```bash
tar -xzf ~/Downloads/mc-locate-macos-universal.tar.gz
xattr -d com.apple.quarantine mc-locate
./mc-locate
```

The `xattr` line matters: the binary is not code-signed, so macOS quarantines
anything downloaded from the internet and will otherwise refuse to run it with
a message about an unidentified developer.

### Linux

```bash
tar -xzf mc-locate-linux-x86_64.tar.gz
chmod +x mc-locate
./mc-locate
```

Built against Ubuntu 22.04's glibc, so it also runs on older distributions.

### Windows

Extract `mc-locate-windows-x86_64.zip` and double-click `mc-locate.exe`, which
opens a console window — or run it from PowerShell:

```powershell
.\mc-locate.exe
```

SmartScreen will warn that the publisher is unknown, because the binary is
unsigned. "More info" then "Run anyway" gets past it.

### Driving it

Arrow keys move the `❯` cursor, Enter selects, Ctrl-C quits. Mode 11 needs no
setup and is the quickest way to confirm it works.

## Build from source

```bash
cargo build --release
```

The binary lands at `target/release/mc-locate`. It is interactive and needs a
real terminal on stdin.

Mode 3 (OCR) needs system Tesseract and Leptonica, so it is behind an optional
feature and **off by default** — everything else builds and runs without it:

```bash
brew install tesseract leptonica pkg-config
cargo build --release --features ocr
```

Without the feature, mode 3 explains how to enable it and offers manual
coordinate entry instead. With it, `backend_tests` renders a synthetic F3
overlay and pushes it through the real Tesseract pipeline, so the OCR path is
covered end to end rather than only at its pure-function edges.

```bash
cargo test
```

## The modes

Modes feed each other. A typical chain is *narrow the area* (6, 8, 10 or 11) →
*pin the position* (1a or 2), or *collect constraints* (1b, 4, 6) → *crack the
seed* (9). Anything a mode learns — seed, version, heading, search box,
candidate seeds — is kept in the session and offered as a default later, so you
type it once.

| # | Mode | What it needs | Where that comes from in game |
|---|------|---------------|-------------------------------|
| 1a | Nether Bedrock: coordinates from pattern | Seed, version, a `#`/`.`/`?` grid at one y-level | Stand on the nether floor/roof and transcribe bedrock vs not. **y=4 and y=123 are the layers worth recording** — bedrock is rarest there, so each block tells you the most |
| 1b | Nether Bedrock: seed crack | Bedrock observations as `X Y Z B` | Same, but collect from **both** floor and roof — they use independent layer seeds, so mixing them cuts false positives hard. 1.18+ only |
| 2 | Overworld Terrain Shape Matcher | Seed, version, a biome or height grid, a bounded box | Read biomes off F3 while walking a grid, or transcribe a height profile from a screenshot |
| 3 | F3 Screenshot OCR Reader | An image or a folder of images | Screenshots with the F3 debug overlay visible. Batch mode writes CSV |
| 4 | Slime Chunk Seed Cracker | Confirmed slime / non-slime chunk coordinates | F3 + G draws chunk borders; slimes spawn below y=40 in slime chunks. The F3 `Chunk:` line gives the chunk you are in |
| 5 | Camera Pose Estimator | A screenshot, plus 4+ tagged pixel ↔ block-corner pairs | Pick recognisable block corners in the shot and read their world coordinates off F3 |
| 6 | Structure-Relative Search Narrower | Seed, version, structure type, centre + radius | You know the seed and can see a village/temple/etc. Produces a box for mode 2 |
| 7 | Chat/Log Coordinate Scraper | A log file or pasted text — or live tailing | `logs/latest.log`. Live mode auto-detects vanilla, Prism/MultiMC, CurseForge and ATLauncher paths |
| 8 | Compass + Biome Triangulation | An orientation cue, seed, anchor, biome sequence | Sun/cloud direction or a known block-texture orientation, plus the biomes you crossed |
| 9 | Multi-Source Seed Cracker | Any mix of End pillars, structures, slime chunks, bedrock | The master mode — see below |
| 10 | Stronghold Ring Triangulator | Player X/Z and yaw per eye-of-ender throw | Throw an eye, then F3 + C and read position and `Facing` |
| 11 | Nether ↔ Overworld Portal Converter | One coordinate and its dimension | Anywhere. Pure arithmetic, no search |
| 12 | Observation Advisor | Whatever you already have | Nothing new — it tells you what to go and look at next, and explains why a candidate survives |
| 13 | Session & Observations | A saved file, a screenshots folder, or the exporter mod's folder | Save/load your work; live-auto-import the mod's exports; watch your screenshots folder and read new ones as you press F2 |
| 14 | Documentation | Nothing | The full write-up for every mode, offline, inside the binary |
| 15 | Decorator / Population-Seed Crack | A decorated feature (ore, plant, dungeon) at a known chunk, plus a candidate list | Recover the chunk's population seed and filter the pillars' 2³² candidates to one. See below |

### Mode 9: three routes to a seed

Mode 9 intersects independent constraints instead of brute-forcing any single
one, which is how SeedCrackerX works. There are three ways in, in rough order
of how much walking they cost you.

The highest-value observation in the game is the **End pillar arrangement**:

```java
long pillarSeed = new Random(worldSeed).nextLong() & 65535L;
Collections.shuffle(order, new Random(pillarSeed));
```

Only 65,536 arrangements exist, so the ten pillar heights identify the pillar
seed outright. That is worth far more than a filter: `pillarSeed` is the low 16
bits of `nextLong()`, which is bits 16..31 of the LCG state two steps in. Since
the LCG is invertible, mc-locate *enumerates* exactly the 2³² structure seeds
consistent with it and steps backwards to each one — instead of testing 2⁴⁸.
That turns a multi-day sweep into minutes.

**Route 2 — structures alone, by bit-lifting.** No End trip needed. A
structure's in-region offset comes from `nextInt(chunkRange)`, and when `2^j`
divides `chunkRange` the identity `v % chunkRange ≡ v (mod 2^j)` means

```text
offset mod 2^j  ==  bits 17..17+j-1 of the LCG state
```

Low bits of a product never depend on high bits, so those bits are fixed by the
low `17 + j` bits of the seed alone. Sieve that small space against every
observation, then sweep the remaining high bits. Desert pyramids, igloos and
swamp huts have `chunkRange` 24, so `j = 3` and the sieve is 2²⁰ wide; four or
five of them usually leave a single survivor and a ~2²⁸ sweep. Ocean monuments
(range 27, odd) and anything on the power-of-two `nextInt` branch leak nothing
this way and are used only in the final check.

**Route 3 — lattice reduction, for single-generator observations.** See below.

Structure seeds are the low 48 bits. Structures, slime chunks and bedrock depend
only on those, so 65,536 world seeds share each one; mode 9 separates them with
biome observations at the end.

## Lattice reduction (`lll.rs`, `reverser.rs`)

A port of the idea behind [LattiCG](https://github.com/mjtb49/LattiCG), for the
case it actually addresses: **many observations of one `Random`**, rather than
one observation each of many.

Every observation says "the 48-bit state at call `k` lies in `[lo, hi]`". Since
`state_k = s·a^(k−k₀) + c_(k−k₀) (mod 2⁴⁸)`, the set of possible state vectors
is a lattice, and seed-finding becomes "which lattice points sit inside this
box". LLL-reduce, enumerate, done — cost proportional to the number of
*answers* rather than to 2⁴⁸.

Details worth knowing:

- **Exact rationals throughout** (`num-bigint`/`num-rational`). Floating-point
  LLL silently drops solutions, which is the one failure mode you cannot detect
  from the output.
- **Enumeration is Fincke–Pohst, not LattiCG's LP.** LattiCG tightens each
  branch with an exact-rational simplex. We instead enumerate the ball
  circumscribing the box and filter. Same completeness, much less machinery to
  get provably right; the price is a wider search tree. Every square root is
  bounded *outward* so intervals are never too narrow.
- **Not everything is a box.** `nextInt(n)` for non-power-of-two `n` is a
  modular condition, and "this block was mossy" is the complement of an
  interval. Neither goes into the lattice; both become filters replayed over
  each candidate. The lattice therefore never excludes a real answer.
- **Dungeons are the natural application** — `nextInt(16)`, `nextInt(16)`,
  `nextInt(height)`, then one `nextInt(4)` per floor block. On the versions
  where dungeon cracking applies (through 1.17, which is also where
  SeedCrackerX stops) the world height is 256, so every bound is a power of two
  and the whole query reduces to clean intervals. From 1.18 the height is 384,
  which is not, so that call would fall back to a replay filter.

## What is verified, and against what

Minecraft constants are easy to misremember, so nothing here is hardcoded from
memory. Every RNG formula is checked against an independent source in the test
suite (293 tests):

- **`java.util.Random`** — cross-checked draw-for-draw against the unrelated
  `java_random` crate, plus literal values from a real JVM. The modular inverse
  used for backwards stepping is computed by extended Euclid and asserted equal
  to the community's `0xDFE05BCB1365`.
- **Slime chunks** — checked against 108 vectors generated by compiling
  cubiomes' own C `isSlimeChunk`.
- **Nether bedrock** — layer seeds, position hash and per-block thresholds all
  checked against the published test vectors in
  [19MisterX98/Nether_Bedrock_Cracker](https://github.com/19MisterX98/Nether_Bedrock_Cracker).
- **Strongholds** — cubiomes' generator is asserted to produce 128 strongholds
  with ring 1 inside the wiki's documented 1,280–2,816 band.
- **End pillars** — the ten coordinates are checked against both the wiki table
  and the game's own angle formula.
- **Structure salts and spacing** — never reimplemented; taken from cubiomes,
  which has them right per version.

Several modes also carry end-to-end round trips: harvest real data out of the
generator, feed it back in, and assert the original answer comes back.

296 tests with `--features ocr`, 293 without.

### Three findings worth recording

**cubiomes' `isSlimeChunk` has signed-overflow UB.** It computes
`chunkX * chunkX * 0x4c1906` in `int`, which overflows for |chunkX| ≥ 21. Built
at `-O2` without `-fwrapv`, clang exploits that and returns a result 2³² away
from the wrapped one — disagreeing with the game for almost every chunk outside
spawn. Java has no such licence (JLS 15.17.1 defines int overflow as wrapping),
so mc-locate uses `wrapping_mul` and matches the game at every optimisation
level. The oracle vectors are generated with `-fwrapv` for this reason.

**Tesseract reads `0` as `@` on the F3 overlay.** Not a guess — the synthetic
overlay test produced `-1290.50@`, `6@ fps` and `-5@`. The digit-repair table
now covers it. The same run exposed a worse bug: the repair table and the regex
character class had drifted apart, so the regex stopped matching at the `@` and
the repair never ran. They are now generated from one macro with a test that
enforces agreement.

**The `cubiomes` crate's stronghold iterator drops one.** `Generator::strongholds()`
yields 127 of the 128, silently losing the first. mc-locate drives the C
iterator directly to get all of them, since losing a ring-1 entry would bias
mode 10's posterior.

## Deliberate gaps

Stated plainly rather than papered over:

- **Pre-1.18 nether bedrock pattern matching (mode 1a) is not implemented.**
  Before 1.18 bedrock was seeded from chunk coordinates alone, so the pattern
  *does* locate you without a seed — but reproducing it needs the exact order in
  which the surface builder consumes one shared per-chunk RNG across every
  column, and that order could not be verified from a primary source. Guessing
  would produce confident, wrong coordinates. Use
  [BedrockFinder](https://github.com/JorianWoltjer/BedrockFinder) or
  [user32dll/bedrock_finder](https://github.com/user32dll/bedrock_finder), which
  implement it and run on the GPU. The mode says this at runtime.
- **Mode 1b is a verifier and ranged search, not a from-scratch 2⁴⁸ cracker.**
  A full sweep needs the layered filter tree from Nether_Bedrock_Cracker. What
  is here is exact verification that composes with mode 9's pillar shortcut,
  which is the path that actually finishes.
- **The decorator crack narrows a candidate list; it is not yet a standalone
  cracker.** Mode 15 wires the lattice reverser to a dungeon (spawner position +
  floor), recovers the decorator seed, and undoes the salt to the chunk's
  population seed — all verified end to end. It then *filters* an existing
  candidate list (the End pillars' 2³², most usefully), which is what finishes.
  What is still not implemented is inverting `setPopulationSeed` standalone —
  `populationSeed = (blockX·a + blockZ·b) ^ worldSeed`, with `a`, `b` derived
  from the seed — to recover structure seeds from a population seed with no
  candidate list. That is its own algorithm (mjtb49's `ChunkRandomReverser`).
- **Mode 10 is a simpler model than Ninjabrain-Bot.** It does model the
  "eye points at the nearest stronghold" constraint, which matters — without it
  a far stronghold that happens to sit along the same bearing can take a third
  of the posterior. It does not fit a calibrated per-player error distribution;
  sigma is a prompt with a conservative default.
- **Mode 8 is fuzzy by nature** and always reports ranked candidates with a
  caveat, never a single answer.
- **Height matching (mode 2) uses cubiomes' approximate surface estimate**, not
  full terrain generation, hence the tolerance.

## Search sizes, honestly

Brute force is quoted up front with a measured rate and an ETA before anything
starts:

| Search | Size | Feasible? |
|--------|------|-----------|
| Mode 9 with pillar data | 2³² per pillar seed | Yes — minutes |
| Mode 4 full space | 2⁴⁸ | ~a day on 8 cores; use a range or mode 9 |
| Mode 1b full space | 2⁴⁸ | Same; ranged or candidate-filtered instead |
| Mode 1a full world border | ~3.6 × 10¹⁵ positions | No. Narrow with mode 11 or 6 first |

### Mode 12: what to look at next

The other modes answer "here is what I saw, what does it mean?". This one runs
the question backwards.

**With a candidate list**, the advice is exact rather than modelled. Any
proposed observation partitions your candidates into the ones that would say
yes and the ones that would say no; a 50/50 split is worth a full bit and
halves the list, a 99/1 split is worth almost nothing. So it evaluates real
candidate seeds against real nearby positions and ranks by the actual split:

```text
Best next observations, by how evenly they split your candidates:
   1. Look at nether floor block (-118, 4, 47)
        0.998 bits  ->  about 1041 candidates left (49.9% eliminated)  [from a screenshot]
   2. Check whether chunk (12, -41) is a slime chunk
        0.471 bits  ->  about 1614 candidates left (22.3% eliminated)  [travel or waiting]
```

**Without one**, there is nothing to partition, so it ranks by a-priori
information content: End pillars 16 bits, a structure origin ~9, a bedrock
block 0.72, a slime chunk 0.47, a *non*-slime chunk 0.15. Effort is tracked
separately and derived from each structure's region size, so a woodland mansion
is not advertised as a short trip just because it carries a lot of information.

It also **explains any candidate**: rather than short-circuiting at the first
failure the way the cracking hot path does, it evaluates every constraint and
reports which matched.

```text
Seed 12840895245824: 146 of 147 constraints matched.
  ✓ chunk (12, -41) is a slime chunk
  ✓ Desert pyramid at (1384, -2952)
  ✗ (11, 4, -97) is bedrock

  A near miss — only 1 constraint failed. That pattern usually means a
  mis-typed coordinate in those specific observations rather than a wrong seed.
```

Mode 9 offers the same breakdown inline on its results.

### Mode 13: persistence, and the mod interface

A session used to live only in memory — quitting discarded every coordinate you
had typed. Mode 13 saves it as plain JSON and reads it back, merging rather than
replacing so re-importing a file adds nothing and will not clobber a seed you
already have.

That same file is **the contract for anything else that wants to feed
mc-locate** — a Fabric mod dumping bedrock as you fly the Nether, a script, a
hand-written file:

```json
{
  "format": "mc-locate-observations",
  "version": 1,
  "bedrock":    [{"x": 11, "y": 4, "z": -97, "is_bedrock": true}],
  "structures": [{"type": "desert_pyramid", "x": 1384, "z": -2952}],
  "pillar_heights": [76, null, 82, null, null, 94, null, null, null, 103]
}
```

Every field is optional, unknown fields from a newer producer are ignored rather
than fatal, and structure names accept both cubiomes' spelling and the menu
labels. The producer never needs to know any of the maths.

It also **watches your screenshots folder**. Minecraft cannot be made to press
F2 for you, but the other half of that loop works: the moment the game writes a
new PNG, the F3 overlay is read out of it. The advisor says what to look at, you
press F2, and the observation arrives without typing.

### Mode 14: documentation, offline

Every mode's full write-up ships inside the binary — what it needs, where that
comes from in game, the actual formulas, its limits, and what it feeds next.
Plus an overview of how the modes chain and a glossary.

The mode list, the menu and the docs all read one registry, so a mode cannot be
added without documentation: [a test enforces it](src/modes.rs).

### Mode 15: cracking from a decorated feature

Every feature a chunk decorates itself with — an ore vein, a plant, a dungeon —
is placed by an RNG seeded from the chunk's *population seed*, a fixed function
of the world seed and the chunk. So a feature the server has shown your client
leaks the seed. The maths are transcribed from SeedFinding's canonical
`mc_core_java`, not guessed: `populationSeed = (blockX·a + blockZ·b) ^ worldSeed`
masked to 48 bits, then `decoratorSeed = populationSeed + index + 10000·step`.

The mode recovers the population seed — directly, from a decorator seed plus its
salt, or from a dungeon (spawner position + 7×7 floor) via the lattice reverser —
and then filters the session's candidate seeds. Because a population seed is 48
bits, one feature collapses the End pillars' 2³² candidates to a single seed. The
salt (`index + 10000·step`) is your input, never a hardcoded biome table: the
index shifts between versions, and a wrong salt would silently drop the true
seed. The full chain is round-trip tested through the real RNG and lattice.

The [exporter mod](jar-dev/mc-locate-exporter/) can be published to Modrinth and
CurseForge automatically — see its [PUBLISHING.md](jar-dev/mc-locate-exporter/PUBLISHING.md).

## The interface

It is a terminal program, and it tries to be a good one.

* **Colour is semantic**, not decorative: one style per role (heading, result,
  warning, literal value) rather than per-call improvisation.
* **`NO_COLOR` and pipes are respected.** All styling goes through the
  `console` crate, so redirecting output to a file gives clean text rather than
  a mess of escape sequences. An earlier version wrote escapes by hand and got
  this wrong everywhere.
* **Prose wraps to your terminal** instead of running off the right edge, and
  boxes and rules size themselves to the window.
* **Results are marked** — `→` for a recovered answer, `✓` / `!` / `✗` for
  outcome — so the thing you came for is findable in a wall of explanation.
* **The status bar** shows what the session is carrying between modes.

## Version support

**Beta 1.7 through 26.2** — 34 versions, up to and including current Minecraft.

Every version-specific constant (structure salts, region grids, biome
pipelines) comes from cubiomes rather than being hardcoded. That is deliberate:
a stale salt produces confident nonsense.

The published `cubiomes` crates vendor Cubitect's C library, which has been
dormant since November 2024 and stops at 1.21.4 — while Minecraft moved to
year-based versions and is now on 26.2. So this builds against
[`xpple/cubiomes`](https://github.com/xpple/cubiomes), an actively maintained
fork, via a `[patch.crates-io]` pointing at
[a fork of the Rust wrapper](https://github.com/LAOUUUUU/cubiomes-rs) whose only
change is the submodule it points at. Every symbol is signature-compatible, and
the C revision is pinned rather than tracked, because that fork is under active
refactoring.

A test asserts the backend still reaches 26.2, so a dependency change that
quietly reverted it — which would make every modern-world answer wrong with no
other symptom — fails loudly instead.

**Newer than 26.2?** Pick "Newer than 26.2" in the version menu. Modes that
generate world data refuse it rather than substituting a nearby version and
giving confident, wrong answers. Everything that does not consult the generator
keeps working on any version at all:

* **Slime chunks** (mode 4) — those constants are unchanged since Beta 1.4
* **Nether bedrock** (mode 1) — the 1.18+ per-position formula
* **End pillars** and the seed maths in mode 9, **portal conversion** (mode 11)

Note that **1.21.4 is what cubiomes calls "1.21 WD"** — the constant was written
before the Winter Drop shipped and never renamed.

Older versions carry real differences the tool accounts for rather than
papering over:

* **Before 1.9** a world has **three** strongholds, not 128 across 8 rings, so
  mode 10's ring prior does not apply and says so.
* **Beta** has no surface-height approximation in cubiomes, so mode 2 offers
  biome patterns only rather than failing several prompts later.
* **Before 1.13** desert pyramids, swamp huts and igloos shared one salt.
* **Before 1.18** nether bedrock is not seed-dependent, so mode 1b cannot work.

## Built on

- **[cubiomes](https://github.com/Cubitect/cubiomes)** (Cubitect) — the C
  biome and structure generator every version-specific constant comes from.
  Statically linked; MIT, notice reproduced in [LICENSE](LICENSE).
- **[LattiCG](https://github.com/mjtb49/LattiCG)** (mjtb49 et al.) — the
  lattice reversal approach `lll.rs` and `reverser.rs` are a port of.
- **[Nether_Bedrock_Cracker](https://github.com/19MisterX98/Nether_Bedrock_Cracker)**
  (19MisterX98) — source of the verified 1.18+ bedrock algorithm and its test
  vectors.
- **[SeedcrackerX](https://github.com/19MisterX98/SeedcrackerX)** — reference
  for how the constraint sources actually compose, including the bit-lifting
  route.
- **[Ninjabrain-Bot](https://github.com/Ninjabrain1/Ninjabrain-Bot)** — the
  Bayesian stronghold model mode 10 follows in spirit.

## Contributing

CI runs the full suite plus clippy on Linux, macOS and Windows for every push,
so a green tick means it genuinely builds and passes everywhere. To cut a
release, tag it:

```bash
git tag v0.10.0 && git push origin v0.10.0
```

That one push builds the CLI binaries and all the exporter-mod jars, publishes
the GitHub release, and — when the mods are armed (`vars.PUBLISH_MODS`) — pushes
every jar to Modrinth and CurseForge. See
[PUBLISHING.md](jar-dev/mc-locate-exporter/PUBLISHING.md).

## A note on use

Seed cracking is fine in single-player and on worlds you own. It violates the
rules of many multiplayer servers and enables griefing. Server operators can
defend with recent versions, randomised structure seeds, hashed-seed spoofing,
or a secure-seed mod.
