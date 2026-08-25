# Mode 15 — Decorator / Population-Seed Crack

Every feature a chunk decorates itself with — ore veins, flowers, dungeons,
geodes — is placed by a random number generator seeded from the chunk's
**population seed**. That population seed is a fixed function of the world seed
and the chunk's coordinates, so a feature the server has shown your client is a
leak of the world seed. This is the technique SeedCrackerX is built around, and
it is entirely a leak of *world* data: nothing here reads another player.

## The chain

```
world seed ──setPopulationSeed(seed, chunkX*16, chunkZ*16)──▶ population seed
population seed ──+ (index + 10000*step)──▶ decorator seed
decorator seed ──new Random(..)──▶ the feature's placement draws
```

Reading it backwards: the lattice reverser recovers the **decorator seed** from
a feature's draws (a dungeon's spawner offset and floor pattern, say); undoing
the salt gives the **population seed**; and because `setSeed` keeps only the low
48 bits, that population seed pins the low 48 bits of the world seed — a
*structure seed*, the same thing the End pillars and Nether bedrock recover.

The formulas are transcribed from SeedFinding's canonical `mc_core_java`, not
guessed. The one quantity this mode will not hardcode is the **salt**
(`index + 10000 * step`): a feature's index is its position in a biome's
decoration list and moves between versions, so you supply it rather than trust a
baked-in table that might be quietly wrong.

## How to use it

A population seed is a 48-bit filter, so its job is to **narrow a list** — most
naturally the 2³² candidates the End pillars produce (mode 9). Get that list
first, then:

1. Enter the chunk the feature is in.
2. Give the population seed — directly, as a decorator seed plus its salt, or by
   letting the mode recover it from a dungeon (spawner position + 7×7 floor
   pattern of cobblestone/mossy).
3. The mode keeps only the candidate seeds that generate that exact feature at
   that chunk. One feature is normally enough to leave a single seed.

Without a candidate list there is nothing to filter: the mode reports the
population seed and points you at the pillars first. A wrong chunk coordinate or
a wrong salt shifts the population seed and eliminates the true seed, so the mode
leaves your candidates untouched when nothing matches rather than guessing.
