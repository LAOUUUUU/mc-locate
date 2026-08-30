package dev.lao.mclocate.client;

import java.io.IOException;
import java.nio.file.Path;
import java.util.Locale;

import com.mojang.brigadier.arguments.ArgumentType;
import com.mojang.brigadier.arguments.IntegerArgumentType;
import com.mojang.brigadier.arguments.StringArgumentType;
import com.mojang.brigadier.builder.LiteralArgumentBuilder;
import com.mojang.brigadier.builder.RequiredArgumentBuilder;

import net.fabricmc.api.ClientModInitializer;
import net.fabricmc.fabric.api.client.command.v2.ClientCommandRegistrationCallback;
import net.fabricmc.fabric.api.client.command.v2.FabricClientCommandSource;
import net.minecraft.client.Minecraft;
import net.minecraft.network.chat.Component;
import net.minecraft.world.level.Level;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Client entry point: registers {@code /mclocate} and starts passive
 * collection.
 *
 * <p>Nothing here talks to a server or reads anything the player cannot
 * already see. Every observation is a block the client has been sent because
 * the player is standing near it.
 */
public class ExporterClient implements ClientModInitializer {
	public static final String MOD_ID = "mc-locate-exporter";
	private static final Logger LOGGER = LoggerFactory.getLogger(MOD_ID);

	private static final int DEFAULT_RADIUS = 24;
	private static final int MAX_RADIUS = 128;

	/** Structure names mc-locate understands, for {@code /mclocate mark}. */
	// Every name here is one the CLI's parse_structure accepts; the Rust test
	// exporter_structure_names locks the two lists together.
	private static final String[] KNOWN_STRUCTURES = {
		"village", "desert_pyramid", "jungle_temple", "swamp_hut", "igloo",
		"ocean_ruin", "shipwreck", "ocean_monument", "woodland_mansion",
		"pillager_outpost", "ruined_portal", "ancient_city", "buried_treasure",
		"mineshaft", "trail_ruins", "trial_chambers", "nether_fortress",
		"bastion_remnant", "ruined_portal_nether", "end_city", "end_gateway",
	};

	private final Session session = new Session();
	private Config config;
	private SeedDatabase seeds;
	private StructureOffsets offsets;
	private StructureDetector detector;

