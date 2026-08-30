package dev.lao.mclocate.client;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/**
 * The learned anchor→origin offsets a {@link StructureDetector} needs to turn a
 * block it can see into a structure's exact origin chunk — on a server, where
 * the integrated-server reader is unavailable.
 *
 * <p>The offset is never hardcoded (a guessed constant would silently kill the
 * crack). It is <em>measured</em> in singleplayer, where the mod knows the real
 * origin from the integrated server's {@code StructureStart}, and only trusted
 * once two independent instances agree. Keyed by {@code type@mcVersion} because
 * structure layouts shift between versions; switching version re-calibrates.
 *
 * <p>Stored as a flat properties file so there is no JSON dependency and a user
 * can read or hand-edit it. Only <em>confirmed</em> offsets are ever applied on
 * a server.
 */
public final class StructureOffsets {
	/** Distinct instances that must agree on an offset before it is trusted. */
	private static final int CONFIRM_AT = 2;

	/** One structure type's calibration state (for the current mc version). */
	public record Offset(int dx, int dz, int count, boolean confirmed) {
	}

	private final Path file;
	private final Properties props = new Properties();

	private StructureOffsets(Path file) {
		this.file = file;
	}

	public static StructureOffsets load(Path directory) {
		StructureOffsets o = new StructureOffsets(directory.resolve("structure-offsets.properties"));
		if (Files.isRegularFile(o.file)) {
			try (InputStream in = Files.newInputStream(o.file)) {
				o.props.load(in);
			} catch (IOException e) {
				// A bad file just means "not calibrated yet" — safe.
			}
		}
		return o;
	}

	private static String key(String type, String mcVersion) {
		return type + "@" + mcVersion;
	}

	/** The current calibration for a type, or null if none recorded. */
	public Offset get(String type, String mcVersion) {
		String k = key(type, mcVersion);
		String dx = props.getProperty(k + ".dx");
		String dz = props.getProperty(k + ".dz");
		if (dx == null || dz == null) {
			return null;
		}
		try {
			return new Offset(
					Integer.parseInt(dx.trim()),
					Integer.parseInt(dz.trim()),
					readInt(k + ".count", 0),
					Boolean.parseBoolean(props.getProperty(k + ".confirmed", "false").trim()));
		} catch (NumberFormatException e) {
			return null;
		}
	}

	public boolean isConfirmed(String type, String mcVersion) {
		Offset o = get(type, mcVersion);
		return o != null && o.confirmed();
	}

	/**
	 * Feeds one measured observation ({@code origin - anchor}) into calibration.
	 * A matching observation advances the confirmation count; a differing one
	 * resets it (a deterministic anchor should never disagree, so disagreement
	 * means the anchor is wrong and must not be trusted). Returns the resulting
	 * state.
	 */
	public Offset observe(String type, String mcVersion, int dx, int dz) {
		String k = key(type, mcVersion);
		Offset prev = get(type, mcVersion);
		int count;
		if (prev != null && prev.dx() == dx && prev.dz() == dz) {
			count = prev.count() + 1;
		} else {
			count = 1;
		}
		boolean confirmed = count >= CONFIRM_AT;
		props.setProperty(k + ".dx", Integer.toString(dx));
		props.setProperty(k + ".dz", Integer.toString(dz));
		props.setProperty(k + ".count", Integer.toString(count));
		props.setProperty(k + ".confirmed", Boolean.toString(confirmed));
		save();
		return new Offset(dx, dz, count, confirmed);
	}

	private int readInt(String key, int fallback) {
		try {
			return Integer.parseInt(props.getProperty(key, Integer.toString(fallback)).trim());
		} catch (NumberFormatException e) {
			return fallback;
		}
	}

	private void save() {
		try {
			Files.createDirectories(file.getParent());
			try (OutputStream out = Files.newOutputStream(file)) {
				props.store(out, "mc-locate structure anchor->origin offsets (calibrated in singleplayer)");
			}
		} catch (IOException e) {
			// The offset still applies this session; persistence is best-effort.
		}
	}
}
