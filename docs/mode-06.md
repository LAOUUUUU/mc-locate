# Mode 6 — Structure-Relative Search Narrower

Turns "I know the seed and I can see a village" into a box mode 2 can afford.

**Needs:** seed, version, a structure type, and a search centre and radius.

**How it works.** Enumerates the structure's real generated positions from
cubiomes, which knows the correct salt, region size and chunk range for every
version. Nothing is hand-rolled — that is the whole point of this mode existing
rather than you doing the arithmetic.

**Output** is every position found, nearest first, with distance and the paired
Nether coordinate for travel planning. Pick one and get a box around it, or take
the tight box enclosing all of them plus a margin.

**Limits.** Structures whose placement cubiomes cannot reproduce are refused
cleanly rather than guessed at. Absurd radii are refused with an explanation
rather than run for an hour.

**Feeds:** a search box for mode 2, and structure observations for mode 9 — with
the caveat, stated in the mode itself, that positions *derived from a known seed*
are only useful as constraints when testing a different candidate seed.