	@Override
	public void onInitializeClient() {
		Path dir = outputDirectory();
		config = Config.load(dir);

		// Restore a session left by a previous run, so a crash or a forgotten
		// export does not throw away an afternoon of collecting.
		String restored = Persistence.load(dir, session, config.maxBedrock);
		if (restored != null) {
			LOGGER.info("mc-locate: {}", restored);
		}

		seeds = SeedDatabase.load(dir);
		offsets = StructureOffsets.load(dir);
		detector = new StructureDetector(offsets);
		new AutoCollector(session, config, dir, seeds, detector).register();

		ClientCommandRegistrationCallback.EVENT.register((dispatcher, registry) -> {
			LiteralArgumentBuilder<FabricClientCommandSource> root = literal("mclocate");

			root.then(literal("bedrock")
					.executes(ctx -> scanBedrock(ctx.getSource(), DEFAULT_RADIUS))
					.then(argument("radius", IntegerArgumentType.integer(1, MAX_RADIUS))
							.executes(ctx -> scanBedrock(ctx.getSource(),
									IntegerArgumentType.getInteger(ctx, "radius")))));

			root.then(literal("pillars")
					.executes(ctx -> scanPillars(ctx.getSource())));

			// Master switch — the whole mod on/off.
			root.then(literal("on").executes(ctx -> setEnabled(ctx.getSource(), true)));
			root.then(literal("off").executes(ctx -> setEnabled(ctx.getSource(), false)));

			// Client-side structure detection (servers): calibrate in SP, then it
			// runs automatically on servers.
			root.then(literal("calibrate").executes(ctx -> calibrate(ctx.getSource())));
			root.then(literal("detect").executes(ctx -> detectStatus(ctx.getSource())));

			root.then(literal("auto")
					.then(literal("on").executes(ctx -> setAuto(ctx.getSource(), true)))
					.then(literal("off").executes(ctx -> setAuto(ctx.getSource(), false)))
					.executes(ctx -> {
						feedback(ctx.getSource(), "Passive collection is "
								+ (config.autoBedrock ? "§aon" : "§coff") + "§r. Use §e/mclocate auto on§r.");
						return 1;
					}));

			root.then(literal("status")
					.executes(ctx -> status(ctx.getSource())));

			root.then(literal("export")
					.executes(ctx -> export(ctx.getSource())));

			root.then(literal("clear")
					.executes(ctx -> {
						session.clear();
						feedback(ctx.getSource(), "Session cleared.");
						return 1;
					}));

			root.then(literal("here")
					.executes(ctx -> here(ctx.getSource())));

			root.then(literal("mark")
					.then(argument("type", StringArgumentType.word())
							.executes(ctx -> mark(ctx.getSource(),
									StringArgumentType.getString(ctx, "type")))));

			root.then(literal("seed")
					.executes(ctx -> captureSeed(ctx.getSource())));

			root.then(literal("structures")
					.executes(ctx -> scanStructures(ctx.getSource()))
					.then(literal("verify")
							.executes(ctx -> verifyStructures(ctx.getSource()))));

			root.then(literal("known")
					.executes(ctx -> listKnown(ctx.getSource()))
					.then(literal("add")
							.then(argument("name", StringArgumentType.greedyString())
									.executes(ctx -> addKnown(ctx.getSource(),
											StringArgumentType.getString(ctx, "name"))))));

			// Slime chunks are recorded by hand: a single slime spawn is not
			// proof (swamps spawn them too), so the player confirms it. `slime`
			// marks the current chunk as a slime chunk, `slime not` as confirmed
			// ordinary. A wrong "yes" here eliminates the true seed, hence manual.
			root.then(literal("slime")
					.executes(ctx -> markSlime(ctx.getSource(), true))
					.then(literal("not").executes(ctx -> markSlime(ctx.getSource(), false))));

			root.then(literal("config")
					.executes(ctx -> showConfig(ctx.getSource()))
					.then(argument("key", StringArgumentType.word())
							.then(argument("value", StringArgumentType.word())
									.executes(ctx -> setConfig(ctx.getSource(),
											StringArgumentType.getString(ctx, "key"),
											StringArgumentType.getString(ctx, "value"))))));

			root.then(literal("shot")
					.executes(ctx -> takeShot(ctx.getSource())));

			root.then(literal("hud")
					.then(literal("on").executes(ctx -> setHud(ctx.getSource(), true)))
					.then(literal("off").executes(ctx -> setHud(ctx.getSource(), false))));

			root.then(literal("gui")
					.executes(ctx -> openGui(ctx.getSource())));

			dispatcher.register(root);
		});

		LOGGER.info("mc-locate exporter ready; observations go to {}", outputDirectory());
	}

	private static Path outputDirectory() {
		return Minecraft.getInstance().gameDirectory.toPath().resolve("mc-locate");
	}

