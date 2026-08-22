# Mode 12 — Observation Advisor

The other modes ask "here is what I saw, what does it mean?". This one runs the
question backwards.

## What should I observe next?

**With a candidate list**, the advice is exact rather than modelled. Any
proposed observation partitions your candidates into the ones that would say yes
and the ones that would say no. A 50/50 split is worth a full bit and halves
the list; a 99/1 split is worth almost nothing. So the advisor evaluates real
candidate seeds against real nearby positions and ranks by the actual split —
the classic decision-tree criterion, with no modelling assumptions at all.

    1. Look at nether floor block (-118, 4, 47)
         0.998 bits  ->  about 1041 candidates left (49.9% eliminated)
    2. Check whether chunk (12, -41) is a slime chunk
         0.471 bits  ->  about 1614 candidates left (22.3% eliminated)

**Without one** there is nothing to partition, so it ranks by a-priori
information content instead:

| Observation | Bits |
|---|---|
| End pillar heights | 16.00 |
| A structure's exact origin chunk | ~9–12 |
| A nether bedrock block at y=4 or y=123 | 0.72 |
| A confirmed slime chunk | 0.47 |
| A confirmed *non*-slime chunk | 0.15 |

A slime chunk is a 1-in-10 event: "yes" is worth `log2(10)` = 3.32 bits but
arrives rarely, "no" is worth 0.15 and arrives often, averaging 0.47. That is
why fifteen-odd are needed and why negatives, though real, are weak.

**Effort is tracked separately** and derived from each structure's region size,
so a woodland mansion is not advertised as a short trip just because it carries
a lot of information — an 80-chunk grid can mean thousands of blocks of travel.

## Explain a candidate

The cracking hot path short-circuits at the first failed constraint, which is
what makes it fast and useless for diagnosis. This evaluates every constraint
and names the failures.

    Seed 12840895245824: 146 of 147 constraints matched.
      ✓ chunk (12, -41) is a slime chunk
      ✗ (11, 4, -97) is bedrock

A near miss is called out explicitly, because 146/147 almost always means one
mis-typed coordinate rather than a wrong seed. Mode 9 offers the same breakdown
inline on its results.
