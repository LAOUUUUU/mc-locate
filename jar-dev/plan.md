# mc-locate — development plan

> **Status: Part 1 complete as of v0.5.0.** Tiers 1–4 all landed. What actually
> happened is recorded at the end under "Outcome"; the tier write-ups below are
> kept as they were written, because the reasoning still explains *why* the work
> was shaped this way. Part 2 (the Fabric mod) is still not started.

Working notes for two pieces of work:

1. **Version support** — getting from 14 supported Minecraft versions to 34,
   including current Minecraft.
2. **The Fabric mod** — a `.jar` that feeds observations into mc-locate.

Everything below was verified on **2026-08-22**; the research section records
what was checked so none of it has to be re-derived later.

---

# Part 1 — Version support

## Where we are

| | Newest supported |
|---|---|
| mc-locate's menu | 1.21.3 |
| cubiomes (crate `0.3.3` **and** upstream master) | 1.21.4 |
| Actual Minecraft | **26.2** (26.3 in snapshots) |

Two things to know about that table.

**"1.21 Winter Drop" is 1.21.4.** cubiomes calls the constant `MC_1_21_WD`
with the comment `// Winter Drop, version TBA`, written before the version
shipped and never renamed. It is the December 2024 release, officially "The
Garden Awakens", which added the Pale Garden and the Creaking. It is not
cosmetic: cubiomes uses it to select a *different biome tree*
(`biomenoise.c`, `bt = &btree21wd`) as well as to enable `pale_garden`
(id 186). Generating a 1.21.4 world as 1.21.3 gives wrong biomes generally,
not merely a missing biome.

**Minecraft moved to year-based version numbers in 2026.** There is no 1.22;
the line runs 1.21.11 → 26.1 → 26.2.

## The key finding

**Cubitect's cubiomes has been dormant since 2024-11-10** (verified against
the commit API). So has cubiomes-viewer, which pins Cubitect's cubiomes as a
submodule.

**`xpple/cubiomes` is an actively maintained fork** — last push 2026-08-21, 51
stars — and supports up to `MC_26_2`:

```c
MC_1_21_WD, MC_1_21_4 = MC_1_21_WD,
MC_1_21_5,
MC_1_21_6,
MC_1_21_9,
MC_1_21_11,
MC_1_21 = MC_1_21_11,
MC_26_1,
MC_26_2,
MC_NEWEST = MC_26_2,
```

It also renames `MC_1_21_WD` to `MC_1_21_4`, confirming the reading above.

**Every C symbol mc-locate depends on exists in the fork with an identical
signature** — checked one by one:

```
getStructureConfig    getStructurePos       isViableStructurePos
initFirstStronghold   nextStronghold        isSlimeChunk
mapApproxHeight       setupGenerator        applySeed
getBiomeAt            genBiomes             biome2str
```

So switching backends is a build-system change, not a rewrite.

## Tier 1 — expose the versions already in the box

cubiomes implements 14 versions the menu does not list:

    Beta 1.7, Beta 1.8, 1.0, 1.1, 1.2.5, 1.3.2, 1.4.7, 1.5.2,
    1.6.4, 1.7.10, 1.9.4, 1.10.2, 1.11.2, 1.21.4

**Effort:** ~1 hour. **Buys:** 14 → 28 versions.

**The trap:** the safe wrapper *panics* rather than erroring for beta versions:

```rust
if self.minecraft_version() == MCVersion::MC_B1_7
    || self.minecraft_version() == MCVersion::MC_B1_8
{ panic!("Surface height approximation not currently supported for beta minecraft") }
```

So mode 2's height matching must refuse Beta before calling it. Adding beta
versions without that guard turns a menu choice into a crash. Needs a test.

## Tier 2 — swap the backend to xpple/cubiomes

**Effort:** ~half a day. **Buys:** 28 → 34, including current Minecraft
(1.21.5, 1.21.6, 1.21.9, 1.21.11, 26.1, 26.2).

Approach: **vendor** the fork's C into the repo and write a thin `-sys` layer
(`cc` + `bindgen`), replacing the `cubiomes` and `cubiomes-sys` crates. The
small safe surface actually used (`Generator`, `Cache`, `Range`,
`BlockPosition`) gets reimplemented; `StructureRegion` is already bypassed on
the hot paths.

Build list grows from 8 `.c` files to 11 — the fork adds `carver.c`,
`terrainnoise.c`, `xradv.c`.

