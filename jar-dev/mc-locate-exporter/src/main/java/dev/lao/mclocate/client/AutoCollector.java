package dev.lao.mclocate.client;

import java.nio.file.Path;
import java.util.List;

import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientChunkEvents;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientEntityEvents;
import net.fabricmc.fabric.api.client.event.lifecycle.v1.ClientTickEvents;
import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.entity.projectile.EyeOfEnder;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.LevelChunk;

/**
 * Passive collection: watches the world as you play instead of waiting for a
 * command.
 *
 * <p>The point is not convenience. Bedrock is a filter rather than a search —
 * it cannot find a seed on its own, it can only strike candidates off a list
 * that something else produced. The list normally comes from the End pillars,
 * which enumerate 2^32 structure seeds from a single observation. So the
 * intended shape of a session is: read the pillars once, then simply play,
 * while the Nether floor quietly narrows what is left.
 */
public final class AutoCollector {
	/** Ticks to keep retrying the pillar read after arriving in the End. */
	private static final int PILLAR_RETRY_TICKS = 400;

	private final Session session;
	private final Config config;
	private final Path outputDir;
	private final SeedDatabase seeds;
	private final EyeTracker eyes = new EyeTracker();

	private int pillarRetriesLeft;
	private boolean pillarsDone;

	/**
	 * The dimension seen last tick. AFTER_CLIENT_LEVEL_CHANGE would be tidier,
	 * but that event does not exist in 1.21.x's Fabric API, so the change is
	 * detected by watching this instead — which needs no event at all.
	 */
	private ResourceKey<Level> lastDimension;

	/** Announce progress at most this often, in ticks, to avoid chat spam. */
	private static final int ANNOUNCE_INTERVAL = 200;
	private int sinceAnnounce;
	private int collectedSinceAnnounce;

	/** Persist the rolling session at most this often, in ticks (~15s). */
	private static final int SAVE_INTERVAL = 300;
	private int sinceSave;
	private boolean wasInWorld;

	/** Refresh the action-bar HUD this often; the vanilla message fades by ~60. */
	private static final int HUD_INTERVAL = 30;
	private int sinceHud;

	/** Scan for newly-loaded structures this often (~2s). */
	private static final int STRUCT_INTERVAL = 40;
	private int sinceStruct;

	/** True while an async structure scan is running, so ticks do not stack them. */
	private boolean scanInFlight;

	/** Reset each time a world is left, so a known seed is flagged on every join. */
	private boolean knownChecked;

	public AutoCollector(Session session, Config config, Path outputDir, SeedDatabase seeds) {
		this.session = session;
		this.config = config;
		this.outputDir = outputDir;
		this.seeds = seeds;
	}

	public void register() {
		ClientChunkEvents.CHUNK_LOAD.register((level, chunk) -> onChunkLoad(level, chunk));
		ClientEntityEvents.ENTITY_LOAD.register((entity, level) -> {
			if (config.autoEyes && entity instanceof EyeOfEnder eye) {
				eyes.watch(eye);
			}
		});
		ClientTickEvents.END_CLIENT_TICK.register(client -> onTick(client));
	}

	/**
	 * Flags, once per world, whether it is a seed in the known-seeds database —
	 * by the real seed in singleplayer, by the biome hash on a server (which
	 * identifies the seed without reading it).
	 */
	private void checkKnownSeed(Minecraft client) {
		if (knownChecked) {
			return;
		}
		SeedDatabase.Entry known;
		if (client.hasSingleplayerServer()) {
			var server = client.getSingleplayerServer();
			if (server == null || server.overworld() == null) {
				return;
			}
			known = seeds.identify(true, server.overworld().getSeed(), null);
		} else {
			long bz = client.level.getBiomeManager().biomeZoomSeed;
			known = seeds.identify(false, 0L, bz);
		}
		knownChecked = true;
		if (known != null) {
			say(client, String.format(java.util.Locale.ROOT,
					"§bmc-locate§r known seed: §a%s§r (%d)", known.name(), known.seed()));
		}
	}

