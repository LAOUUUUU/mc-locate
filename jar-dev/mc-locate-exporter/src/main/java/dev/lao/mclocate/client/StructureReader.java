package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.registries.Registries;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;

/**
 * Reads generated structures and their exact origin chunks (singleplayer).
 *
 * <p>The lift needs the chunk cubiomes reports as a structure's origin, exactly;
 * an approximate position silently kills the seed. Rather than guess each
 * structure's block-to-origin offset from what the client renders, this asks the
 * integrated server, which stores the authoritative {@link StructureStart}.
 *
 * <p>Crucially, the integrated server's chunk cache only answers on the server
 * thread — {@code getChunkNow} returns null off it — so the reads run inside
 * {@code server.execute(..)} and the results are handed back to the client thread
 * for display. That threading is why an earlier synchronous version found
 * nothing. On a real server the client never receives structure starts, so this
 * is singleplayer-only and not faked there.
 */
public final class StructureReader {
    private StructureReader() {
    }

    /** One newly-found structure: its exact origin and its block bounding box. */
    public record Found(String type, int x, int z,
            int minX, int minY, int minZ, int maxX, int maxY, int maxZ) {
    }

    /** Outcome of a scan; {@code serverUnsupported} means "not singleplayer". */
    public record Result(List<Found> found, boolean serverUnsupported) {
        public int added() {
            return found.size();
        }
    }

    private static final int CHUNK_RADIUS = 8;

    @FunctionalInterface
    private interface Sink {
        void accept(String path, int originX, int originZ, BoundingBox box);
    }

    /**
     * Records nearby structures at their exact origins, then delivers the newly
     * added ones to {@code onResult} on the client thread.
     */
    public static void scan(Session session, Consumer<Result> onResult) {
        onServerThread(
                (server, level, centerX, centerZ) -> {
                    List<Found> found = new ArrayList<>();
                    iterate(server, level, centerX, centerZ, (path, ox, oz, box) -> {
                        String name = normalise(path);
                        if (session.addStructure(name, ox, oz)) {
                            found.add(new Found(name, ox, oz,
                                    box.minX(), box.minY(), box.minZ(),
                                    box.maxX(), box.maxY(), box.maxZ()));
                        }
                    });
                    return new Result(found, false);
                },
                new Result(List.of(), true),
                new Result(List.of(), false),
                onResult);
    }

    /** Diagnostic: the origin/bounding-box relationship for nearby structures. */
    public static void verify(Consumer<List<String>> onResult) {
        onServerThread(
                (server, level, centerX, centerZ) -> {
                    List<String> out = new ArrayList<>();
                    iterate(server, level, centerX, centerZ, (path, ox, oz, box) -> {
                        int ocx = ox / 16;
                        int ocz = oz / 16;
                        boolean match = Math.floorDiv(box.minX(), 16) == ocx
                                && Math.floorDiv(box.minZ(), 16) == ocz;
                        out.add(String.format(java.util.Locale.ROOT,
                                "%s§r origin_chunk=(%d,%d) bboxMin=(%d,%d) blockOffset=(%d,%d) %s",
                                path, ocx, ocz, box.minX(), box.minZ(),
                                box.minX() - ox, box.minZ() - oz,
                                match ? "§abboxMinChunk==origin" : "§ebboxMinChunk!=origin"));
                    });
                    return out;
                },
                List.of("§cSingleplayer only — a server does not expose structure starts."),
                List.<String>of(),
                onResult);
    }

    /** A unit of work run on the server thread, given the level and player chunk. */
    @FunctionalInterface
    private interface ServerWork<T> {
        T run(MinecraftServer server, ServerLevel level, int centerX, int centerZ);
    }

    /**
     * Runs {@code work} on the integrated-server thread and returns the result to
     * {@code onResult} on the client thread; delivers {@code notSingleplayer} or
     * {@code unavailable} instead when there is no integrated server / no level.
     */
    private static <T> void onServerThread(ServerWork<T> work, T notSingleplayer, T unavailable,
            Consumer<T> onResult) {
        Minecraft client = Minecraft.getInstance();
        if (!client.hasSingleplayerServer()) {
            onResult.accept(notSingleplayer);
            return;
        }
        var server = client.getSingleplayerServer();
        var player = client.player;
        if (server == null || player == null || client.level == null) {
            onResult.accept(unavailable);
            return;
        }
        var dimension = client.level.dimension();
        int centerX = Math.floorDiv((int) Math.floor(player.getX()), 16);
        int centerZ = Math.floorDiv((int) Math.floor(player.getZ()), 16);

        server.execute(() -> {
            T result;
            try {
                ServerLevel level = server.getLevel(dimension);
                result = level == null ? unavailable : work.run(server, level, centerX, centerZ);
            } catch (Throwable t) {
                result = unavailable;
            }
            T delivered = result;
            client.execute(() -> onResult.accept(delivered));
        });
    }

    /** Iterates valid structures near the centre. MUST run on the server thread. */
    private static void iterate(MinecraftServer server, ServerLevel level, int centerX, int centerZ,
            Sink sink) {
        // The registry accessor was renamed in the 1.21.2 registry refactor.
        //? if <1.21.2 {
        /*var registry = server.registryAccess().registryOrThrow(Registries.STRUCTURE);
        *///?} else
        var registry = server.registryAccess().lookupOrThrow(Registries.STRUCTURE);

        for (int cx = centerX - CHUNK_RADIUS; cx <= centerX + CHUNK_RADIUS; cx++) {
            for (int cz = centerZ - CHUNK_RADIUS; cz <= centerZ + CHUNK_RADIUS; cz++) {
                LevelChunk chunk = level.getChunkSource().getChunkNow(cx, cz);
                if (chunk == null) {
                    continue;
                }
                for (Map.Entry<Structure, StructureStart> e : chunk.getAllStarts().entrySet()) {
                    StructureStart start = e.getValue();
                    if (start == null || !start.isValid()) {
                        continue;
                    }
                    ChunkPos origin = start.getChunkPos();
                    // A start is referenced from every chunk it spans; take it
                    // only from its own origin chunk, so each is seen once and the
                    // position is the exact one the lift expects.
                    if (origin.getMinBlockX() != cx * 16 || origin.getMinBlockZ() != cz * 16) {
                        continue;
                    }
                    var id = registry.getKey(e.getKey());
                    if (id == null) {
                        continue;
                    }
                    sink.accept(id.getPath(), origin.getMinBlockX(), origin.getMinBlockZ(),
                            start.getBoundingBox());
                }
            }
        }
    }

    /**
     * Collapses per-biome structure ids to the family name the CLI cracks:
     * villages and ruined portals are registered as {@code village_plains},
     * {@code ruined_portal_desert}, etc., but crack the same way.
     */
    private static String normalise(String path) {
        if (path.startsWith("village")) {
            return "village";
        }
        if (path.startsWith("ruined_portal")) {
            return "ruined_portal";
        }
        return path;
    }
}
