package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.function.Consumer;

import net.minecraft.client.Minecraft;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;
import net.minecraft.world.level.chunk.LevelChunkSection;

/**
 * Recognises structures from the blocks the client can see, so the seed crack
 * works on a server — where the integrated-server {@link StructureReader} is
 * unavailable because a server never sends structure starts.
 *
 * <p>The crack needs a structure's <em>exact</em> origin chunk; one wrong chunk
 * silently eliminates the true seed. So this never guesses:
 * <ul>
 *   <li>It anchors on an unambiguous, deterministic block — for a desert pyramid
 *       the hidden TNT trap, which generates in no other overworld structure —
 *       and takes the cluster's fixed min corner.</li>
 *   <li>The anchor→origin offset is <em>learned</em> in singleplayer against the
 *       real origin (see {@link StructureOffsets}); it is applied on a server
 *       only once confirmed.</li>
 *   <li>Even then it re-checks the computed origin is chunk-aligned; a wrong
 *       anchor fails that and is refused rather than recorded.</li>
 * </ul>
 * Worst case is "detects nothing," never "wrong origin."
 *
 * <p>Only desert pyramids for now — the safest first detector (no rotation, a
 * unique anchor). More can follow the same learn-then-apply pattern.
 */
public final class StructureDetector {
	/** Chunks around the player to sweep for anchors. */
	private static final int CHUNK_RADIUS = 6;
	/** TNT blocks within this Chebyshev distance are treated as one pyramid. */
	private static final int CLUSTER_SPAN = 8;

	private final StructureOffsets offsets;
	/** Announce an "un-calibrated" refusal at most once per world, not per tick. */
	private boolean warnedUncalibrated;

	public StructureDetector(StructureOffsets offsets) {
		this.offsets = offsets;
	}

	public void onWorldLeave() {
		warnedUncalibrated = false;
	}

	/** One recognised anchor: a structure type and a deterministic block. */
	public record Anchor(String type, int x, int y, int z) {
	}

	// ---- server-side passive detection --------------------------------------

	/**
	 * Applies confirmed offsets to anchors the client can see and records the
	 * resulting origins. Runs on a server (and only there — in singleplayer the
	 * exact {@link StructureReader} is authoritative). Returns the count recorded.
	 */
	public void detectOnServer(Minecraft client, Session session, Consumer<StructureReader.Found> onRecorded,
			Consumer<String> say) {
		if (client.level == null || client.player == null) {
			return;
		}
		String version = client.getLaunchedVersion();
		List<Anchor> anchors = findAnchors(client.level,
				client.player.blockPosition(), CHUNK_RADIUS);
		boolean sawUncalibrated = false;
		for (Anchor a : anchors) {
			StructureOffsets.Offset off = offsets.get(a.type(), version);
			if (off == null || !off.confirmed()) {
				sawUncalibrated = true;
				continue;
			}
			int originX = a.x() + off.dx();
			int originZ = a.z() + off.dz();
			// A confirmed offset applied to a good anchor always lands on a chunk
			// corner. If it does not, this anchor is malformed (partial load, an
			// unexpected TNT) — refuse rather than poison the crack.
			if (Math.floorMod(originX, 16) != 0 || Math.floorMod(originZ, 16) != 0) {
				continue;
			}
			if (session.addStructure(a.type(), originX, originZ)) {
				// Visual box bracketing the pyramid, centred on the anchor. Only
				// the recorded origin must be exact; the box is a locator.
				Outlines.add(a.x() - 11, a.y() - 2, a.z() - 11,
						a.x() + 11, a.y() + 18, a.z() + 11, AutoCollector.outlineColor(a.type()));
				var found = new StructureReader.Found(a.type(), originX, originZ,
						a.x() - 11, a.y() - 2, a.z() - 11, a.x() + 11, a.y() + 18, a.z() + 11);
				onRecorded.accept(found);
			}
		}
		if (sawUncalibrated && !warnedUncalibrated) {
			warnedUncalibrated = true;
			say.accept("§emc-locate§r saw a structure it can't place yet on this server. "
					+ "Calibrate once in singleplayer: stand by a desert pyramid and run "
					+ "§e/mclocate calibrate§r (needs two to confirm).");
		}
	}

	// ---- singleplayer calibration -------------------------------------------

	/**
	 * Learns anchor→origin offsets by matching detected anchors to the real
	 * origins the integrated server knows. Singleplayer only. Reports per type.
	 */
	public void calibrate(Minecraft client, Consumer<String> say) {
		if (client.level == null || client.player == null) {
			say.accept("§cNo world loaded.");
			return;
		}
		if (!client.hasSingleplayerServer()) {
			say.accept("§cCalibration needs your own singleplayer world — it learns the offset "
					+ "against the real structure origin, which a server does not expose.");
			return;
		}
		String version = client.getLaunchedVersion();
		List<Anchor> anchors = findAnchors(client.level, client.player.blockPosition(), CHUNK_RADIUS);
		if (anchors.isEmpty()) {
			say.accept("§7No detectable structure nearby. Stand within ~"
					+ (CHUNK_RADIUS * 16) + " blocks of a desert pyramid and try again.");
			return;
		}
		// Pull the authoritative origins, then match on the client thread.
		StructureReader.near(founds -> {
			int matched = 0;
			for (Anchor a : anchors) {
				StructureReader.Found real = enclosing(founds, a);
				if (real == null) {
					continue;
				}
				int dx = real.x() - a.x();
				int dz = real.z() - a.z();
				StructureOffsets.Offset o = offsets.observe(a.type(), version, dx, dz);
				matched++;
				say.accept(String.format(Locale.ROOT,
						"§bcalibrate§r %s: offset (%d, %d) — %s (%d/2)",
						a.type(), dx, dz,
						o.confirmed() ? "§aconfirmed§r, works on servers now" : "§eneed one more instance",
						o.count()));
			}
			if (matched == 0) {
				say.accept("§7Found an anchor but no matching structure origin — move so the whole "
						+ "structure is in loaded chunks, then retry.");
			}
		});
	}