	/**
	 * In singleplayer the client owns the integrated server, so the true seed is
	 * right there — no cracking needed. Grabbing it also gives a way to check the
	 * whole pipeline: collect in a known world, then confirm the CLI recovers it.
	 */
	private void captureSeed(Minecraft client) {
		if (session.hasSeed() || !client.hasSingleplayerServer()) {
			return;
		}
		var server = client.getSingleplayerServer();
		if (server == null || server.overworld() == null) {
			return;
		}
		long seed = server.overworld().getSeed();
		if (session.setSeed(seed)) {
			say(client, "§bmc-locate§r captured world seed §a" + seed + "§r (singleplayer).");
		}
	}

	/**
	 * On a server the seed cannot be read, but the client stores the biome-zoom
	 * seed — the world seed hashed twice — which pins it just as well. The access
	 * widener opens the field; read it once per world. Singleplayer already has
	 * the real seed, so skip it there.
	 */
	private void captureBiomeHash(Minecraft client) {
		if (session.hasBiomeHash() || client.hasSingleplayerServer() || client.level == null) {
			return;
		}
		long bz = client.level.getBiomeManager().biomeZoomSeed;
		if (session.setBiomeHash(bz)) {
			say(client, "§bmc-locate§r captured the biome hash — the CLI can pin the seed from it");
		}
	}

	private void onLevelChange(Level level) {
		// Eyes do not survive a dimension change, and a stale entity reference
		// would report a bearing measured in a world we have left.
		eyes.forget();

		if (level != null && Level.END.equals(level.dimension())) {
			pillarsDone = false;
			pillarRetriesLeft = PILLAR_RETRY_TICKS;
		} else {
			pillarRetriesLeft = 0;
		}
	}

	private void onChunkLoad(Level level, LevelChunk chunk) {
		if (!config.autoBedrock || level == null || !Level.NETHER.equals(level.dimension())) {
			return;
		}
		int originX = chunk.getPos().getMinBlockX();
		int originZ = chunk.getPos().getMinBlockZ();
		int added = sampleLayer(level, originX, originZ, Collector.FLOOR_Y)
				+ sampleLayer(level, originX, originZ, Collector.ROOF_Y);
		collectedSinceAnnounce += added;
	}

	/**
	 * Samples one 16x16 chunk layer on a stride.
	 *
	 * <p>Every block in the layer would be 256 observations per chunk per
	 * level, which is far past the point of diminishing returns: roughly 45
	 * samples already carry the ~32 bits needed to pick a single seed out of
	 * the pillar candidates. The stride spends the budget over more chunks
	 * instead, which also spreads it over more of the layer seed's output.
	 */
	private int sampleLayer(Level level, int originX, int originZ, int y) {
		int added = 0;
		int stride = Math.max(1, config.bedrockStride);
		BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();

		for (int dx = 0; dx < 16; dx += stride) {
			for (int dz = 0; dz < 16; dz += stride) {
				int x = originX + dx;
				int z = originZ + dz;
				pos.set(x, y, z);

				// The chunk that fired the event is loaded, but isLoaded also
				// covers the y range, and a wrong answer here is poison: air
				// from an unloaded section reads as a confident "no bedrock".
				if (!level.isLoaded(pos)) {
					continue;
				}
				BlockState state = level.getBlockState(pos);
				if (session.addBedrock(x, y, z, state.getBlock() == Blocks.BEDROCK, config.maxBedrock)) {
					added++;
				}
			}
		}
		return added;
	}

	private void onTick(Minecraft client) {
		if (client.level == null) {
			if (wasInWorld) {
				// Left the world (quit to menu / disconnect): flush once so a
				// session is never lost to a forgotten export.
				wasInWorld = false;
				knownChecked = false;
				Persistence.save(outputDir, session);
			}
			return;
		}
		wasInWorld = true;
		captureSeed(client);
		captureBiomeHash(client);
		checkKnownSeed(client);
		trackStructures(client);

		ResourceKey<Level> dim = client.level.dimension();
		if (!dim.equals(lastDimension)) {
			lastDimension = dim;
			onLevelChange(client.level);
		}
		if (config.autoEyes) {
			List<EyeTracker.Throw> readings = eyes.tick();
			for (EyeTracker.Throw t : readings) {
				session.addThrow(t);
				say(client, String.format(java.util.Locale.ROOT,
						"§bmc-locate§r eye bearing %.2f° from (%.0f, %.0f) — %d throw(s)",
						t.yaw(), t.x(), t.z(), session.throwCount()));
			}
		}
		tickPillars(client);
		tickAnnounce(client);

		if (++sinceSave >= SAVE_INTERVAL) {
			sinceSave = 0;
			Persistence.save(outputDir, session);
		}
		updateHud(client);
	}

