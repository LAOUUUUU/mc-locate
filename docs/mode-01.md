# Mode 1 — Nether Bedrock Toolkit

Two sub-modes over the Nether's bedrock floor (y 0–4) and roof (y 122–127).

## 1a — coordinates from a pattern (seed known)

**Needs:** your world seed, the version, and an ASCII grid of what you can see.
`#` for bedrock, `.` for definitely-not-bedrock, `?` for anything you cannot
make out. Rows run along +Z, columns along +X.

**Where it comes from:** stand on the nether floor or under the roof and
transcribe a patch, or read it off a screenshot. **Record y=4 or y=123** — those
are the layers where bedrock is rarest, so each block carries the most
information.

**How it works.** Since 1.18 each block position gets its own `Random`:

    h = (x * 3129871) ^ (z * 116129781) ^ y
    h = h*h*42317861 + h*11
    value = new Random(layerSeed ^ (h >>> 16)).nextFloat()

Bedrock when `value < (5 - y)/5` on the floor, or `value >= (5 - (y-122))/5` on
the roof. So y=0 and y=127 are always bedrock, y=5 and y=122 never are, and
y=4/y=123 are 20% — the informative middle.

Because each position is independent, the search just slides your pattern over
a region and tests it. Cells are ordered rarest-first so most positions are
rejected after one test.

**Limits.** The full world border is ~3.6 x 10^15 positions — weeks of CPU. You
must bound the search; mode 11 (from a nether coordinate) or mode 6 are the
usual ways. The mode measures throughput and tells you the ETA before starting.

**Pre-1.18 is not implemented.** Before 1.18 bedrock came from the chunk
coordinates alone with no world seed, so the pattern *does* locate you without a
seed — but reproducing it needs the exact order one shared per-chunk RNG is
consumed across every column, and that could not be verified from a primary
source. Guessing would produce confident, wrong coordinates. Use
JorianWoltjer/BedrockFinder for those versions.

## 1b — crack the seed from bedrock (1.18+ only)

**Needs:** observations as `X Y Z B`, one per line, where B is 1/# for bedrock
and 0/. for not.

**Collect from both floor and roof.** They use independent layer seeds, so
mixing them cuts false positives sharply.

**How it works.** The layer seeds come from the world seed:

    rand      = new Random(worldSeed).nextLong()
    floorSeed = new Random(rand ^ hash("minecraft:bedrock_floor")).nextLong()
    roofSeed  = new Random(rand ^ hash("minecraft:bedrock_roof")).nextLong()

so any candidate seed can be checked exactly against your observations.

**Limits.** This is a verifier and a ranged search, not a from-scratch 2^48
cracker — that needs the layered filter tree from
19MisterX98/Nether_Bedrock_Cracker. It composes with mode 9, which is the route
that actually finishes.

**Feeds:** a candidate seed list, or a filtered one.
