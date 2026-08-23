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
	private static final String[] KNOWN_STRUCTURES = {
		"village", "desert_pyramid", "jungle_temple", "swamp_hut", "igloo",
		"pillager_outpost", "ocean_monument", "woodland_mansion", "ruined_portal",
		"shipwreck", "buried_treasure", "fortress", "bastion", "end_city",
	};

	private final Session session = new Session();
	private Config config;

	@Override
	public void onInitializeClient() {
		Path dir = outputDirectory();
		config = Config.load(dir);
		new AutoCollector(session, config).register();

		ClientCommandRegistrationCallback.EVENT.register((dispatcher, registry) -> {
			LiteralArgumentBuilder<FabricClientCommandSource> root = literal("mclocate");

			root.then(literal("bedrock")
					.executes(ctx -> scanBedrock(ctx.getSource(), DEFAULT_RADIUS))
					.then(argument("radius", IntegerArgumentType.integer(1, MAX_RADIUS))
							.executes(ctx -> scanBedrock(ctx.getSource(),
									IntegerArgumentType.getInteger(ctx, "radius")))));

			root.then(literal("pillars")
					.executes(ctx -> scanPillars(ctx.getSource())));

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

	private int setAuto(FabricClientCommandSource source, boolean on) {
		config.autoBedrock = on;
		config.save();
		feedback(source, on
				? "Passive collection §aon§r — bedrock is recorded as Nether chunks load."
				: "Passive collection §coff§r.");
		return 1;
	}

	private int status(FabricClientCommandSource source) {
		feedback(source, "§bmc-locate session§r");
		feedback(source, String.format(Locale.ROOT, "  bedrock: §a%d§r (≈%.1f bits)%s",
				session.bedrockCount(), session.bedrockCount() * 0.7219280948873623,
				session.droppedCount() > 0 ? " §7[" + session.droppedCount() + " dropped at cap]" : ""));
		feedback(source, "  pillars: " + (session.hasPillars() ? "§arecorded" : "§7none"));
		feedback(source, "  eye throws: §a" + session.throwCount());
		feedback(source, "  structures: §a" + session.structureCount());
		feedback(source, "  passive: " + (config.autoBedrock ? "§aon" : "§coff"));

		if (session.hasPillars()) {
			// The pillar shortcut leaves 2^32 structure seeds; bedrock is what
			// carves that down to one.
			double left = session.expectedSurvivorsOf(4294967296.0);
			feedback(source, String.format(Locale.ROOT,
					"  §7expected seeds still standing: %s",
					left < 1.5 ? "≈1 — you likely have enough" : String.format(Locale.ROOT, "≈%.0f", left)));
		} else {
			feedback(source, "  §7no candidate source yet — read the End pillars to get one");
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
