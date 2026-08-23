package dev.lao.mclocate.client;

import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Everything collected since the mod loaded, accumulated across dimensions and
 * world reloads until explicitly cleared.
 *
 * <p>Bedrock samples are keyed by position, so re-walking ground you have
 * already covered costs nothing. That matters more than it sounds: chunks
 * reload constantly as you move, and without the key a single afternoon would
 * produce a file full of the same few hundred blocks repeated. mc-locate would
 * still reach the right answer, but it would spend the whole time re-testing
 * observations that cannot eliminate anything new.
 */
public final class Session {
	/** Packed position to sample, preserving insertion order for stable files. */
	private final Map<Long, boolean[]> bedrock = new LinkedHashMap<>();
	private final List<EyeTracker.Throw> throws_ = new ArrayList<>();
	private final List<String[]> structures = new ArrayList<>();
	private final Map<Long, Boolean> slime = new LinkedHashMap<>();

	private Integer[] pillarHeights;
	private String minecraftVersion;
	private Long seed;

	/** Positions rejected because the session is already at its cap. */
	private int dropped;

	public synchronized boolean addBedrock(int x, int y, int z, boolean isBedrock, int cap) {
		Long key = pack(x, y, z);

		if (bedrock.containsKey(key)) {
			return false;
		}
		if (bedrock.size() >= cap) {
			dropped++;
			return false;
		}
		bedrock.put(key, new boolean[] { isBedrock });
		return true;
	}

	public synchronized void addSlime(int chunkX, int chunkZ, boolean isSlime) {
		slime.put((((long) chunkX) << 32) | (chunkZ & 0xFFFFFFFFL), isSlime);
	}

	public synchronized void addStructure(String type, int x, int z) {
		structures.add(new String[] { type, Integer.toString(x), Integer.toString(z) });
	}

	public synchronized void addThrow(EyeTracker.Throw t) {
		throws_.add(t);
	}

	public synchronized void setPillarHeights(Integer[] heights) {
		this.pillarHeights = heights;
	}

	public synchronized void setMinecraftVersion(String v) {
		this.minecraftVersion = v;
	}

	/** Records the ground-truth seed (singleplayer). Returns true if new. */
	public synchronized boolean setSeed(long value) {
		boolean isNew = seed == null || seed != value;
		seed = value;
		return isNew;
	}

	public synchronized boolean hasSeed() {
		return seed != null;
	}

	public synchronized int bedrockCount() {
		return bedrock.size();
	}

	public synchronized int slimeCount() {
		return slime.size();
	}

	public synchronized int throwCount() {
		return throws_.size();
	}

	public synchronized int structureCount() {
		return structures.size();
	}

	public synchronized int droppedCount() {
		return dropped;
	}

	public synchronized boolean hasPillars() {
		return pillarHeights != null;
	}

	public synchronized boolean isEmpty() {
		return bedrock.isEmpty() && slime.isEmpty() && structures.isEmpty()
				&& throws_.isEmpty() && pillarHeights == null && seed == null;
	}

	public synchronized void clear() {
		bedrock.clear();
		slime.clear();
		structures.clear();
		throws_.clear();
		pillarHeights = null;
		seed = null;
		dropped = 0;
	}

	/**
	 * How many 48-bit seeds a set of bedrock samples is expected to leave
	 * standing, as a rough progress signal.
	 *
	 * <p>Each sampled block is an independent 20%/80% draw, so it carries the
	 * binary entropy of that split — about 0.72 bits. This is the same figure
	 * mc-locate's advisor reports, repeated here so the in-game readout and the
	 * tool agree.
	 */
	public synchronized double expectedSurvivorsOf(double startingCandidates) {
		double bits = bedrock.size() * 0.7219280948873623;
		return startingCandidates / Math.pow(2.0, bits);
	}

	public synchronized Observations snapshot() {
		Observations obs = new Observations();

		if (minecraftVersion != null) {
			obs.setMinecraftVersion(minecraftVersion);
		}
		for (Map.Entry<Long, boolean[]> e : bedrock.entrySet()) {
			long k = e.getKey();
			obs.addBedrock(unpackX(k), unpackY(k), unpackZ(k), e.getValue()[0]);
		}
		for (Map.Entry<Long, Boolean> e : slime.entrySet()) {
			long k = e.getKey();
			obs.addSlimeChunk((int) (k >> 32), (int) k, e.getValue());
		}
		for (String[] s : structures) {
			obs.addStructure(s[0], Integer.parseInt(s[1]), Integer.parseInt(s[2]));
		}
		for (EyeTracker.Throw t : throws_) {
			obs.addEyeThrow(t.x(), t.z(), t.yaw());
		}
		if (pillarHeights != null) {
			obs.setPillarHeights(pillarHeights);
		}
		if (seed != null) {
			obs.setSeed(seed);
		}
		return obs;
	}

	public Path write(Path directory, String fileName) throws IOException {
		return snapshot().write(directory, fileName);
	}

	// Nether bedrock spans y 0..127 and the world is +/-30M blocks, so 26 bits
	// per horizontal axis and 8 for the vertical fit inside a long with room to
	// spare.
	private static long pack(int x, int y, int z) {
		return ((long) (x & 0x3FFFFFF) << 38) | ((long) (z & 0x3FFFFFF) << 12) | (y & 0xFFF);
	}

	private static int unpackX(long k) {
		return signExtend26((int) ((k >> 38) & 0x3FFFFFF));
	}

	private static int unpackZ(long k) {
		return signExtend26((int) ((k >> 12) & 0x3FFFFFF));
	}

	private static int unpackY(long k) {
		return (int) (k & 0xFFF);
	}

	private static int signExtend26(int v) {
		return (v << 6) >> 6;
	}
}
