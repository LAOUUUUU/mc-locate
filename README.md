# mc-locate

Reverse-engineering Minecraft **Java Edition** seeds and coordinates from limited
in-game observations — screenshots, footage, and data you can collect by walking
around.

It all rests on one fact: Java worldgen is driven by `java.util.Random`, a 48-bit
linear congruential generator. It is not cryptographic, its state is small, and
because the multiplier is odd it is **invertible** — you can step it backwards as
cheaply as forwards. A handful of independent observations is often enough to
collapse the space to a single seed.

## Build

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
coordinate entry instead.

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

### Mode 9 and the End pillar shortcut

Mode 9 is the one to aim for. It intersects independent constraints instead of
brute-forcing any single one, which is how SeedCrackerX works.

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

So: **go to the End, record the ten pillar heights, and start from there.**
Without pillar data the space is 2⁴⁸ and the tool says so plainly rather than
starting a search that will not finish.

Structure seeds are the low 48 bits. Structures, slime chunks and bedrock depend
only on those, so 65,536 world seeds share each one; mode 9 separates them with
biome observations at the end.

## What is verified, and against what

Minecraft constants are easy to misremember, so nothing here is hardcoded from
memory. Every RNG formula is checked against an independent source in the test
suite (157 tests):

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

### Two findings worth recording

**cubiomes' `isSlimeChunk` has signed-overflow UB.** It computes
`chunkX * chunkX * 0x4c1906` in `int`, which overflows for |chunkX| ≥ 21. Built
at `-O2` without `-fwrapv`, clang exploits that and returns a result 2³² away
from the wrapped one — disagreeing with the game for almost every chunk outside
spawn. Java has no such licence (JLS 15.17.1 defines int overflow as wrapping),
so mc-locate uses `wrapping_mul` and matches the game at every optimisation
level. The oracle vectors are generated with `-fwrapv` for this reason.

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
- **No LattiCG.** The lattice-reduction reversal that makes sparse constraints
  cheap is not ported; mode 9 uses candidate-set intersection, which is
  legitimate and sufficient given the pillar shortcut.
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

## A note on use

Seed cracking is fine in single-player and on worlds you own. It violates the
rules of many multiplayer servers and enables griefing. Server operators can
defend with recent versions, randomised structure seeds, hashed-seed spoofing,
or a secure-seed mod.
