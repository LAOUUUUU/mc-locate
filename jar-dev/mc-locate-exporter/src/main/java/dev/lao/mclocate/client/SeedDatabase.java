package dev.lao.mclocate.client;

import java.io.IOException;
import java.io.Reader;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonParseException;

import net.minecraft.world.level.biome.BiomeManager;

/**
 * A local list of known seeds, so joining a world on one is flagged.
 *
 * <p>It matches two ways. In singleplayer the real seed is known, so it is a
 * direct comparison. On a server the seed cannot be read, but the client's
 * biome-zoom seed can (see {@link AutoCollector}); that value is
 * {@code obfuscateSeed(obfuscateSeed(worldSeed))}, so computing the same double
 * hash for each known seed and comparing identifies the world's seed <em>on a
 * server</em>, without ever reading it.
 */
public final class SeedDatabase {
    public record Entry(long seed, String name) {
    }

    public static final String FILE = "known-seeds.json";

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    private final Path file;
    private final List<Entry> entries;

    private SeedDatabase(Path file, List<Entry> entries) {
        this.file = file;
        this.entries = entries;
    }

    /** Loads the database, writing a commented template on first run. */
    public static SeedDatabase load(Path directory) {
        Path file = directory.resolve(FILE);
        List<Entry> entries = new ArrayList<>();
        if (Files.isRegularFile(file)) {
            try (Reader r = Files.newBufferedReader(file)) {
                Entry[] read = GSON.fromJson(r, Entry[].class);
                if (read != null) {
                    for (Entry e : read) {
                        if (e != null && e.name() != null) {
                            entries.add(e);
                        }
                    }
                }
            } catch (IOException | JsonParseException e) {
                // A broken file should not stop the game; start empty.
            }
        } else {
            writeTemplate(file);
        }
        return new SeedDatabase(file, entries);
    }

    /** The known seed matching the world, or null. */
    public Entry identify(boolean singleplayer, long realSeed, Long biomeZoomSeed) {
        for (Entry e : entries) {
            if (singleplayer) {
                if (e.seed() == realSeed) {
                    return e;
                }
            } else if (biomeZoomSeed != null
                    && BiomeManager.obfuscateSeed(BiomeManager.obfuscateSeed(e.seed())) == biomeZoomSeed) {
                return e;
            }
        }
        return null;
    }

    /** Adds a seed and persists it. Returns false if that seed is already known. */
    public boolean add(long seed, String name) {
        for (Entry e : entries) {
            if (e.seed() == seed) {
                return false;
            }
        }
        entries.add(new Entry(seed, name));
        save();
        return true;
    }

    public List<Entry> entries() {
        return List.copyOf(entries);
    }

    private void save() {
        try {
            Files.createDirectories(file.getParent());
            Files.writeString(file, GSON.toJson(entries.toArray(new Entry[0])) + "\n");
        } catch (IOException e) {
            // Best-effort; the in-memory list still works this session.
        }
    }

    private static void writeTemplate(Path file) {
        try {
            Files.createDirectories(file.getParent());
            // A valid, empty-but-illustrative array. The player adds entries with
            // /mclocate known add, or by editing this file.
            Files.writeString(file,
                    "[\n  { \"seed\": 3257840388504953787, \"name\": \"example — replace me\" }\n]\n");
        } catch (IOException e) {
            // ignore
        }
    }
}
