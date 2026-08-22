# Mode 9 — Multi-Source Seed Cracker

The destination mode. No single observation cracks a seed cheaply; combining
independent ones multiplies their power. This is what SeedCrackerX does, and
mc-locate follows the same structure.

**Needs:** any mix of End pillar heights, structure positions, slime chunks and
nether bedrock.

## Route 1 — the End pillar shortcut

The ten obsidian pillars are the cheapest 16 bits in the game:

    pillarSeed = new Random(worldSeed).nextLong() & 65535
    Collections.shuffle(order, new Random(pillarSeed))

Only 65,536 arrangements exist, so the pillar heights identify the pillar seed
outright. That is not merely a filter. `pillarSeed` is the low 16 bits of
`nextLong()`, which is bits 16..31 of the LCG state two steps in — and because
the LCG is invertible, mc-locate *enumerates* exactly the 2^32 structure seeds
consistent with it and steps backwards to each. A 2^48 problem becomes 2^32:
days become minutes.

**If you can get to the End, do this first.** Nothing else on the list compares.

## Route 2 — structures alone, by bit-lifting

No End trip needed. When `2^j` divides a structure's chunk range,
`offset mod 2^j` equals bits 17..17+j-1 of the LCG state, and low bits of a
product never depend on high ones — so those bits are fixed by the low `17+j`
bits of the seed alone. Sieve that small space, then sweep the rest.

Desert pyramids, igloos and swamp huts have chunk range 24, giving three
liftable bits and a 2^20 sieve. Four or five of them usually leave one survivor
and a ~2^28 sweep. Ocean monuments (range 27, odd) and anything on the
power-of-two `nextInt` branch leak nothing this way and are used only in the
final check.

**Positions must be exact.** Lifting has no tolerance to spend: one wrong chunk
eliminates the true seed silently.

## Route 3 — filtering an existing candidate list

Slime chunks and bedrock are strong filters. If you already have candidates
from mode 4 or 1b, this narrows them cheaply.

## Recovering the full world seed

All of the above give a *structure seed* — the low 48 bits. 65,536 world seeds
share each one. Give the mode a few biome observations at spread-out
coordinates and it brute-forces the remaining 16 bits against them.

**Also offers**, on its results, the per-constraint breakdown from mode 12 — a
near miss at 146/147 almost always means one mis-typed coordinate rather than a
wrong seed.
