package dev.lao.mclocate.client;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

/**
 * The mc-locate observation file, built up in memory and written as JSON.
 *
 * <p>The schema is defined by mc-locate's {@code src/observations.rs}. It is
 * deliberately forgiving: every field is optional, and a reader ignores keys it
 * does not recognise. That matters because this mod and the tool are versioned
 * separately and will drift.
 *
 * <p>JSON is written by hand rather than pulling in a library. The document is
 * a handful of flat arrays of numbers and booleans, so a dependency would cost
 * more than it saves — and the only string that ever reaches the output is a
 * structure name from a fixed set.
 */
public final class Observations {
	/** Bumped only for a breaking change; new fields do not need it. */
	private static final int FORMAT_VERSION = 1;

	private final List<String> bedrock = new ArrayList<>();
	private final List<String> slime = new ArrayList<>();
	private final List<String> structures = new ArrayList<>();
	private final List<String> eyeThrows = new ArrayList<>();
	private Integer[] pillarHeights;
	private String minecraftVersion;
	private Long seed;
	private Long biomeHash;

	public void addBedrock(int x, int y, int z, boolean isBedrock) {
		bedrock.add(String.format(Locale.ROOT,
				"{\"x\": %d, \"y\": %d, \"z\": %d, \"is_bedrock\": %s}",
				x, y, z, isBedrock));
	}

	public void addSlimeChunk(int chunkX, int chunkZ, boolean isSlime) {
		slime.add(String.format(Locale.ROOT,
				"{\"chunk_x\": %d, \"chunk_z\": %d, \"is_slime\": %s}",
				chunkX, chunkZ, isSlime));
	}

	/** A located structure, as mc-locate's {@code StructureDto}. */
	public void addStructure(String type, int x, int z) {
		structures.add(String.format(Locale.ROOT,
				"{\"type\": \"%s\", \"x\": %d, \"z\": %d}", escape(type), x, z));
	}

	/**
	 * The bearing of a thrown eye of ender, for stronghold triangulation.
	 *
	 * <p>Locale.ROOT is not optional here. Formatting a double under a locale
	 * that uses a decimal comma would write {@code 41,7}, which is not a JSON
	 * number, and the file would fail to parse on exactly the machines whose
	 * users never see it in testing.
	 */
	public void addEyeThrow(double x, double z, double yaw) {
		eyeThrows.add(String.format(Locale.ROOT,
				"{\"x\": %.3f, \"z\": %.3f, \"yaw\": %.4f}", x, z, yaw));
	}

	/** The ten pillar heights in mc-locate's fixed order; nulls for unmeasured. */
	public void setPillarHeights(Integer[] heights) {
		this.pillarHeights = heights;
	}

	public void setMinecraftVersion(String version) {
		this.minecraftVersion = version;
	}

	/** The known world seed (singleplayer ground truth), if any. */
	public void setSeed(Long seed) {
		this.seed = seed;
	}

	/** The client's biome-zoom seed (doubly-hashed world seed); pins the seed. */
	public void setBiomeHash(Long biomeHash) {
		this.biomeHash = biomeHash;
	}

	public int bedrockCount() {
		return bedrock.size();
	}

	public int slimeCount() {
		return slime.size();
	}

	public boolean hasPillars() {
		return pillarHeights != null;
	}

	public int structureCount() {
		return structures.size();
	}

	public int eyeThrowCount() {
		return eyeThrows.size();
	}

	public boolean isEmpty() {
		return bedrock.isEmpty() && slime.isEmpty() && structures.isEmpty()
				&& eyeThrows.isEmpty() && pillarHeights == null && seed == null && biomeHash == null;
	}

	public String toJson() {
		StringBuilder sb = new StringBuilder(256 + bedrock.size() * 48);
		sb.append("{\n  \"format\": \"mc-locate-observations\",\n");
		sb.append("  \"version\": ").append(FORMAT_VERSION).append(",\n");
		sb.append("  \"source\": \"mc-locate-exporter\"");

		if (seed != null) {
			sb.append(",\n  \"seed\": ").append(seed.toString());
		}
		if (biomeHash != null) {
			sb.append(",\n  \"biome_hash\": ").append(biomeHash.toString());
		}
		if (minecraftVersion != null) {
			// Escaped even though the value comes from the game, not the user:
			// it is the only free-form string in the document.
			sb.append(",\n  \"minecraft_version\": \"").append(escape(minecraftVersion)).append('"');
		}
		appendArray(sb, "bedrock", bedrock);
		appendArray(sb, "slime", slime);
		appendArray(sb, "structures", structures);
		appendArray(sb, "eye_throws", eyeThrows);

		if (pillarHeights != null) {
			sb.append(",\n  \"pillar_heights\": [");
			for (int i = 0; i < pillarHeights.length; i++) {
				if (i > 0) {
					sb.append(", ");
				}
				sb.append(pillarHeights[i] == null ? "null" : pillarHeights[i].toString());
			}
			sb.append(']');
		}

		sb.append("\n}\n");
		return sb.toString();
	}

	private static void appendArray(StringBuilder sb, String name, List<String> items) {
		if (items.isEmpty()) {
			// Omit rather than write an empty array, so the file stays readable
			// by hand.
			return;
		}
		sb.append(",\n  \"").append(name).append("\": [\n");
		for (int i = 0; i < items.size(); i++) {
			sb.append("    ").append(items.get(i));
			if (i < items.size() - 1) {
				sb.append(',');
			}
			sb.append('\n');
		}
		sb.append("  ]");
	}

	private static String escape(String s) {
		return s.replace("\\", "\\\\").replace("\"", "\\\"");
	}

	public Path write(Path directory, String fileName) throws IOException {
		Files.createDirectories(directory);
		Path out = directory.resolve(fileName);
		Files.writeString(out, toJson());
		return out;
	}
}
