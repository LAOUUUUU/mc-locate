# Mode 10 — Stronghold Ring Triangulator (Bayesian)

**Needs:** one or more eye-of-ender throws — your X, Z and yaw for each.

**Where it comes from:** throw an eye, then press F3 + C and read the position
and `Facing` angle.

**The ring structure.** All 128 strongholds sit in 8 concentric rings around the
origin, with counts 3, 6, 10, 15, 21, 28, 36, 9 and documented block bands
(1,280–2,816 for the first, and so on out to 24,320). Within a ring they sit at
roughly equal angles from one random start angle, then each is nudged up to 112
blocks to land in a suitable biome — which is why ring maths alone can never be
exact.

**How the inference works.** A throw is a ray: it fixes direction, not distance.
The measured yaw is treated as a noisy observation of the true bearing, and
throws are combined by multiplying likelihoods, which is Bayes with a flat
prior. One throw leaves an arc; two from well-separated positions produce a
sharp peak.

**The nearest-stronghold constraint matters more than it sounds.** An eye always
points at the *nearest* stronghold, so anything that is not nearest to every
throw position could not have produced your readings. Without this, a far
stronghold that happens to sit almost along the same bearing stays a serious
rival — at seed 1234 a ring-5 stronghold lands within 0.6° of both rays and
takes a third of the posterior. Applying the constraint removes it outright.

**Two modes.** If the session knows the seed, it scores cubiomes' *real*
biome-snapped positions — the accurate path. Without a seed it falls back to
the ring prior, which cannot predict biome snapping, so expect to dig within
about a chunk and search.

**Move between throws.** Two throws from nearly the same spot give nearly the
same ray, and the intersection becomes wildly sensitive to a small angle error.
The mode warns if your throws are under 100 blocks apart; a few hundred blocks
perpendicular to the first sight line is what you want.

**Limits.** A simpler model than Ninjabrain-Bot: no calibrated per-player error
distribution, so sigma is a prompt with a conservative default.
