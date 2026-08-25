# mc-locate exporter

A tiny **client-side Fabric mod** that collects the in-game observations
[mc-locate](https://github.com/LAOUUUUU/mc-locate) uses to recover a world's
seed and coordinates — Nether bedrock, End pillar heights, and eye-of-ender
bearings — and writes them to a JSON file the CLI can import.

It only reads blocks and entities the game has **already sent to your own
client** because you are standing near them. It sends nothing, talks to no
server, and never touches other players. See [Scope & limits](#scope--limits).

---

## Requirements

- **Fabric Loader** ≥ 0.16
- **Fabric API** (any build matching your Minecraft version)
- One of the supported Minecraft versions below

## Which jar do I download?

One jar is built per Minecraft version. Grab the one matching your game.

| Your Minecraft | Download this jar | Also runs on |
|---|---|---|
| 1.21 | `…+1.21.jar` | — |
| 1.21.1 | `…+1.21.1.jar` | 1.21 |
| 1.21.2 | `…+1.21.2.jar` | — |
| 1.21.3 | `…+1.21.3.jar` | — |
| 1.21.4 | `…+1.21.4.jar` | — |
| 1.21.5 | `…+1.21.5.jar` | — |
| 1.21.6 | `…+1.21.6.jar` | — |
| 1.21.7 | `…+1.21.7.jar` | — |
| 1.21.8 | `…+1.21.8.jar` | 1.21.7 |
| 1.21.9 | `…+1.21.9.jar` | — |
| 1.21.10 | `…+1.21.10.jar` | — |
| 1.21.11 | `…+1.21.11.jar` | — |
| 26.1 / 26.1.1 / 26.1.2 | `…+26.1.2.jar` | all of 26.1.x |
| 26.2 | `…+26.2.jar` | — |

> The full name is `mc-locate-exporter-<mod version>+<mc version>.jar`.
> Loader refuses to load a jar built for the wrong Minecraft version, so you
> cannot pick the wrong one by accident.

## Install

1. Install [Fabric Loader](https://fabricmc.net/use/installer/) for your
   Minecraft version.
2. Drop **Fabric API** and the matching **mc-locate exporter** jar into your
   `mods/` folder.
3. Launch the game.

---

## Commands

All commands are client-side (`/mclocate …`) and run in single-player or on any
server — they only read what your client can already see.

| Command | What it does |
|---|---|
| `/mclocate bedrock [radius]` | Sample bedrock in a square around you at y=4 and y=123. `radius` defaults to **24**, max **128**. **Nether only.** |
| `/mclocate pillars` | Read the ten End pillar heights. **End only**; refuses to save an arrangement that isn't a valid pillar set. |
| `/mclocate auto on` / `off` | Turn passive collection on/off (see below). |
| `/mclocate status` | Show what the session holds and how many seeds it's expected to leave standing. |
| `/mclocate here` | Print your exact position, yaw, and dimension. |
| `/mclocate mark <type>` | Record a structure at your current position (21 types). |
| `/mclocate seed` | Record the world seed. **Singleplayer only** — the client owns the seed there, so no cracking is needed. |
| `/mclocate structures` | Record nearby structures at their exact origins. **Singleplayer only** — servers don't send structure positions to the client. |
| `/mclocate slime` / `slime not` | Mark the current chunk as a slime chunk / confirmed ordinary. Manual on purpose (see below). |
| `/mclocate config` | Show settings, or `config <key> <value>` to change one in-game. |
| `/mclocate shot` | Take a screenshot (feeds the CLI's F3 screenshot reader). |
| `/mclocate hud on` / `off` | Live status on the action bar. Works on 1.21.x and 26.1.x; 26.2 removed the client action bar in its render rewrite, so it no-ops there. |
| `/mclocate export` | Write everything collected so far to a JSON file. |
| `/mclocate clear` | Empty the current session. |

`mark` accepts: `village`, `desert_pyramid`, `jungle_temple`, `swamp_hut`,
`igloo`, `pillager_outpost`, `ocean_monument`, `woodland_mansion`,
`ruined_portal`, `shipwreck`, `buried_treasure`, `fortress`, `bastion`,
`end_city`.

## Passive mode — the intended workflow

Bedrock is a **filter**, not a search: on its own it can't find a seed, it can
only strike candidates off a list. The list comes from the **End pillars**,
which pin down 2³² structure seeds from a single reading. So the natural loop is:

1. Visit the End once → `/mclocate pillars` (or let auto-collect read them).
2. Turn on passive collection: `/mclocate auto on`.
3. **Just play.** Flying around the Nether accumulates bedrock as chunks load,
   quietly narrowing the candidate set. `/mclocate status` tells you when
   you likely have enough.
4. `/mclocate export`.

Passive collection also records **eye-of-ender bearings** the moment you throw
one — taken from the eye entity itself, which is exact where an F3 reading is
eyeballed. Those feed the CLI's stronghold triangulation.

Passive **bedrock** defaults to *off* (a mod shouldn't start logging your world
uninvited); pillar and eye capture default to *on*. In singleplayer, passive
mode also grabs the **world seed** automatically — the client owns it there.

### Your session survives restarts

Everything collected is written continuously to
`.minecraft/mc-locate/session-current.json` and flushed again when you leave the
world, then reloaded on the next launch. A crash or a forgotten `export` no
longer throws away an afternoon of collecting. `/mclocate clear` starts fresh.

### About slime chunks

Slime chunks are marked **by hand**, and that's deliberate. A single slime
spawn is not proof a chunk is a slime chunk — swamps spawn them too — and a
wrong "yes" eliminates the true seed, the one failure this whole tool is built
to avoid. So confirm it yourself (a slime spawning below y=40, away from a
swamp) and then `/mclocate slime`. Use `/mclocate slime not` for a chunk you're
sure is ordinary.

## Multiplayer

It works on servers, with one exception. Every command is **client-side** —
Fabric handles `/mclocate ...` locally and never sends it to the server — and
every observation is a block or entity the server already sent your client
because you are near it. So collecting bedrock, pillars, and eye bearings, and
exporting them, all work on a normal (even vanilla) server. Nothing is sent to
the server and no gameplay is changed.

The exception is `/mclocate seed`: it reads the integrated server's seed, which
only exists in singleplayer. On a server there is no seed to read — that is the
whole reason to crack one. So the multiplayer flow is: collect here, then crack
in the CLI.

Seed cracking is against the rules on many servers (and hardened servers
randomise the bits it relies on). Only use it where the server allows it.

## Config

Settings live in `.minecraft/mc-locate/config.properties`, written on first run:

| Key | Default | Meaning |
|---|---|---|
| `autoBedrock` | `false` | Record Nether bedrock as chunks load |
| `autoPillars` | `true` | Read pillar heights on entering the End |
| `autoEyes` | `true` | Record thrown-eye bearings |
| `bedrockStride` | `4` | Sample every Nth block in a chunk layer |
| `maxBedrock` | `4096` | Stop accumulating past this many samples |
| `announce` | `true` | Print a chat line when something is collected |
| `hud` | `false` | Live action-bar status (1.21.x / 26.1.x only) |

---

## Getting the data into mc-locate

Export files land in **`.minecraft/mc-locate/observations-<timestamp>.json`**.

In the CLI:

1. Open **mode 13 — Session & Observations**.
2. Choose **"Load / import observations from a file"**.
3. Point it at the exported JSON.

The observation file format is the contract between the two — the CLI doesn't
care whether a mod, a script, or you by hand wrote it.

---

## Scope & limits

- **Client-side only.** Reads your own loaded chunks and your own thrown eyes.
  No packets are forged; no other player is ever referenced.
- **No structures via generation.** It records structures you *stand on* with
  `mark`, but it does not compute structure positions — that needs the CLI's
  worldgen backend, which the mod deliberately doesn't ship.
- **Server rules.** Seed-cracking is against the rules on many servers, and
  hardened servers randomise the very bits this relies on (Paper's
  `feature-seeds` and friends). On those it simply won't converge — by design.
- **Not yet tested in a live game.** Every version *compiles* against the real
  Minecraft API and the JSON round-trips through the CLI's tests, but the
  in-game behaviour (chunk-load timing, the pillar scan) is unverified. If you
  hit something, please open an issue.

---

## Building from source

The mod is a [Stonecutter](https://stonecutter.kikugie.dev/) project — one
codebase, many Minecraft versions.

```bash
# build every supported version at once
# (each jar lands in versions/<mc version>/build/libs/)
./gradlew build

# switch the checked-out source to one version, e.g. to work on it in an IDE
./gradlew "Set active project to 1.21.8"

# put the source back in its canonical form before committing
./gradlew "Reset active project"
```

Requires a JDK able to run Gradle (17+); the per-version compile toolchains
(Java 21 for 1.21.x, Java 25 for 26.x) are provisioned automatically.
