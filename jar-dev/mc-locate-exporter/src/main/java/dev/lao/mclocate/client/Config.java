package dev.lao.mclocate.client;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/**
 * Mod settings, persisted next to the exported observations.
 *
 * <p>Passive collection defaults to <em>off</em>. It writes a file that grows
 * as you play, and a mod that starts logging your world without being asked is
 * a surprise even when the data is harmless.
 */
public final class Config {
	/** Sample every Nth block along each axis of a chunk layer. */
	public int bedrockStride = 4;

	/** Stop accumulating past this many bedrock samples. */
	public int maxBedrock = 4096;

	/** Collect bedrock from Nether chunks as they load. */
	public boolean autoBedrock = false;

	/** Read the pillar heights on entering the End. */
	public boolean autoPillars = true;

	/** Record the bearing of thrown eyes of ender. */
	public boolean autoEyes = true;

	/** Print a chat line when something is collected. */
	public boolean announce = true;

	/** Show a live status readout on the action bar (no effect on 26.2+). */
	public boolean hud = false;

	/** Announce structures as they load (singleplayer). */
	public boolean announceStructures = true;

	/** Draw a particle outline around found structures. */
	public boolean outline = true;

	private Path file;

	public static Config load(Path directory) {
		Config cfg = new Config();
		cfg.file = directory.resolve("config.properties");
		Properties p = new Properties();

		if (Files.isRegularFile(cfg.file)) {
			try (InputStream in = Files.newInputStream(cfg.file)) {
				p.load(in);
			} catch (IOException e) {
				// A corrupt config should not stop the game loading; the
				// defaults above are all safe.
				return cfg;
			}
		}

		cfg.bedrockStride = clamp(readInt(p, "bedrockStride", cfg.bedrockStride), 1, 16);
		cfg.maxBedrock = clamp(readInt(p, "maxBedrock", cfg.maxBedrock), 64, 1_000_000);
		cfg.autoBedrock = readBool(p, "autoBedrock", cfg.autoBedrock);
		cfg.autoPillars = readBool(p, "autoPillars", cfg.autoPillars);
		cfg.autoEyes = readBool(p, "autoEyes", cfg.autoEyes);
		cfg.announce = readBool(p, "announce", cfg.announce);
		cfg.hud = readBool(p, "hud", cfg.hud);
		cfg.announceStructures = readBool(p, "announceStructures", cfg.announceStructures);
		cfg.outline = readBool(p, "outline", cfg.outline);
		return cfg;
	}

	public void save() {
		if (file == null) {
			return;
		}
		Properties p = new Properties();
		p.setProperty("bedrockStride", Integer.toString(bedrockStride));
		p.setProperty("maxBedrock", Integer.toString(maxBedrock));
		p.setProperty("autoBedrock", Boolean.toString(autoBedrock));
		p.setProperty("autoPillars", Boolean.toString(autoPillars));
		p.setProperty("autoEyes", Boolean.toString(autoEyes));
		p.setProperty("announce", Boolean.toString(announce));
		p.setProperty("hud", Boolean.toString(hud));
		p.setProperty("announceStructures", Boolean.toString(announceStructures));
		p.setProperty("outline", Boolean.toString(outline));

		try {
			Files.createDirectories(file.getParent());
			try (OutputStream out = Files.newOutputStream(file)) {
				p.store(out, "mc-locate exporter settings");
			}
		} catch (IOException e) {
			// Nothing useful to do; the setting still applies this session.
		}
	}

	private static int readInt(Properties p, String key, int fallback) {
		try {
			return Integer.parseInt(p.getProperty(key, Integer.toString(fallback)).trim());
		} catch (NumberFormatException e) {
			return fallback;
		}
	}

	private static boolean readBool(Properties p, String key, boolean fallback) {
		String v = p.getProperty(key);
		return v == null ? fallback : Boolean.parseBoolean(v.trim());
	}

	private static int clamp(int v, int lo, int hi) {
		return Math.max(lo, Math.min(hi, v));
	}
}
