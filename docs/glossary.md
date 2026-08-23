# Glossary

**World seed** — the full 64-bit number Minecraft shows you. Only the low 48
bits affect most generation.

**Structure seed** — the low 48 bits of the world seed. Structures, slime
chunks, nether bedrock and End pillars depend only on these, so cracking them
gives you a structure seed. 65,536 world seeds share each one; biome data
separates them.

**LCG** — linear congruential generator. Java's is
`state = state * 0x5DEECE66D + 0xB mod 2^48`. Invertible, which is the whole
basis of this tool.

**`next(bits)`** — the LCG's primitive: step once, return the top `bits` of the
48-bit state. `nextInt`, `nextDouble` and friends are built on it.

**Region** — structures are placed on a grid. The world is divided into
`region_size` x `region_size` chunk cells, and each cell gets one placement
attempt whose offset comes from two `nextInt` calls seeded from the region
coordinates, the world seed, and a per-structure **salt**.

**Salt** — a constant unique to each structure type, mixed into its region
seed. These changed at 1.13 and vary by version; mc-locate never hardcodes
them, it reads them from cubiomes.

**Generation attempt** — the position the RNG picks for a structure in a
region. It becomes a real structure only if the biome there is suitable.

**Bit-lifting** — recovering the low bits of a seed first, cheaply, then
sweeping the rest. Works because `nextInt(n) mod 2^j` depends only on the low
bits of the seed when `2^j` divides `n`.

**Pillar seed** — `new Random(worldSeed).nextLong() & 65535`. Determines the
End pillar arrangement. Only 65,536 possibilities, so observing the pillars
identifies it outright and fixes 16 bits of the seed.

**Decorator seed** — the seed used for per-chunk features like dungeons,
derived from the world seed via the population seed. Recovering a world seed
from one needs an extra inversion step that mc-locate does not yet implement.

**Lattice reduction (LLL)** — a way to find short vectors in a lattice. Used to
reverse many observations of a *single* `Random` at once, rather than testing
seeds one by one.

**Winter Drop / 1.21.4** — cubiomes names this version `MC_1_21_WD` with the
comment "version TBA", because the constant predates the release. It is 1.21.4,
"The Garden Awakens" (December 2024). mc-locate labels it 1.21.4.

**cubiomes** — the C library that reproduces Minecraft biome and structure
generation. Every version-specific constant in mc-locate comes from it,
deliberately, so nothing is hand-rolled from memory. mc-locate builds against
`xpple/cubiomes`, a maintained fork reaching 26.2; Cubitect's original has been
dormant since November 2024 and stops at 1.21.4.
