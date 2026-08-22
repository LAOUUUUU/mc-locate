# Mode 7 — Chat/Log Coordinate Scraper (live or file)

**Needs:** a log file, pasted text, or a live game.

**Where it comes from:** `logs/latest.log` in your game directory. The live
watcher auto-detects vanilla, Prism, MultiMC, ATLauncher, GDLauncher and
CurseForge layouts across all three operating systems, and lets you type a path
if yours is elsewhere.

**How it works.** A compiled-once set of regexes covers the coordinate shapes
people actually write: `x: N y: N z: N`, bare triples, `/tp`, bracketed forms,
and two-value `x/z` pairs — negatives and decimals included. Matches outside the
world border, or with an impossible Y, are rejected as noise rather than
reported: those are almost always timestamps or version numbers.

The live watcher seeks to the end of the file and tails it, handling rotation,
and additionally recognises chat lines, join/leave/death messages, and the
`/seed` command's response. If it sees a seed it offers to adopt it for the rest
of the session.

**The limit worth knowing:** `latest.log` contains only chat-visible and console
text. It does **not** contain F3 coordinates — that overlay is never written to
disk. Mode 3 or manual entry are still required for those.

**Feeds:** coordinates, a search box, and sometimes the seed itself.
