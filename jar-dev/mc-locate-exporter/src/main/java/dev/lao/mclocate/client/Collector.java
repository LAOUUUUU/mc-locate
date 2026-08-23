package dev.lao.mclocate.client;

import net.minecraft.core.BlockPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;

/**
 * Reads observations out of chunks the client already has loaded.
 *
 * <p>Everything here is deliberately read-only and client-side: no packets, no
 * server, no reliance on anything the vanilla client is not already told. That
 * is what lets it work on any world you can load, and it is why the collected
 * data is exactly what a player could write down by looking.
 */
public final class Collector {
	private Collector() {
	}

	/**
	 * The ten End pillar positions, in mc-locate's generation order.
	 *
	 * <p>These are seed-independent: the game places the pillars on a radius-42
	 * circle at fixed coordinates and only the *heights* vary. The order must
	 * match mc-locate's {@code PILLAR_POSITIONS}, because the heights are
	 * reported positionally.
	 */
	private static final int[][] PILLAR_POSITIONS = {
			{42, 0}, {33, 24}, {12, 39}, {-13, 39}, {-34, 24},
			{-42, -1}, {-34, -25}, {-13, -40}, {12, -40}, {33, -25},
	};

	/** Pillar tops run 76, 79 … 103. Anything outside that is not a pillar. */
	private static final int PILLAR_MIN_Y = 76;
	private static final int PILLAR_MAX_Y = 103;

	/**
	 * The nether layers worth recording.
	 *
	 * <p>y=4 on the floor and y=123 on the roof are where bedrock is rarest —
	 * 20% each — so a block there carries far more information than one at y=0
	 * or y=127, which are always bedrock and say nothing at all.
	 */
	public static final int FLOOR_Y = 4;
	public static final int ROOF_Y = 123;

	/**
	 * Records the bedrock/not-bedrock state of every column in a square around
	 * the player, at one y level.
	 *
	 * <p>Only positions in loaded chunks are recorded. An unloaded chunk
	 * returns air from {@code getBlockState}, which would be recorded as a
	 * confident "not bedrock" and is exactly the kind of false observation that
	 * eliminates the true seed.
	 */
	public static int collectBedrock(Level level, int centreX, int centreZ, int y, int radius,
			Observations out) {
		int recorded = 0;
		BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();

		for (int x = centreX - radius; x <= centreX + radius; x++) {
			for (int z = centreZ - radius; z <= centreZ + radius; z++) {
				pos.set(x, y, z);
				if (!level.isLoaded(pos)) {
					continue;
				}
				BlockState state = level.getBlockState(pos);
				out.addBedrock(x, y, z, state.getBlock() == Blocks.BEDROCK);
				recorded++;
			}
		}
		return recorded;
	}

	/**
	 * Reads the ten pillar heights by scanning each known position downward for
	 * the topmost obsidian.
	 *
	 * <p>Returns nulls for pillars whose chunk is not loaded, rather than
	 * guessing: mc-locate treats a null as "not measured" and still narrows the
	 * pillar seed with the ones it does have.
	 */
	public static Integer[] collectPillarHeights(Level level) {
		Integer[] heights = new Integer[PILLAR_POSITIONS.length];
		BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();

		for (int i = 0; i < PILLAR_POSITIONS.length; i++) {
			int x = PILLAR_POSITIONS[i][0];
			int z = PILLAR_POSITIONS[i][1];

			pos.set(x, PILLAR_MAX_Y, z);
			if (!level.isLoaded(pos)) {
				continue;
			}
			for (int y = PILLAR_MAX_Y; y >= PILLAR_MIN_Y; y--) {
				pos.set(x, y, z);
				if (level.getBlockState(pos).getBlock() == Blocks.OBSIDIAN) {
					heights[i] = y;
					break;
				}
			}
		}
		return heights;
	}

	/** How many of the ten pillars were actually measured. */
	public static int measuredPillars(Integer[] heights) {
		int n = 0;
		for (Integer h : heights) {
			if (h != null) {
				n++;
			}
		}
		return n;
	}

	/**
	 * Pillar heights are a permutation of ten distinct values, so a repeat means
	 * something was misread — a nearby obsidian structure, most likely.
	 */
	public static boolean heightsLookValid(Integer[] heights) {
		boolean[] seen = new boolean[(PILLAR_MAX_Y - PILLAR_MIN_Y) / 3 + 1];
		for (Integer h : heights) {
			if (h == null) {
				continue;
			}
			if (h < PILLAR_MIN_Y || h > PILLAR_MAX_Y || (h - PILLAR_MIN_Y) % 3 != 0) {
				return false;
			}
			int slot = (h - PILLAR_MIN_Y) / 3;
			if (seen[slot]) {
				return false;
			}
			seen[slot] = true;
		}
		return true;
	}
}