	private int scanBedrock(FabricClientCommandSource source, int radius) {
		Minecraft client = Minecraft.getInstance();
		if (client.level == null || client.player == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		if (!Level.NETHER.equals(client.level.dimension())) {
			feedback(source, "§cBedrock sampling only means anything in the Nether.");
			return 0;
		}
		int cx = (int) Math.floor(client.player.getX());
		int cz = (int) Math.floor(client.player.getZ());
		int before = session.bedrockCount();

		for (int y : new int[] { Collector.FLOOR_Y, Collector.ROOF_Y }) {
			for (int x = cx - radius; x <= cx + radius; x++) {
				for (int z = cz - radius; z <= cz + radius; z++) {
					net.minecraft.core.BlockPos pos = new net.minecraft.core.BlockPos(x, y, z);
					if (!client.level.isLoaded(pos)) {
						continue;
					}
					boolean isBedrock = client.level.getBlockState(pos).getBlock()
							== net.minecraft.world.level.block.Blocks.BEDROCK;
					session.addBedrock(x, y, z, isBedrock, config.maxBedrock);
				}
			}
		}
		int added = session.bedrockCount() - before;
		feedback(source, String.format(Locale.ROOT,
				"Added §a%d§r new bedrock sample(s); §a%d§r in session (≈%.1f bits).",
				added, session.bedrockCount(), session.bedrockCount() * 0.7219280948873623));
		return 1;
	}

	private int scanPillars(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		if (client.level == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		if (!Level.END.equals(client.level.dimension())) {
			feedback(source, "§cYou need to be in the End.");
			return 0;
		}
		Integer[] heights = Collector.collectPillarHeights(client.level);
		int measured = Collector.measuredPillars(heights);

		if (measured == 0) {
			feedback(source, "§cNo pillars in range — fly closer to the central island.");
			return 0;
		}
		if (!Collector.heightsLookValid(heights)) {
			feedback(source, "§cThose readings are not a valid pillar set, so nothing was saved. "
					+ "Something other than a pillar was probably measured.");
			return 0;
		}
		session.setPillarHeights(heights);
		feedback(source, "Recorded §a" + measured + "/10§r pillar heights.");
		return 1;
	}

	private int setEnabled(FabricClientCommandSource source, boolean on) {
		config.enabled = on;
		config.save();
		if (!on) {
			Outlines.enabled = false;
			// Flush before going inert so nothing collected is lost.
			Persistence.save(outputDirectory(), session);
			feedback(source, "§bmc-locate §cOFF§r — inert: no collection, capture, detection, or outline. "
					+ "Turn back on with §e/mclocate on§r.");
		} else {
			feedback(source, "§bmc-locate §aON§r — collecting and detecting again.");
		}
		return 1;
	}

	private int calibrate(FabricClientCommandSource source) {
		detector.calibrate(Minecraft.getInstance(), msg -> feedback(source, msg));
		return 1;
	}

	private int detectStatus(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		feedback(source, "§bmc-locate detection§r  " + detector.status(client));
		if (client.hasSingleplayerServer()) {
			feedback(source, "§7Singleplayer uses the exact reader (§e/mclocate structures§7). "
					+ "Calibrate for servers here: stand by a desert pyramid, run §e/mclocate calibrate§7 "
					+ "(twice, on different pyramids).");
		} else {
			feedback(source, "§7On a server this runs automatically as you explore — but only once its "
					+ "offset is confirmed in singleplayer. Detected origins feed the crack via the biome hash.");
		}
		return 1;
	}

	private int setAuto(FabricClientCommandSource source, boolean on) {
		config.autoBedrock = on;
		config.save();
		feedback(source, on
				? "Passive collection §aon§r — bedrock is recorded as Nether chunks load."
				: "Passive collection §coff§r.");
		return 1;
	}

	private int status(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		boolean server = !client.hasSingleplayerServer();
		feedback(source, "§bmc-locate session§r  §7(" + (server ? "multiplayer" : "singleplayer") + ")");
		feedback(source, String.format(Locale.ROOT, "  bedrock: §a%d§r (≈%.1f bits)%s",
				session.bedrockCount(), session.bedrockCount() * 0.7219280948873623,
				session.droppedCount() > 0 ? " §7[" + session.droppedCount() + " dropped at cap]" : ""));
		feedback(source, "  pillars: " + (session.hasPillars() ? "§arecorded" : "§7none"));
		feedback(source, "  slime chunks: §a" + session.slimeCount());
		feedback(source, "  eye throws: §a" + session.throwCount());
		feedback(source, "  structures: §a" + session.structureCount());
		feedback(source, "  passive: " + (config.autoBedrock ? "§aon" : "§coff"));

		if (session.hasSeed()) {
			feedback(source, "  §aseed known§r — export and you are done; no cracking needed");
		} else if (session.hasPillars()) {
			// The pillar shortcut leaves 2^32 structure seeds; bedrock is what
			// carves that down to one.
			double left = session.expectedSurvivorsOf(4294967296.0);
			feedback(source, String.format(Locale.ROOT,
					"  §7expected seeds still standing: %s",
					left < 1.5 ? "≈1 — you likely have enough" : String.format(Locale.ROOT, "≈%.0f", left)));
		} else {
			feedback(source, "  §7no candidate source yet — read the End pillars to get one");
		}
		if (server && !session.hasSeed()) {
			// The seed cannot be read off a server, so the whole point here is to
			// collect and crack. Say so, since /mclocate seed will refuse.
			feedback(source, "  §7on a server: collect here, then crack in the CLI "
					+ "(the seed can't be read directly). Check the server allows seed cracking.");
		}
		return 1;
	}

	private int here(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		if (client.player == null || client.level == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		feedback(source, String.format(Locale.ROOT, "§bhere§r %.1f %.1f %.1f  yaw %.2f  in %s",
				client.player.getX(), client.player.getY(), client.player.getZ(),
				client.player.getYRot(), dimensionName(client.level)));
		return 1;
	}

	private int mark(FabricClientCommandSource source, String type) {
		Minecraft client = Minecraft.getInstance();
		if (client.player == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		String normalised = type.toLowerCase(Locale.ROOT);
		boolean known = false;
		for (String s : KNOWN_STRUCTURES) {
			if (s.equals(normalised)) {
				known = true;
				break;
			}
		}
		if (!known) {
			feedback(source, "§cUnknown structure §e" + normalised + "§c. Known: "
					+ String.join(", ", KNOWN_STRUCTURES));
			return 0;
		}
		int x = (int) Math.floor(client.player.getX());
		int z = (int) Math.floor(client.player.getZ());
		session.addStructure(normalised, x, z);
		feedback(source, "Marked §a" + normalised + "§r at " + x + ", " + z + ".");
		return 1;
	}

	private int listKnown(FabricClientCommandSource source) {
		var all = seeds.entries();
		if (all.isEmpty()) {
			feedback(source, "§7No known seeds yet. Add one with §e/mclocate known add <name>§7 "
					+ "(singleplayer), or edit §emc-locate/known-seeds.json§7. You are then told "
					+ "when you join that seed — even on a server, via its biome hash.");
			return 1;
		}
		feedback(source, "§bKnown seeds§r (" + all.size() + "):");
		for (SeedDatabase.Entry e : all) {
			feedback(source, "  §a" + e.name() + "§r — " + e.seed());
		}
		return 1;
	}

	private int addKnown(FabricClientCommandSource source, String name) {
		Minecraft client = Minecraft.getInstance();
		if (!client.hasSingleplayerServer() || client.getSingleplayerServer() == null
				|| client.getSingleplayerServer().overworld() == null) {
			feedback(source, "§cAdding needs your own singleplayer world (to read the seed). "
					+ "On a server, add the seed by editing mc-locate/known-seeds.json.");
			return 0;
		}
		long seed = client.getSingleplayerServer().overworld().getSeed();
		if (seeds.add(seed, name)) {
			feedback(source, "Added §a" + name + "§r (" + seed + ") to the known seeds.");
		} else {
			feedback(source, "§7That seed (" + seed + ") is already known.");
		}
		return 1;
	}

	private int verifyStructures(FabricClientCommandSource source) {
		StructureReader.verify(rows -> {
			if (rows.isEmpty()) {
				feedback(source, "§7No structures in range to calibrate against — explore toward one.");
				return;
			}
			feedback(source, "§bmc-locate structure calibration§r (singleplayer oracle):");
			for (String row : rows) {
				feedback(source, "  " + row);
			}
			feedback(source, "§7These rows establish the origin/bounding-box relationship a "
					+ "server-side detector will need. Share them if a detector misbehaves.");
		});
		return 1;
	}

	private int scanStructures(FabricClientCommandSource source) {
		StructureReader.scan(session, r -> {
			if (r.serverUnsupported()) {
				feedback(source, "§cStructure reading needs your own singleplayer world — a server "
						+ "does not send structure positions to the client. (Bedrock, pillars and eye "
						+ "throws still work on servers.)");
				return;
			}
			if (r.added() == 0) {
				feedback(source, "§7No new structures in the loaded chunks around you. "
						+ "Explore toward one and run this again.");
				return;
			}
			for (StructureReader.Found f : r.found()) {
				feedback(source, "  §afound§r " + f.type() + " at " + f.x() + ", " + f.z());
			}
			feedback(source, "Recorded §a" + r.added() + "§r structure(s); §a"
					+ session.structureCount() + "§r in session.");
		});
		return 1;
	}

	private int captureSeed(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		if (!client.hasSingleplayerServer() || client.getSingleplayerServer() == null
				|| client.getSingleplayerServer().overworld() == null) {
			feedback(source, "§cThe seed is only readable in your own singleplayer world.");
			return 0;
		}
		long seed = client.getSingleplayerServer().overworld().getSeed();
		session.setSeed(seed);
		feedback(source, "Recorded world seed §a" + seed + "§r.");
		return 1;
	}

	private int markSlime(FabricClientCommandSource source, boolean isSlime) {
		Minecraft client = Minecraft.getInstance();
		if (client.player == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		int chunkX = Math.floorDiv((int) Math.floor(client.player.getX()), 16);
		int chunkZ = Math.floorDiv((int) Math.floor(client.player.getZ()), 16);
		session.addSlime(chunkX, chunkZ, isSlime);
		feedback(source, "Chunk §a" + chunkX + ", " + chunkZ + "§r marked "
				+ (isSlime ? "§aslime" : "§cnot slime") + "§r.");
		if (isSlime) {
			feedback(source, "§7Only mark a chunk you have confirmed by a slime spawn below y=40.");
		}
		return 1;
	}

	private int openGui(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		Config cfg = this.config;
		// Deferred to the client thread: a command runs while the chat screen is
		// closing, so open the config next tick. setScreen was renamed in 26.2.
		client.execute(() -> {
			//? if >=26.2 {
			client.setScreenAndShow(new ConfigScreen(cfg));
			//?} else
			//client.setScreen(new ConfigScreen(cfg));
		});
		feedback(source, "Opening the mc-locate settings…");
		return 1;
	}

	private int takeShot(FabricClientCommandSource source) {
		Minecraft client = Minecraft.getInstance();
		if (client.level == null) {
			feedback(source, "§cNo world loaded.");
			return 0;
		}
		// 26.2's render rewrite dropped getMainRenderTarget and added a
		// Minecraft-only grab; earlier versions take the render target directly.
		//? if >=26.2 {
		net.minecraft.client.Screenshot.grab(client, false);
		//?} else
		//net.minecraft.client.Screenshot.grab(client.gameDirectory, client.getMainRenderTarget(), msg -> {});
		feedback(source, "Screenshot saved to the §escreenshots§r folder — the CLI's screenshot watcher can OCR it.");
		return 1;
	}

	private int setHud(FabricClientCommandSource source, boolean on) {
		config.hud = on;
		config.save();
		if (!on) {
			feedback(source, "HUD §coff§r.");
			return 1;
		}
		//? if <26.2 {
		/*feedback(source, "HUD §aon§r — live status shows on the action bar.");
		*///?} else
		feedback(source, "§eHUD on, but 26.2 removed the client action bar (render rewrite); it won't show. Use /mclocate status.");
		return 1;
	}

	private int showConfig(FabricClientCommandSource source) {
		feedback(source, "§bmc-locate config§r  (change with §e/mclocate config <key> <value>§r)");
		feedback(source, "  enabled = " + (config.enabled ? "§atrue" : "§cfalse") + "§r  §7(master; /mclocate on|off)");
		feedback(source, "  autoBedrock = " + config.autoBedrock);
		feedback(source, "  autoPillars = " + config.autoPillars);
		feedback(source, "  autoEyes = " + config.autoEyes);
		feedback(source, "  announce = " + config.announce);
		feedback(source, "  announceStructures = " + config.announceStructures);
		feedback(source, "  outline = " + config.outline);
		feedback(source, "  detectStructures = " + config.detectStructures + "  §7(server-side, needs calibration)");
		feedback(source, "  hud = " + config.hud);
		feedback(source, "  bedrockStride = " + config.bedrockStride + "  §7(1-16)");
		feedback(source, "  maxBedrock = " + config.maxBedrock);
		return 1;
	}

	private int setConfig(FabricClientCommandSource source, String key, String value) {
		try {
			switch (key.toLowerCase(Locale.ROOT)) {
				case "enabled" -> config.enabled = Boolean.parseBoolean(value);
				case "autobedrock" -> config.autoBedrock = Boolean.parseBoolean(value);
				case "autopillars" -> config.autoPillars = Boolean.parseBoolean(value);
				case "autoeyes" -> config.autoEyes = Boolean.parseBoolean(value);
				case "announce" -> config.announce = Boolean.parseBoolean(value);
				case "announcestructures" -> config.announceStructures = Boolean.parseBoolean(value);
				case "outline" -> config.outline = Boolean.parseBoolean(value);
				case "detectstructures" -> config.detectStructures = Boolean.parseBoolean(value);
				case "hud" -> config.hud = Boolean.parseBoolean(value);
				case "bedrockstride" -> config.bedrockStride = Math.max(1, Math.min(16, Integer.parseInt(value)));
				case "maxbedrock" -> config.maxBedrock = Math.max(64, Integer.parseInt(value));
				default -> {
					feedback(source, "§cUnknown key §e" + key + "§c. See §e/mclocate config§c.");
					return 0;
				}
			}
		} catch (NumberFormatException e) {
			feedback(source, "§c'" + value + "' is not a valid value for " + key + ".");
			return 0;
		}
		config.save();
		feedback(source, "Set §a" + key + "§r = §a" + value + "§r.");
		return 1;
	}

	private int export(FabricClientCommandSource source) {
		if (session.isEmpty()) {
			feedback(source, "§cNothing collected yet.");
			return 0;
		}
		Minecraft client = Minecraft.getInstance();
		session.setMinecraftVersion(client.getLaunchedVersion());

		try {
			Path out = session.write(outputDirectory(),
					"observations-" + System.currentTimeMillis() + ".json");
			feedback(source, "Wrote §a" + out.getFileName() + "§r to §e" + out.getParent() + "§r.");
			feedback(source, "§7Load it with mc-locate's \"Import observations\" mode.");
			LOGGER.info("wrote {}", out);
			return 1;
		} catch (IOException e) {
			feedback(source, "§cCould not write the file: " + e.getMessage());
			LOGGER.error("export failed", e);
			return 0;
		}
	}

	// The Fabric convenience class for these was renamed ClientCommandManager ->
	// ClientCommands between 1.21 and 26.x. Brigadier's own builder statics are
	// what both versions wrap, so calling them directly is version-proof.
	private static LiteralArgumentBuilder<FabricClientCommandSource> literal(String name) {
		return LiteralArgumentBuilder.literal(name);
	}

	private static <T> RequiredArgumentBuilder<FabricClientCommandSource, T> argument(
			String name, ArgumentType<T> type) {
		return RequiredArgumentBuilder.argument(name, type);
	}

	/**
	 * A short dimension label, derived by comparing the level's key to the three
	 * known ones. This sidesteps the Identifier/Identifier accessor rename
	 * between 1.21 and 26.x entirely.
	 */
	private static String dimensionName(Level level) {
		if (Level.NETHER.equals(level.dimension())) {
			return "the_nether";
		}
		if (Level.END.equals(level.dimension())) {
			return "the_end";
		}
		if (Level.OVERWORLD.equals(level.dimension())) {
			return "overworld";
		}
		return "other";
	}

	private static void feedback(FabricClientCommandSource source, String message) {
		source.sendFeedback(Component.literal(message));
	}
}
