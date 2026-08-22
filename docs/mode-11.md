# Mode 11 — Nether ↔ Overworld Portal Converter

Pure arithmetic, no search, no seed. The quickest way to confirm the tool works.

**Needs:** one coordinate and which dimension it is in.

**How it works.** The Nether runs at 1:8 horizontally; Y is unscaled.

The rounding direction is the classic trap: the game uses floor division, not
truncation toward zero. Overworld -1290 maps to nether **-162**, not -161.
Rust's `/` would get this wrong, so every conversion here goes through
`div_euclid`.

**Why it emits a box rather than a point.** Walking through a portal does not
land you on the exact converted block. The game looks for an existing portal
near the ideal destination and only builds a new one if it finds none — within
128 blocks going *to* the Nether, but up to 1024 going back to the Overworld.
The search areas are asymmetric, and the mode reports the right one for your
direction.

Going Nether → Overworld also multiplies your uncertainty by eight: each nether
block covers 64 overworld blocks of area, so a nether coordinate read off a
screenshot pins you to within 8 blocks before the portal search radius is even
applied.

**Feeds:** a search box for modes 2 and 6. This is the standard way to turn a
nether coordinate glimpsed in a screenshot into somewhere worth searching.
