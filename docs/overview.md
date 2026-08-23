# How mc-locate fits together

Every mode answers one of two questions:

* **Where am I?**  — turn an observation into a *coordinate*.
* **Which world is this?** — turn observations into a *seed*.

They are not separate tools. Coordinate modes narrow the area a seed search has
to cover, and a recovered seed makes every coordinate question trivial. The
session carries whatever one mode learns into the next, so you type a seed,
version, heading or search box once.

## The one fact everything rests on

Minecraft Java's world generation is driven by `java.util.Random`: a 48-bit
linear congruential generator.

    state = (state * 0x5DEECE66D + 0xB) mod 2^48

It is not cryptographic, its state is small, and the multiplier is odd — which
makes it invertible. You can step it backwards exactly as cheaply as forwards.
Everything here is a consequence of that.

Because `setSeed` masks to 48 bits, most of world generation only ever sees the
low 48 bits of your 64-bit world seed. That lower half is called the
**structure seed**, and 65,536 world seeds share each one. Biome generation is
what separates them.

## Typical routes

**I know the seed, I want to know where I am.**
Mode 6 (find a structure you can see) or mode 11 (convert a nether coordinate)
to get a small box, then mode 2 to match terrain inside it. Mode 1a if you have
a nether bedrock pattern. Mode 10 if you want a stronghold.

**I don't know the seed.**
Mode 9 is the destination. Feed it whatever you have:

* End pillar heights — the single best observation in the game, 16 bits from
  one visit, and the route that finishes in minutes.
* Four or five structures — the bit-lifting route, no End trip needed.
* Slime chunks and nether bedrock — strong filters that narrow a candidate list.

Not sure what to collect? Mode 12 tells you, and with a candidate list its
advice is exact rather than estimated.

**I have screenshots or logs rather than typed notes.**
Mode 3 reads F3 overlays, mode 7 scrapes chat and logs (live or from a file),
mode 13 watches your screenshots folder and reads new ones as you take them.

## When Minecraft is newer than the generator

Every version-specific constant comes from cubiomes. The bundled build reaches
26.2 — current Minecraft — but the game will move ahead again. Rather than let
you pick a nearby version and quietly generate the wrong world, the version menu
has a "Newer than ..." entry: generator-backed modes then refuse, and the rest
carry on.

Slime chunks, nether bedrock, End pillars and portal maths never consult the
generator, so they work on any version at all.

## Saving your work

Mode 13 writes everything to a JSON file and reads it back. That same format is
the contract any external producer writes to — a mod, a script, anything.