	/** The known structure whose bounding box contains the anchor, if any. */
	private static StructureReader.Found enclosing(List<StructureReader.Found> founds, Anchor a) {
		for (StructureReader.Found f : founds) {
			if (f.type().equals(a.type())
					&& a.x() >= f.minX() && a.x() <= f.maxX()
					&& a.z() >= f.minZ() && a.z() <= f.maxZ()) {
				return f;
			}
		}
		return null;
	}

	/** Calibration state summary for /mclocate detect. */
	public String status(Minecraft client) {
		String version = client.getLaunchedVersion();
		StructureOffsets.Offset o = offsets.get("desert_pyramid", version);
		if (o == null) {
			return "desert_pyramid: §7not calibrated (run /mclocate calibrate in singleplayer)";
		}
		return "desert_pyramid: " + (o.confirmed()
				? "§aconfirmed§r offset (" + o.dx() + ", " + o.dz() + ")"
				: "§e" + o.count() + "/2 — needs one more instance in singleplayer");
	}

	// ---- anchor finding ------------------------------------------------------

	/**
	 * Sweeps loaded chunks near the player for structure anchors. Currently only
	 * the desert pyramid's TNT trap.
	 */
	public List<Anchor> findAnchors(ClientLevel level, BlockPos center, int chunkRadius) {
		int ccx = center.getX() >> 4;
		int ccz = center.getZ() >> 4;
		List<BlockPos> tnt = new ArrayList<>();
		for (int cx = ccx - chunkRadius; cx <= ccx + chunkRadius; cx++) {
			for (int cz = ccz - chunkRadius; cz <= ccz + chunkRadius; cz++) {
				collectTnt(level, cx, cz, tnt);
			}
		}
		List<Anchor> anchors = new ArrayList<>();
		for (int[] c : cluster(tnt)) {
			// Deterministic corner: min x, then z, then y.
			anchors.add(new Anchor("desert_pyramid", c[0], c[1], c[2]));
		}
		return anchors;
	}

	private static void collectTnt(ClientLevel level, int cx, int cz, List<BlockPos> out) {
		try {
			if (!level.hasChunk(cx, cz)) {
				return;
			}
			LevelChunk chunk = level.getChunk(cx, cz);
			LevelChunkSection[] sections = chunk.getSections();
			// getMinBuildHeight() was renamed getMinY() in 1.21.2.
			//? if >=1.21.2 {
			int minY = chunk.getMinY();
			//?} else
			/*int minY = chunk.getMinBuildHeight();*/
			for (int i = 0; i < sections.length; i++) {
				LevelChunkSection sec = sections[i];
				if (sec == null || sec.hasOnlyAir()) {
					continue;
				}
				// Fast palette reject: skip sections that cannot contain TNT.
				if (!sec.maybeHas(s -> s.is(Blocks.TNT))) {
					continue;
				}
				int secMinY = minY + i * 16;
				for (int ly = 0; ly < 16; ly++) {
					for (int lx = 0; lx < 16; lx++) {
						for (int lz = 0; lz < 16; lz++) {
							BlockState st = sec.getBlockState(lx, ly, lz);
							if (st.is(Blocks.TNT)) {
								out.add(new BlockPos(cx * 16 + lx, secMinY + ly, cz * 16 + lz));
							}
						}
					}
				}
			}
		} catch (Throwable t) {
			// A malformed/empty chunk must never crash the tick; just skip it.
		}
	}

	/**
	 * Groups TNT positions into clusters (one per pyramid) and returns each
	 * cluster's deterministic min corner as {x, y, z}.
	 */
	private static List<int[]> cluster(List<BlockPos> tnt) {
		List<int[]> corners = new ArrayList<>();
		boolean[] used = new boolean[tnt.size()];
		for (int i = 0; i < tnt.size(); i++) {
			if (used[i]) {
				continue;
			}
			// Flood the cluster reachable from i by CLUSTER_SPAN steps.
			List<Integer> group = new ArrayList<>();
			group.add(i);
			used[i] = true;
			for (int g = 0; g < group.size(); g++) {
				BlockPos a = tnt.get(group.get(g));
				for (int j = 0; j < tnt.size(); j++) {
					if (used[j]) {
						continue;
					}
					BlockPos b = tnt.get(j);
					if (Math.abs(a.getX() - b.getX()) <= CLUSTER_SPAN
							&& Math.abs(a.getZ() - b.getZ()) <= CLUSTER_SPAN
							&& Math.abs(a.getY() - b.getY()) <= CLUSTER_SPAN) {
						used[j] = true;
						group.add(j);
					}
				}
			}
			int mx = Integer.MAX_VALUE;
			int mz = Integer.MAX_VALUE;
			int my = Integer.MAX_VALUE;
			for (int idx : group) {
				BlockPos p = tnt.get(idx);
				// Order the corner by x, then z, then y so it is one fixed point.
				if (p.getX() < mx || (p.getX() == mx && p.getZ() < mz)
						|| (p.getX() == mx && p.getZ() == mz && p.getY() < my)) {
					mx = p.getX();
					mz = p.getZ();
					my = p.getY();
				}
			}
			corners.add(new int[] { mx, my, mz });
		}
		return corners;
	}
}
