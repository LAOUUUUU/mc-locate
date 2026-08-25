package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import net.minecraft.client.Minecraft;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructureStart;

/**
 * Reads generated structures and their exact origin chunks.
 *
 * <p>Structure cracking needs the chunk cubiomes reports as a structure's
 * origin, exactly — the lift has zero tolerance, so an approximate position
 * silently kills the seed. Rather than guess each structure's block-to-origin
 * offset from what the client renders, this asks the integrated server, which
 * stores the authoritative {@link StructureStart} for every structure. That is
 * only available in singleplayer, so this reader is:
 *
 * <ul>
 *   <li>a way to validate the whole cracker end to end — collect structures in a
 *       known world, crack, and confirm the recovered seed matches; and</li>
 *   <li>the correctness oracle for a future client-only detector: whatever that
 *       computes from blocks must equal the origin reported here.</li>
 * </ul>
 *
 * <p>On a server the client never receives structure starts, so there is nothing
 * exact to read here — that case is deliberately not faked.
 */
public final class StructureReader {
    private StructureReader() {
    }

    /** One newly-found structure at its exact origin. */
    public record Found(String type, int x, int z) {
    }

    /** Outcome of a scan; {@code serverUnsupported} means "not singleplayer". */
    public record Result(java.util.List<Found> found, boolean serverUnsupported) {
        public int added() {
            return found.size();
        }
    }

    private static final int CHUNK_RADIUS = 8;

    public static Result scan(Session session) {
        Minecraft client = Minecraft.getInstance();
        if (!client.hasSingleplayerServer()) {
            return new Result(java.util.List.of(), true);
        }
        var server = client.getSingleplayerServer();
        var player = client.player;
        if (server == null || player == null || client.level == null) {
            return new Result(java.util.List.of(), false);
        }
        var serverLevel = server.getLevel(client.level.dimension());
        if (serverLevel == null) {
            return new Result(java.util.List.of(), false);
        }
        java.util.List<Found> found = new ArrayList<>();
        // The registry accessor was renamed in the 1.21.2 registry refactor:
        // registryOrThrow before, lookupOrThrow after (both return the Registry).
        //? if <1.21.2 {
        /*var registry = server.registryAccess().registryOrThrow(Registries.STRUCTURE);
        *///?} else
        var registry = server.registryAccess().lookupOrThrow(Registries.STRUCTURE);

        // Derive the player's chunk from block coords; ChunkPos's x/z accessors
        // are public fields on some versions and record methods on others, so
        // avoid them and use getMinBlockX/Z (stable everywhere) throughout.
        int centerX = Math.floorDiv((int) Math.floor(player.getX()), 16);
        int centerZ = Math.floorDiv((int) Math.floor(player.getZ()), 16);
        for (int cx = centerX - CHUNK_RADIUS; cx <= centerX + CHUNK_RADIUS; cx++) {
            for (int cz = centerZ - CHUNK_RADIUS; cz <= centerZ + CHUNK_RADIUS; cz++) {
                LevelChunk chunk;
                try {
                    // Non-generating: null when the chunk is not already loaded.
                    chunk = serverLevel.getChunkSource().getChunkNow(cx, cz);
                } catch (Throwable t) {
                    continue;
                }
                if (chunk == null) {
                    continue;
                }
                for (Map.Entry<Structure, StructureStart> e : chunk.getAllStarts().entrySet()) {
                    StructureStart start = e.getValue();
                    if (start == null || !start.isValid()) {
                        continue;
                    }
                    ChunkPos origin = start.getChunkPos();
                    // A start is referenced from every chunk it spans; record it
                    // only from its own origin chunk, so it is counted once and
                    // the position is the exact one the lift expects.
                    if (origin.getMinBlockX() != cx * 16 || origin.getMinBlockZ() != cz * 16) {
                        continue;
                    }
                    var id = registry.getKey(e.getKey());
                    if (id == null) {
                        continue;
                    }
                    String name = normalise(id.getPath());
                    int ox = origin.getMinBlockX();
                    int oz = origin.getMinBlockZ();
                    if (session.addStructure(name, ox, oz)) {
                        found.add(new Found(name, ox, oz));
                    }
                }
            }
        }
        return new Result(found, false);
    }

    /**
     * Diagnostic (singleplayer): for each nearby structure the integrated server
     * knows, reports its exact origin chunk and how the origin relates to the
     * structure's block bounding box.
     *
     * <p>This is the calibration a server-side block detector needs: on a server
     * the client can see blocks but not the {@link StructureStart}, so to compute
     * the origin from blocks it must know the origin-to-bounding-box relationship.
     * Establishing it here from the game's own data — rather than guessing — is
     * the honest way to build that detector. The key line each row reports is
     * whether {@code floor(boundingBox.min / 16) == originChunk}, and the exact
     * block offset when it does not.
     */
    public static List<String> verify() {
        List<String> out = new ArrayList<>();
        Minecraft client = Minecraft.getInstance();
        if (!client.hasSingleplayerServer()) {
            out.add("§cSingleplayer only — a server does not expose structure starts.");
            return out;
        }
        var server = client.getSingleplayerServer();
        var player = client.player;
        if (server == null || player == null || client.level == null) {
            return out;
        }
        var serverLevel = server.getLevel(client.level.dimension());
        if (serverLevel == null) {
            return out;
        }
        //? if <1.21.2 {
        /*var registry = server.registryAccess().registryOrThrow(Registries.STRUCTURE);
        *///?} else
        var registry = server.registryAccess().lookupOrThrow(Registries.STRUCTURE);

        int centerX = Math.floorDiv((int) Math.floor(player.getX()), 16);
        int centerZ = Math.floorDiv((int) Math.floor(player.getZ()), 16);
        for (int cx = centerX - CHUNK_RADIUS; cx <= centerX + CHUNK_RADIUS; cx++) {
            for (int cz = centerZ - CHUNK_RADIUS; cz <= centerZ + CHUNK_RADIUS; cz++) {
                LevelChunk chunk;
                try {
                    chunk = serverLevel.getChunkSource().getChunkNow(cx, cz);
                } catch (Throwable t) {
                    continue;
                }
                if (chunk == null) {
                    continue;
                }
                for (Map.Entry<Structure, StructureStart> e : chunk.getAllStarts().entrySet()) {
                    StructureStart start = e.getValue();
                    if (start == null || !start.isValid()) {
                        continue;
                    }
                    ChunkPos origin = start.getChunkPos();
                    if (origin.getMinBlockX() != cx * 16 || origin.getMinBlockZ() != cz * 16) {
                        continue;
                    }
                    var id = registry.getKey(e.getKey());
                    if (id == null) {
                        continue;
                    }
                    BoundingBox bb = start.getBoundingBox();
                    int ocx = origin.getMinBlockX() / 16;
                    int ocz = origin.getMinBlockZ() / 16;
                    boolean match = Math.floorDiv(bb.minX(), 16) == ocx
                            && Math.floorDiv(bb.minZ(), 16) == ocz;
                    out.add(String.format(java.util.Locale.ROOT,
                            "%s§r origin_chunk=(%d,%d) bboxMin=(%d,%d) blockOffset=(%d,%d) %s",
                            id.getPath(), ocx, ocz, bb.minX(), bb.minZ(),
                            bb.minX() - origin.getMinBlockX(), bb.minZ() - origin.getMinBlockZ(),
                            match ? "§abboxMinChunk==origin" : "§ebboxMinChunk!=origin"));
                }
            }
        }
        return out;
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
