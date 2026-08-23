package dev.lao.mclocate.client;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

import net.minecraft.world.entity.projectile.EyeOfEnder;
import net.minecraft.world.phys.Vec3;

/**
 * Reads the bearing of a thrown eye of ender.
 *
 * <p>An eye flies at the nearest stronghold, so its direction of travel is the
 * bearing the triangulation in mc-locate wants. Taking it from the entity is
 * strictly better than reading the player's yaw off the F3 screen: the F3
 * figure is wherever the player happened to be looking when they threw, which
 * is only approximately at the eye, and it is quantised by mouse resolution.
 * The eye itself is exact.
 *
 * <p>The reading is deferred by a tick or two. On the client an eye arrives
 * from the server with its velocity not yet populated, so sampling at spawn
 * gives a direction of zero.
 */
public final class EyeTracker {
	/** A bearing measurement, in mc-locate's {@code Throw} shape. */
	public record Throw(double x, double z, double yaw) {
	}

	/** Horizontal speed, in blocks per tick, below which direction is noise. */
	private static final double MIN_SPEED = 0.05;

	/** Give up on an eye that never reports a usable velocity. */
	private static final int MAX_TICKS_WAITING = 40;

	/** Guards against a pathological world dumping entities into the list. */
	private static final int MAX_PENDING = 64;

	private final List<Pending> pending = new ArrayList<>();

	private static final class Pending {
		final EyeOfEnder eye;
		final double originX;
		final double originZ;
		int ticks;

		Pending(EyeOfEnder eye) {
			this.eye = eye;
			// The eye spawns at the thrower, so its initial position is the
			// point the bearing was taken from.
			this.originX = eye.getX();
			this.originZ = eye.getZ();
		}
	}

	public void watch(EyeOfEnder eye) {
		if (pending.size() < MAX_PENDING) {
			pending.add(new Pending(eye));
		}
	}

	public void forget() {
		pending.clear();
	}

	public int pendingCount() {
		return pending.size();
	}

	/**
	 * Advances every watched eye by one tick, returning any bearing that became
	 * readable this tick.
	 */
	public List<Throw> tick() {
		List<Throw> out = new ArrayList<>();

		for (Iterator<Pending> it = pending.iterator(); it.hasNext();) {
			Pending p = it.next();
			p.ticks++;

			if (!p.eye.isAlive() || p.ticks > MAX_TICKS_WAITING) {
				it.remove();
				continue;
			}

			Vec3 v = p.eye.getDeltaMovement();
			double speed = Math.sqrt(v.x * v.x + v.z * v.z);

			if (speed < MIN_SPEED) {
				continue;
			}
			out.add(new Throw(p.originX, p.originZ, yawOf(v.x, v.z)));
			it.remove();
		}
		return out;
	}

	/**
	 * Minecraft's yaw for a horizontal direction, in degrees on (-180, 180].
	 *
	 * <p>Yaw 0 faces +Z, and increasing yaw turns toward -X, so the facing
	 * vector is {@code (-sin y, cos y)}. Inverting that gives this atan2. The
	 * convention matches {@code bearing_to} in mc-locate's stronghold module.
	 */
	public static double yawOf(double dx, double dz) {
		double deg = Math.toDegrees(Math.atan2(-dx, dz));
		// atan2 returns [-180, 180]; fold the -180 endpoint up so the range has
		// exactly one representation of due north.
		return deg <= -180.0 ? deg + 360.0 : deg;
	}
}