**Vendor a pinned commit rather than tracking master.** xpple is actively
refactoring internals ("uint8_t instead of int for blocks array", "Refactor
terrain generation"), so following master invites silent breakage. Pinning also
keeps CI builds self-contained with no submodule fetch.

**Risk to state plainly:** the existing 261 tests verify the *integration* —
the 108-vector slime oracle and the structure cross-checks in `lifting.rs`
would catch a mis-wired build immediately. They cannot verify that the fork's
26.2 biome tree is *correct*. Only a real 26.2 world confirms that. This is
trusting xpple's work, which is reasonable given the reputation, but it is
trust rather than verification.

**Licensing:** the fork is MIT. Its notice goes in `LICENSE` alongside
Cubitect's, and the vendored tree keeps its own `LICENSE` file.

## Tier 3 — stop the generator gating everything

`Version` currently gates the whole tool, but several modes never touch
cubiomes:

| Mode | Depends on cubiomes? |
|---|---|
| 4 slime chunks | **No** — constants unchanged since Beta 1.4 |
| 1 nether bedrock | **No** — the 1.18+ per-position formula |
| 9 End pillars, 11 portal maths | **No** |
| 2, 6, 8, 10 | Yes — biomes, salts, stronghold rings |

So today someone on 26.2 could crack a seed from slime chunks and bedrock, and
the menu simply will not let them pick their version.

Split "generator-backed version" from "any version": cubiomes-dependent modes
refuse an unsupported version with a clear message, everything else works.

**Effort:** ~half a day. **Buys:** partial support for *every* version,
including ones nobody has ported — and it keeps working the next time
Minecraft outruns the backend, which it will.

## Tier 4 — make the next gap visible

A test that asserts the vendored generator's newest version against a recorded
"current Minecraft as of <date>", so drift surfaces as a failing test rather
than a silently wrong answer.

**Effort:** ~1 hour.

## Recommended order

**1 → 3 → 2 → 4.**

Tier 1 is free. Tier 3 goes before Tier 2 because it is independent and
lower-risk, and it makes the tool useful on 26.2 *even if* the backend swap
runs into trouble. Tier 2 is the big win and the only one that can go wrong.
Tier 4 stops this recurring.

Total: roughly a day and a half for 14 → 34 versions plus partial coverage of
everything else.

---

# Part 2 — the Fabric mod

## What it is for

Not another seed cracker. **SeedCrackerX already is that** — Fabric, mixins
into chunk loading, structure finders, pillar detection. Rebuilding it would
redo solved work.

What mc-locate has that SeedCrackerX does not is coordinate recovery (bedrock
1a, terrain matching, camera pose, portal maths), the Bayesian stronghold
triangulator, and the observation advisor. So the mod should be a **thin
exporter** feeding those, not a cracker.

## The interface already exists

`src/observations.rs` defines the format and it is built and tested. The mod
writes this; mc-locate imports it in mode 13:

```json
{
  "format": "mc-locate-observations",
  "version": 1,
  "minecraft_version": "26.2",
  "bedrock":    [{"x": 11, "y": 4, "z": -97, "is_bedrock": true}],
  "slime":      [{"chunk_x": 5, "chunk_z": -3, "is_slime": true}],
  "structures": [{"type": "desert_pyramid", "x": 1384, "z": -2952}],
  "pillar_heights": [76, null, 82, null, null, 94, null, null, null, 103]
}
```

Every field is optional; unknown fields from a newer producer are ignored
rather than fatal, because the mod and the tool will be versioned separately
and will drift. Import merges rather than replaces, so re-importing a dump is
idempotent.

There is also a **zero-integration path**: mode 7 already tails `latest.log`
live, so a mod that merely *prints* observations to the console is consumed
today with no changes on the Rust side.

## Scope — and what to leave out

Easy, because the client already has the blocks:

* **Nether bedrock** at y=4 and y=123 from loaded chunks — the highest yield
  per minute of play, dozens of observations from one flight.
* **End pillar heights** — ten known positions, 16 bits from one visit.
* **Coordinates, heading, biome, dimension** — trivially available.

Hard, and not worth it now:

* **Structure detection.** The client is never told "village here"; it gets
  blocks. SeedCrackerX pattern-matches loaded chunks with a per-structure
  finder — real work times fifteen structure types.
* **Slime chunks** need observed spawns; not reliably automatable.

**Scope it to bedrock, pillars and coordinates.** Those are version-independent
(see Tier 3), so they work on 26.2 *today*, whereas structure observations
would feed constraints mc-locate cannot currently evaluate on 26.x.

## Toolchain, as of 2026-08-22

Targeting **26.2**:

| | |
|---|---|
| Fabric Loader | 0.19.3 |
| Loom | 1.17 |
| Gradle | 9.5.1 |
| Java | 21 |

For 26.1 it was Loader 0.18.4 / Loom 1.15 / Gradle 9.4.0.

Start from the [template generator](https://fabricmc.net/develop/template/) or
[fabric-example-mod](https://github.com/FabricMC/fabric-example-mod).

**Two warnings.** 26.1 carried "the largest tooling and API changes ever made
for a single release", so any tutorial predating March 2026 is likely wrong.
And Fabric's in-world rendering events were removed pending a replacement, so
render-adjacent work currently needs mixins — relevant because 26.1 was the
last OpenGL-only release and 26.2 snapshots let the backend switch to Vulkan.

## The honest constraint

Everything else in this repository was verified by running it. **A Fabric mod
cannot be**, not from here — it needs Minecraft launched and flown around.
Whoever writes it ships something to be tested rather than something tested.
PrismLauncher and JDK 21 are already installed on the dev machine, so testing
is available to the human.

---

# Research log

Checked 2026-08-22, so it does not need re-deriving:

* Cubitect/cubiomes — last commit **2024-11-10**, `MC_NEWEST = MC_1_21`
  (= 1.21.4). Dormant.
* Cubitect/cubiomes-viewer — last commit 2024-11-10, submodule pinned to
  Cubitect/cubiomes.
* xpple/cubiomes — last push **2026-08-21**, `MC_NEWEST = MC_26_2`. All 12
  required symbols signature-compatible. Adds `carver.c`, `terrainnoise.c`,
  `xradv.c`.
* Minecraft current release **26.2** (June 2026); 26.3 in snapshots since June
  2026. Year-based numbering adopted 2026.
* Winter Drop = **1.21.4**, December 2024, "The Garden Awakens".
* Java Edition is replacing OpenGL with Vulkan; 26.1 was the last OpenGL-only
  release. **Irrelevant to worldgen** — rendering is presentation; every
  formula here runs before a pixel is drawn. Only modes 3 (F3 OCR) and 5
  (camera pose) touch pixels and might need retuning.

---

# Outcome — Part 1, delivered in v0.5.0

**Beta 1.7 → 26.2, 34 versions**, up from 14. All four tiers landed. Three
things went differently from the plan and are worth recording.

## Tier 2 was done by forking the wrapper, not vendoring the C

The plan called for vendoring xpple's C and hand-writing a `-sys` layer.
`cubiomes-rs` turned out to be a clean workspace with the C as a *submodule*,
so forking it and repointing that submodule was a two-line change instead of
roughly 500 lines of new binding code — same result, far less to get wrong, and
upstreamable. The fork is `LAOUUUUU/cubiomes-rs`; its only changes are the
submodule URL and three extra `.c` files in the build list
(`carver.c`, `terrainnoise.c`, `xradv.c`).

mc-locate points at it with `[patch.crates-io]` pinned by revision.
`Cargo.lock` records the exact commit, and `--locked` builds work.

**The evidence the swap was safe:** all 269 pre-existing tests passed against
the new backend *before* any versions were added, including the 108-vector
slime oracle and the structure cross-checks across four versions.

## Tier 1 hid two real bugs

Exposing the old versions naively would have shipped both:

* **Pre-1.9 worlds have three strongholds, not 128.** cubiomes returns
  `(mc >= MC_1_9 ? 128 : 3)`; the iterator capped at a hardcoded 128 and mode
  10's ring prior assumes an 8-ring layout that did not exist yet.
* **Beta *panics* on the height map** — the wrapper literally calls `panic!`,
  so picking Beta 1.8 in mode 2 would have crashed the program.

Both are now guarded, with tests. Version boundary checks were also rewritten
as `at_least(MCVersion)` comparisons, since cubiomes declares its enum in
release order — one less hand-maintained list.

## The 26.2 regression test initially failed, and the test was wrong

It asserted 26.2 generates differently from 1.21.4, sampling at y=63, and
failed. The instinct was "the patch is not applying". The C says otherwise:
26.2's addition is `sulfur_caves`, id 187 — a **cave** biome. At y=10 the
difference is obvious (13 hits, and 1.21.4 never produces it). The surface is
genuinely unchanged.

A separate false alarm: the bindings appeared to lack `MC_26_2` because the
grep hit a stale build directory from before the patch.

## Tier 3 needed a second pass

The first pass added `ChosenVersion` and taught mode 1 to accept a version past
the ceiling, but modes 9 and 12 still called `prompt_version` and so refused
outright — locking a modern-version player out of the End-pillar route, slime
and bedrock constraints, and all of the advisor, none of which touch the
generator. `ConstraintSet::build` now takes `Option<Version>`, drops structure
constraints with a count rather than silently, and both modes degrade instead
of refusing.

## Tier 4 arrived by accident

A Tier 3 test asserting the ceiling was 1.21.4 failed when Tier 2 moved it —
exactly the drift guard Tier 4 called for. It is now a regression check: if a
dependency change silently reverts to the dormant upstream, every modern-world
answer would be wrong with no other symptom, and this fails loudly instead.

## Known risk carried forward

The build now depends on a **git dependency with a submodule**, so CI needs
network at build time. `--locked` works locally and the lockfile pins the
revision, but the first CI run on `main` is the real test.
