# Mode 4 — Slime Chunk Seed Cracker

**Needs:** chunk coordinates you have confirmed *are* slime chunks, and
optionally ones you are confident are *not*.

**Where it comes from:** F3 + G draws chunk borders; F3's `Chunk:` line gives
the chunk you are standing in. Slimes spawn below y=40 in slime chunks
regardless of light level.

**How it works.** Eligibility is a pure function of seed and chunk:

    Random(seed + cx*cx*0x4c1906 + cx*0x5ac0db
                + cz*cz*0x4307a7 + cz*0x5f24f
           ^ 0x3ad8025f).nextInt(10) == 0

Two details matter and are easy to get wrong. The `^ 0x3ad8025f` applies to the
*whole* sum, because `^` binds looser than `+` in Java. And the casts are not
uniform: the x terms wrap in 32-bit before widening, while `cz*cz` widens first.
Both are pinned by tests against 108 vectors from cubiomes' own C.

**Positives are worth ten times a negative.** A confirmed slime chunk rejects
90% of seeds; a confirmed non-slime rejects 10%. The cracker tests positives
first for that reason. Fifteen-odd positives is the usual rule of thumb.

**Limits.** The full 48-bit space is roughly a day of CPU here. The mode
measures the rate and quotes an ETA before starting, and offers to filter an
existing candidate list instead, which is enormously cheaper. Mode 9's pillar
shortcut is the better route if you can get to the End.

**Feeds:** structure seed candidates.
