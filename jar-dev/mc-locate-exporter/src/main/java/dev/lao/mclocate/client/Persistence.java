package dev.lao.mclocate.client;

import java.io.IOException;
import java.io.Reader;
import java.nio.file.Files;
import java.nio.file.Path;

import com.google.gson.Gson;
import com.google.gson.JsonParseException;

/**
 * Crash-safe session storage.
 *
 * <p>A collecting session is real effort — an afternoon of flying the Nether is
 * hundreds of bedrock samples — so it should not evaporate on a crash or a
 * forgotten {@code /mclocate export}. This keeps a single rolling file,
 * {@code session-current.json}, always holding the latest state, and reloads it
 * on startup.
 *
 * <p>Writing reuses {@link Observations}' emitter, so the rolling file is a
 * valid observation file in its own right. Reading uses Gson, which Minecraft
 * already ships, rather than a hand-written parser.
 */
public final class Persistence {
	private Persistence() {
	}

	public static final String CURRENT_FILE = "session-current.json";

	private static final Gson GSON = new Gson();

	/** Mirror of the observation schema, only for reading our own file back. */
	private static final class Doc {
		Long seed;
		String minecraft_version;
		Integer[] pillar_heights;
		Bedrock[] bedrock;
		Slime[] slime;
		Structure[] structures;
		Eye[] eye_throws;

		static final class Bedrock {
			int x;
			int y;
			int z;
			boolean is_bedrock;
		}

		static final class Slime {
			int chunk_x;
			int chunk_z;
			boolean is_slime;
		}

		static final class Structure {
			String type;
			int x;
			int z;
		}

		static final class Eye {
			double x;
			double z;
			double yaw;
		}
	}

	/** Writes the rolling file. Silent on failure — persistence is best-effort. */
	public static void save(Path directory, Session session) {
		if (session.isEmpty()) {
			return;
		}
		try {
			session.write(directory, CURRENT_FILE);
		} catch (IOException e) {
			// Nothing useful to do mid-game; the in-memory session is unaffected.
		}
	}

	/**
	 * Repopulates {@code session} from the rolling file if present.
	 *
	 * @return a short human summary, or null if there was nothing to load.
	 */
	public static String load(Path directory, Session session, int bedrockCap) {
		Path file = directory.resolve(CURRENT_FILE);
		if (!Files.isRegularFile(file)) {
			return null;
		}
		Doc doc;
		try (Reader r = Files.newBufferedReader(file)) {
			doc = GSON.fromJson(r, Doc.class);
		} catch (IOException | JsonParseException e) {
			// A corrupt rolling file must not stop the game loading.
			return null;
		}
		if (doc == null) {
			return null;
		}

		int bedrock = 0;
		int slime = 0;
		int throws_ = 0;
		int structures = 0;

		if (doc.seed != null) {
			session.setSeed(doc.seed);
		}
		if (doc.minecraft_version != null) {
			session.setMinecraftVersion(doc.minecraft_version);
		}
		if (doc.pillar_heights != null && doc.pillar_heights.length == 10) {
			session.setPillarHeights(doc.pillar_heights);
		}
		if (doc.bedrock != null) {
			for (Doc.Bedrock b : doc.bedrock) {
				if (session.addBedrock(b.x, b.y, b.z, b.is_bedrock, bedrockCap)) {
					bedrock++;
				}
			}
		}
		if (doc.slime != null) {
			for (Doc.Slime s : doc.slime) {
				session.addSlime(s.chunk_x, s.chunk_z, s.is_slime);
				slime++;
			}
		}
		if (doc.structures != null) {
			for (Doc.Structure s : doc.structures) {
				if (s.type != null) {
					session.addStructure(s.type, s.x, s.z);
					structures++;
				}
			}
		}
		if (doc.eye_throws != null) {
			for (Doc.Eye e : doc.eye_throws) {
				session.addThrow(new EyeTracker.Throw(e.x, e.z, e.yaw));
				throws_++;
			}
		}

		if (bedrock + slime + throws_ + structures == 0
				&& !session.hasSeed() && !session.hasPillars()) {
			return null;
		}
		return String.format(java.util.Locale.ROOT,
				"restored %d bedrock, %d slime, %d eye throw(s), %d structure(s)%s%s",
				bedrock, slime, throws_, structures,
				session.hasPillars() ? ", pillars" : "",
				session.hasSeed() ? ", seed" : "");
	}
}