	/**
	 * Announces structures as their chunks load. Uses the integrated server's
	 * exact origins (singleplayer), so it says "found X at (x, z)" the moment you
	 * reach one, and records it for the crack. A server sends no structure starts,
	 * so this is silent there rather than guessing.
	 */
	private void trackStructures(Minecraft client) {
		if (!client.hasSingleplayerServer() || client.player == null || client.level == null) {
			return;
		}
		if (!config.announceStructures) {
			return;
		}
		if (++sinceStruct >= STRUCT_INTERVAL && !scanInFlight) {
			sinceStruct = 0;
			scanInFlight = true;
			StructureReader.scan(session, result -> {
				scanInFlight = false;
				for (StructureReader.Found f : result.found()) {
					if (config.announceStructures) {
						say(client, String.format(java.util.Locale.ROOT,
								"§bmc-locate§r found §a%s§r at %d, %d",
								f.type(), f.x(), f.z()));
					}
				}
			});
		}
	}

	/**
	 * Paints a live one-line readout on the action bar.
	 *
	 * <p>Only 1.21.x and 26.1.x expose the action-bar setter; 26.2's render
	 * rewrite removed it, so this compiles to a no-op there and the toggle says
	 * as much.
	 */
	private void updateHud(Minecraft client) {
		if (!config.hud || client.player == null) {
			return;
		}
		if (++sinceHud < HUD_INTERVAL) {
			return;
		}
		sinceHud = 0;
		//? if <26.2 {
		/*int n = session.bedrockCount();
		String tail = session.hasSeed() ? " §aseed!" : session.hasPillars() ? " pillars" : "";
		String text = String.format(java.util.Locale.ROOT,
				"§bmc-locate§r %d bedrock · %d throw(s)%s", n, session.throwCount(), tail);
		client.gui.setOverlayMessage(Component.literal(text), false);
		*///?}
	}

	private void tickPillars(Minecraft client) {
		if (pillarsDone || pillarRetriesLeft <= 0 || !config.autoPillars) {
			return;
		}
		pillarRetriesLeft--;

		// Only worth a look once a second; the scan touches ten columns and
		// nothing changes between ticks.
		if (pillarRetriesLeft % 20 != 0) {
			return;
		}
		Integer[] heights = Collector.collectPillarHeights(client.level);
		int measured = Collector.measuredPillars(heights);

		if (measured == 0) {
			return;
		}
		if (!Collector.heightsLookValid(heights)) {
			// Something other than a pillar was measured. Writing this would
			// eliminate the true seed, so keep waiting instead.
			return;
		}
		if (measured < heights.length && pillarRetriesLeft > 20) {
			// Partial reads still constrain the seed, but hold out for the full
			// set while there is time left on the clock.
			return;
		}
		session.setPillarHeights(heights);
		pillarsDone = true;
		say(client, "§bmc-locate§r read " + measured
				+ "/10 End pillars — run §e/mclocate export§r when ready");
	}

	private void tickAnnounce(Minecraft client) {
		if (!config.announce || collectedSinceAnnounce == 0) {
			return;
		}
		if (++sinceAnnounce < ANNOUNCE_INTERVAL) {
			return;
		}
		sinceAnnounce = 0;
		int n = session.bedrockCount();
		say(client, String.format(java.util.Locale.ROOT,
				"§bmc-locate§r +%d bedrock (%d total, ≈%.1f bits)",
				collectedSinceAnnounce, n, n * 0.7219280948873623));
		collectedSinceAnnounce = 0;
	}

	private static void say(Minecraft client, String message) {
		if (client.player != null) {
			// sendSystemMessage exists on 26.x and 1.21.1 but not 1.21.11;
			// displayClientMessage exists on all 1.21.x but not 26.x. Split at 26.1.
			//? if <26.1 {
			/*client.player.displayClientMessage(Component.literal(message), false);
			*///?} else
			client.player.sendSystemMessage(Component.literal(message));
		}
	}
}
