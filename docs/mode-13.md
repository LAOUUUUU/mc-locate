# Mode 13 — Session & Observations

Persistence, and the intake for anything that is not you typing.

## Save and load

A session used to live only in memory, so quitting discarded every coordinate
you had entered. Forty bedrock blocks is real effort; this saves it as plain
JSON — readable, diffable, and small.

**Loading merges rather than replaces.** Importing the same file twice adds
nothing and reports the duplicates, and an import will not overwrite a seed or
version you already have unless you say so. Both matter once a mod is writing
these files: re-importing a dump must not silently double every constraint.

## The file format

    {
      "format": "mc-locate-observations",
      "version": 1,
      "minecraft_version": "1.21.3",
      "bedrock":    [{"x": 11, "y": 4, "z": -97, "is_bedrock": true}],
      "slime":      [{"chunk_x": 5, "chunk_z": -3, "is_slime": true}],
      "structures": [{"type": "desert_pyramid", "x": 1384, "z": -2952}],
      "pillar_heights": [76, null, 82, null, null, 94, null, null, null, 103]
    }

Every field is optional, so the minimum a producer must write is the format tag,
the version, and whatever it actually observed. Unknown fields from a newer
producer are ignored rather than fatal, which matters because a mod and this
tool will be versioned separately and will drift.

Structure names accept both cubiomes' canonical form (`desert_pyramid`) and the
menu labels (`Desert pyramid`), in any case, with spaces or underscores.

**This is the contract for external producers.** A Fabric mod dumping bedrock as
you fly the Nether, a script, a hand-written file — anything that writes this
JSON feeds mc-locate, and the producer never needs to know any of the maths.

## Watching the exporter mod's folder (live)

The exporter mod writes its observations to `<gameDir>/mc-locate`. Point this
watcher at that folder and every file the mod writes is imported the moment it
appears — no copy-paste. The mod keeps a single rolling `session-current.json`
that grows as you play, and the watcher re-reads it whenever it changes, so the
CLI stays in sync with the game in real time: fly the Nether, watch the
candidate count fall here. Existing files in the folder are imported once on
entry, since a file already sitting there is data you want, not history to skip.
Every import is deduplicated, so re-reading the rolling file costs nothing.

## Watching a screenshots folder

Minecraft cannot be made to press F2 for you, and driving its window from
outside would be fragile. The other half of that loop works well though: watch
the screenshots folder, and the moment the game writes a new PNG, read the F3
overlay out of it. The advisor tells you what to look at, you press F2, and the
observation arrives without typing.

Only files that appear *after* you start watching are considered, so an existing
pile of screenshots is not re-read every session. Needs the `ocr` feature to
actually read them; without it new shots are reported but not parsed.
