<!--
Store copy for Modrinth / CurseForge. The SUMMARY is the one-line card text
(Modrinth "Summary", CurseForge summary). Everything under DESCRIPTION is the
project body — both sites render Markdown.
-->

## SUMMARY (one line)

Export Nether bedrock, End pillars, and eye-of-ender bearings for seed and coordinate recovery with the mc-locate CLI.

## Alt summaries (pick whichever fits the field length)

- Client-side data exporter for mc-locate — turn what your game already shows you into a seed crack.
- Collect the observations that recover a world's seed and coordinates, straight from your own client.

---

## DESCRIPTION

# mc-locate exporter

A lightweight, **client-side** companion mod for the
[mc-locate](https://github.com/LAOUUUUU/mc-locate) command-line tool. It collects
the in-game observations mc-locate uses to recover a world's **seed** and
**coordinates** — Nether bedrock patterns, End pillar heights, and eye-of-ender
bearings — and writes them to a JSON file the CLI reads.

It never sends anything to the server and never touches other players. Every
observation is a block or entity your client already received because you were
standing near it — the mod just does the bookkeeping.

## What it does

- **Nether bedrock** — samples the floor (y=4) and roof (y=123), where bedrock
  is rarest and therefore most informative, either on command or passively as
  you fly around.
- **End pillars** — reads the ten obsidian pillar heights, which pin down the
  structure seed in one visit.
- **Eye of ender** — records the exact bearing of a thrown eye, taken from the
  entity itself (far more precise than reading the F3 angle), for stronghold
  triangulation.
- **Crash-safe sessions** — everything is saved continuously and reloaded on the
  next launch, so a crash or a forgotten export never loses your work.
- **Live handoff** — mc-locate can watch the export folder and import as you
  play, so the candidate count falls in real time while you're still in the game.

## Commands

All commands are client-side (`/mclocate …`) and work in single-player or on a
server:

| Command | What it does |
|---|---|
| `/mclocate bedrock [radius]` | Sample Nether bedrock around you |
| `/mclocate pillars` | Read the End pillar heights |
| `/mclocate auto on` / `off` | Passive collection as you play |
| `/mclocate status` | What you've collected and how close you are |
| `/mclocate seed` | Record the seed (single-player only) |
| `/mclocate shot` | Screenshot for the CLI's F3 reader |
| `/mclocate hud on` / `off` | Live status on the action bar |
| `/mclocate export` | Write the observation file |
| `/mclocate here` · `mark` · `slime` · `config` · `clear` | Coordinates, structures, slime chunks, settings |

## Multiplayer

It works on servers — commands are handled client-side and never reach the
server, and no gameplay is changed. The one single-player-only command is
`/mclocate seed`, because a server's seed cannot be read; that is exactly what
you crack instead.

**Please only use it where the server allows seed cracking.** Many servers
forbid it, and hardened servers randomize the data it relies on.

## Requirements

- Fabric Loader
- Fabric API
- A matching Minecraft version (each download targets a specific version — see
  the file list)

## Supported versions

One build per Minecraft version: **1.21 through 1.21.11, 26.1.x, and 26.2**.
Grab the file that matches your game.

## Getting the seed

This mod only **collects** — the actual seed/coordinate recovery is done by the
free, open-source [mc-locate CLI](https://github.com/LAOUUUUU/mc-locate)
(Windows, macOS, Linux). Export in game, then import the file in the CLI.
