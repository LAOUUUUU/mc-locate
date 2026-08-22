# Mode 2 — Overworld Terrain Shape Matcher

You know the seed but not where you are. Transcribe what you can see and this
finds where it fits.

**Needs:** seed, version, a bounded search box, and one of two pattern kinds:

* **Biome grid** — biome names separated by spaces, `?` for unknown. Names are
  matched leniently: case, spaces and hyphens all work, and numeric ids too.
* **Height grid** — numbers, `?` for unknown, matched within a tolerance.

**Where it comes from:** walk a rough grid reading F3's biome line, or
transcribe a height profile from a screenshot.

**Cell scale** matters more than anything else for speed. One cell can cover 1
block, 4 blocks (cubiomes' native biome scale) or 16 (a chunk). Coarser is
dramatically faster and usually enough.

**The box is mandatory.** The world is ~60 million blocks across; unbounded
search is not a search. Modes 6, 8, 10 and 11 all exist partly to produce the
box this mode consumes, and the session hands it over automatically.

**Limits.** Height data comes from cubiomes' `mapApproxHeight`, an *estimate*
of the surface rather than full terrain generation — good enough to match the
shape of a ridgeline, not block-exact. Hence the tolerance, and hence results
are a ranked list rather than an answer.

Ties are reported honestly: if forty placements match equally the mode says so
rather than picking the first and looking confident.

**Feeds:** a tightened search box.
