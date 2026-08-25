package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * Static registry of structure bounding boxes to outline, shared between the
 * collector (which fills it) and the render mixin (which draws it).
 *
 * <p>It is static because a Mixin cannot reach the mod's instances; both sides
 * touch this instead. Holds only ints — no Minecraft types — so it compiles on
 * every version even though the drawing mixin is 1.21.11-only.
 */
public final class Outlines {
    private Outlines() {
    }

    private static final int MAX = 64;
    private static final List<int[]> BOXES = Collections.synchronizedList(new ArrayList<>());

    /** Whether to draw outlines at all (mirrors the config flag). */
    public static volatile boolean enabled = true;

    /** {minX,minY,minZ,maxX,maxY,maxZ}, max inclusive. */
    public static void add(int minX, int minY, int minZ, int maxX, int maxY, int maxZ) {
        synchronized (BOXES) {
            for (int[] b : BOXES) {
                if (b[0] == minX && b[2] == minZ) {
                    return;
                }
            }
            if (BOXES.size() >= MAX) {
                BOXES.remove(0);
            }
            BOXES.add(new int[] { minX, minY, minZ, maxX, maxY, maxZ });
        }
    }

    public static void clear() {
        synchronized (BOXES) {
            BOXES.clear();
        }
    }

    /** A copy, safe to iterate on the render thread. */
    public static List<int[]> snapshot() {
        synchronized (BOXES) {
            return new ArrayList<>(BOXES);
        }
    }
}
