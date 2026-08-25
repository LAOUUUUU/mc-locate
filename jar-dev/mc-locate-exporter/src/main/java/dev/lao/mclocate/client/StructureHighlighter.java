package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.List;

import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.particles.ParticleTypes;

/**
 * Draws a particle wireframe around found structures, so you can see one rather
 * than trust a chat line.
 *
 * <p>A real 3D line renderer is exactly what 26.x's render rewrite makes fragile
 * across versions; particles are not — {@code addParticle} and the particle
 * types are stable everywhere — so this traces the twelve edges of each box with
 * END_ROD particles. It only outlines boxes near the player and re-emits on a
 * throttle, since particles fade.
 */
public final class StructureHighlighter {
    private static final int MAX_BOXES = 64;
    private static final double RANGE = 96.0;
    private static final int STEP = 2;

    /** {minX,minY,minZ,maxX,maxY,maxZ} in block coords (max is inclusive). */
    private final List<int[]> boxes = new ArrayList<>();

    public void add(StructureReader.Found f) {
        for (int[] b : boxes) {
            if (b[0] == f.minX() && b[2] == f.minZ()) {
                return; // already tracked
            }
        }
        if (boxes.size() >= MAX_BOXES) {
            boxes.remove(0);
        }
        boxes.add(new int[] { f.minX(), f.minY(), f.minZ(), f.maxX(), f.maxY(), f.maxZ() });
    }

    public void clear() {
        boxes.clear();
    }

    public void render(ClientLevel level, double px, double py, double pz) {
        for (int[] b : boxes) {
            double cx = (b[0] + b[3]) / 2.0;
            double cz = (b[2] + b[5]) / 2.0;
            if (Math.abs(cx - px) > RANGE || Math.abs(cz - pz) > RANGE) {
                continue;
            }
            drawBox(level, b[0], b[1], b[2], b[3] + 1, b[4] + 1, b[5] + 1);
        }
    }

    private void drawBox(ClientLevel level, double x0, double y0, double z0,
            double x1, double y1, double z1) {
        // Four vertical edges.
        edge(level, x0, y0, z0, x0, y1, z0);
        edge(level, x1, y0, z0, x1, y1, z0);
        edge(level, x0, y0, z1, x0, y1, z1);
        edge(level, x1, y0, z1, x1, y1, z1);
        // Bottom and top rectangles.
        for (double y : new double[] { y0, y1 }) {
            edge(level, x0, y, z0, x1, y, z0);
            edge(level, x0, y, z1, x1, y, z1);
            edge(level, x0, y, z0, x0, y, z1);
            edge(level, x1, y, z0, x1, y, z1);
        }
    }

    private void edge(ClientLevel level, double ax, double ay, double az,
            double bx, double by, double bz) {
        double dist = Math.sqrt((bx - ax) * (bx - ax) + (by - ay) * (by - ay) + (bz - az) * (bz - az));
        int steps = Math.max(1, (int) (dist / STEP));
        for (int i = 0; i <= steps; i++) {
            double t = (double) i / steps;
            level.addParticle(ParticleTypes.END_ROD,
                    ax + (bx - ax) * t, ay + (by - ay) * t, az + (bz - az) * t, 0, 0, 0);
        }
    }
}
