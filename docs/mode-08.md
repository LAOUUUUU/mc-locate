# Mode 8 — Compass + Biome Triangulation Estimator

Two stages: work out which way you are facing, then find where along that
heading your biome sequence occurs.

**Needs:** an orientation cue (or a heading from mode 5), the seed, an anchor
point, and the biomes you crossed with rough spacing if you know it.

**Stage 1 — heading.** Offers cues that were each checked against the Minecraft
Wiki before being included: the sun and moon's path, cloud drift, the "T" on
stone-brick-family top faces, sunflower facing, the fletching table's feather,
and the block-breaking overlay's stem. One folklore cue — the cobblestone "L"
pointing a particular way — could **not** be verified and is labelled
`[UNVERIFIED]` in the menu itself rather than quietly presented as fact. A test
enforces that labelling.

You can also type a yaw directly, and add an angular offset for "the cue is 30°
left of centre in my screenshot".

**Stage 2 — the sequence.** Samples biomes along a *cone*, not a single ray,
because the heading is uncertain. Sweeps a range of angles and start offsets
and scores how well the observed transition order matches, plus the distances
where you gave them.

Scoring is a Needleman-Wunsch style alignment rather than a positional
comparison, because both sides are noisy in different ways: the generator emits
slivers you never noticed, and people merge or forget legs. Biome identity is
weighted well above remembered distance — three biomes in the right order with
badly wrong spacing beats a sequence with the wrong biome in it.

**Limits.** This is the fuzziest mode in the tool and says so. Biome sequences
repeat, spacing estimates are rough, and the cue itself is approximate. Output
is always a ranked list with a confidence caveat, never a single answer.

**Feeds:** a search box around the best candidate.
