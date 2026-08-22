//! Mode 5 — camera pose from a screenshot plus a handful of hand-tagged block
//! corners.
//!
//! The user finds a few block corners they can identify in a screenshot, types
//! the pixel each one lands on and the world coordinate it belongs to, and this
//! recovers where the camera was and which way it was pointing. The yaw that
//! falls out is exactly the quantity mode 8 wants as a starting heading, which
//! is the main reason the mode exists: "I was facing roughly north-east" is
//! worth very little to a triangulation, whereas "yaw 312.4° with a 1.8 px RMS
//! fit" is worth a great deal.
//!
//! Two conventions have to be exactly right or everything downstream is
//! quietly wrong:
//!
//! * **Minecraft yaw runs clockwise seen from above**, opposite to the
//!   mathematical convention. Yaw 0 faces +Z (south), 90 faces -X (west), 180
//!   faces -Z (north), 270 (= -90) faces +X (east); the facing vector is
//!   `dx = -sin(yaw)`, `dz = cos(yaw)`. Pitch is -90 straight up and +90
//!   straight down — again the opposite sign to the usual "elevation". This is
//!   what the F3 `Facing:` line reports and what the `Entity` rotation fields
//!   hold. A sign error here does not crash anything; it silently sends mode 8
//!   looking in the wrong quadrant, so [`yaw_to_vector`] and
//!   [`yaw_to_cardinal`] are pinned by tests.
//! * **The FOV option is the *vertical* field of view.** Minecraft feeds the
//!   video-settings number straight into a `perspective(fovy, aspect, ..)`
//!   projection, so the default 70 is 70 degrees measured top-to-bottom of the
//!   framebuffer, not corner-to-corner and not horizontal. The far end of the
//!   slider, labelled "Quake Pro", is 110. Building the intrinsics from the
//!   image *width* instead would put the focal length out by the aspect ratio
//!   and bias every angle.
//!
//! The solver is a textbook PnP and deliberately nothing cleverer: a DLT for an
//! initial guess, then Levenberg–Marquardt on the reprojection error. The only
//! Minecraft-specific piece of geometry it exploits is that a Minecraft camera
//! never rolls — used both as a fallback initialiser (a coarse sweep over the
//! two angles that actually exist) and, afterwards, as a free sanity check: a
//! solve that comes back with several degrees of roll has bad tags in it.

use anyhow::{Result, anyhow, bail};
use nalgebra::{DMatrix, DVector, Matrix3, Matrix3x4, Matrix4, Vector3};

use crate::session::Session;
use crate::ui;

/// Minecraft's default FOV slider position, in degrees of *vertical* FOV.
pub const DEFAULT_FOV_DEG: f64 = 70.0;
/// The far end of the FOV slider, labelled "Quake Pro" in the video settings.
pub const QUAKE_PRO_FOV_DEG: f64 = 110.0;
/// Eye height of a standing player in Java Edition, in blocks. Only used as a
/// sanity note when reporting the solved camera Y: a first-person screenshot's
/// camera should sit this far above whatever the player was standing on.
pub const PLAYER_EYE_HEIGHT: f64 = 1.62;

/// A PnP solve needs six degrees of freedom out of two equations per point, so
/// three points is the theoretical floor — and three has up to four solutions.
/// Four is the smallest number that is usually unambiguous, so it is the floor
/// here.
pub const MIN_CORRESPONDENCES: usize = 4;
/// The linear DLT solves for a 12-vector up to scale from two rows per point,
/// so it needs six points before the design matrix even has a one-dimensional
/// null space. Below this the coarse-sweep initialiser is used instead.
pub const MIN_FOR_DLT: usize = 6;
/// Solving for the focal length adds a seventh unknown; the brief asks for six
/// correspondences before offering it, which also keeps the problem
/// comfortably over-determined rather than merely determined.
pub const MIN_FOR_FOCAL: usize = 6;

/// One tagged point: a pixel in the screenshot and the world-space block corner
/// it is the image of.
///
/// `u` is measured rightwards from the left edge, `v` downwards from the top
/// edge — the usual image convention, and the one screenshot tooling reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correspondence {
    pub u: f64,
    pub v: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// The answer, in the terms Minecraft uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Camera position in world coordinates. For a first-person screenshot
    /// this is the *eye*, roughly [`PLAYER_EYE_HEIGHT`] above the feet.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Minecraft yaw in degrees, normalised to `[0, 360)`.
    pub yaw_deg: f64,
    /// Minecraft pitch in degrees: negative is up, positive is down.
    pub pitch_deg: f64,
    /// Root-mean-square reprojection error over the tagged points, in pixels,
    /// defined per point as `sqrt(mean(du² + dv²))`.
    pub rms_px: f64,
}

/// Everything the solve knows, for callers that want more than the pose.
#[derive(Debug, Clone)]
pub struct Solution {
    pub pose: Pose,
    /// Rotation about the view axis, in degrees. A real Minecraft camera cannot
    /// roll, so anything much above a degree means at least one tag is wrong.
    pub roll_deg: f64,
    /// Focal length actually used, in pixels (equal in x and y).
    pub focal_px: f64,
    /// The vertical FOV corresponding to [`Solution::focal_px`]; differs from
    /// the FOV that was fed in only when the focal length was solved for.
    pub vfov_deg: f64,
    /// Whether the focal length was a free parameter in the refinement.
    pub focal_was_solved: bool,
    /// Levenberg–Marquardt iterations actually taken.
    pub iterations: usize,
    /// Per-correspondence reprojection error in pixels, in input order.
    pub residuals_px: Vec<f64>,
    /// Per-correspondence straight-line distance from the camera, in blocks.
    pub distances: Vec<f64>,
    /// How the initial guess was obtained, for the report.
    pub init: &'static str,
}

// ---------------------------------------------------------------------------
// Conventions
// ---------------------------------------------------------------------------

/// The horizontal facing vector `(dx, dz)` for a Minecraft yaw.
///
/// Yaw 0 → `(0, 1)` = +Z = south, 90 → `(-1, 0)` = -X = west, 180 → `(0, -1)` =
/// -Z = north, 270 → `(1, 0)` = +X = east.
pub fn yaw_to_vector(yaw_deg: f64) -> (f64, f64) {
    let r = yaw_deg.to_radians();
    (-r.sin(), r.cos())
}

/// The nearest cardinal to a yaw, named both as a signed axis and as the
/// compass word Minecraft's F3 screen uses for it.
pub fn yaw_to_cardinal(yaw_deg: f64) -> &'static str {
    // Bucket into 90° bins centred on each cardinal; ties (exact 45° offsets)
    // round up, which only matters for output text.
    let quadrant = (normalise_yaw(yaw_deg + 45.0) / 90.0).floor() as i64 % 4;
    match quadrant {
        0 => "+Z (south)",
        1 => "-X (west)",
        2 => "-Z (north)",
        _ => "+X (east)",
    }
}

/// Wraps a yaw into `[0, 360)`, which is how [`Session::heading`] stores it.
pub fn normalise_yaw(yaw_deg: f64) -> f64 {
    let w = yaw_deg % 360.0;
    if w < 0.0 { w + 360.0 } else { w }
}

/// The same yaw in the `(-180, 180]` range that F3 prints, so the user can
/// compare the estimate against a remembered debug screen directly.
pub fn yaw_f3_style(yaw_deg: f64) -> f64 {
    let w = normalise_yaw(yaw_deg);
    if w > 180.0 { w - 360.0 } else { w }
}

/// Intrinsic matrix from the image size and Minecraft's vertical FOV.
///
/// `fy = (height/2) / tan(vfov/2)`, `fx = fy` because Minecraft's pixels are
/// square, and the principal point is the image centre. Half-pixel conventions
/// (whether the centre is `h/2` or `(h-1)/2`) shift the answer by at most a
/// twentieth of a degree at typical resolutions, well under the noise in a
/// hand-tagged pixel, so the simpler `h/2` is used throughout — including when
/// projecting, so the two always agree.
pub fn intrinsics(width: u32, height: u32, vfov_deg: f64) -> Matrix3<f64> {
    let f = focal_from_vfov(height, vfov_deg);
    Matrix3::new(
        f,
        0.0,
        width as f64 / 2.0,
        0.0,
        f,
        height as f64 / 2.0,
        0.0,
        0.0,
        1.0,
    )
}

/// Focal length in pixels for a given image height and vertical FOV.
pub fn focal_from_vfov(height: u32, vfov_deg: f64) -> f64 {
    (height as f64 / 2.0) / (vfov_deg.to_radians() / 2.0).tan()
}

/// Inverse of [`focal_from_vfov`], used to report a solved-for FOV in the units
/// the user's video settings are in.
pub fn vfov_from_focal(height: u32, focal_px: f64) -> f64 {
    (2.0 * ((height as f64 / 2.0) / focal_px).atan()).to_degrees()
}

/// World→camera rotation for a Minecraft yaw and pitch, with no roll.
///
/// The camera frame is the usual computer-vision one: x right across the
/// image, y *down* the image, z along the view direction. Its rows are
/// therefore the camera's right, down and forward axes expressed in world
/// coordinates:
///
/// * forward `= (-sin y·cos p, -sin p, cos y·cos p)` — the yaw convention above
///   with pitch folded in, and `-sin p` because positive pitch looks down.
/// * right `= (-cos y, 0, -sin y)` — facing south (+Z) your right hand points
///   west (-X), which is what this gives at yaw 0.
/// * down `= forward × right`.
pub fn rotation_from_yaw_pitch(yaw_deg: f64, pitch_deg: f64) -> Matrix3<f64> {
    let (sy, cy) = yaw_deg.to_radians().sin_cos();
    let (sp, cp) = pitch_deg.to_radians().sin_cos();
    Matrix3::new(
        -cy,
        0.0,
        -sy,
        sp * sy,
        -cp,
        -sp * cy,
        -sy * cp,
        -sp,
        cy * cp,
    )
}

/// Recovers `(yaw, pitch, roll)` in degrees from a world→camera rotation.
///
/// The third row is the forward axis whatever the roll, so yaw and pitch come
/// straight out of it; roll is then how far the actual right axis has turned
/// away from the level right axis that a Minecraft camera would have had.
pub fn yaw_pitch_roll_from_rotation(r: &Matrix3<f64>) -> (f64, f64, f64) {
    let forward = Vector3::new(r[(2, 0)], r[(2, 1)], r[(2, 2)]);
    let yaw = (-forward.x).atan2(forward.z).to_degrees();
    let pitch = (-forward.y).clamp(-1.0, 1.0).asin().to_degrees();

    let level = rotation_from_yaw_pitch(yaw, pitch);
    let right = Vector3::new(r[(0, 0)], r[(0, 1)], r[(0, 2)]);
    let level_right = Vector3::new(level[(0, 0)], level[(0, 1)], level[(0, 2)]);
    let level_down = Vector3::new(level[(1, 0)], level[(1, 1)], level[(1, 2)]);
    let roll = right.dot(&level_down).atan2(right.dot(&level_right)).to_degrees();

    (normalise_yaw(yaw), pitch, roll)
}

/// Translation vector for a camera at `position` with rotation `r`, i.e. the
/// `t` in `p_camera = R·p_world + t`.
pub fn translation_from_position(r: &Matrix3<f64>, position: &Vector3<f64>) -> Vector3<f64> {
    -(r * position)
}

/// Projects a world point to a pixel. Returns `None` when the point is behind
/// the camera, where the projection is meaningless rather than merely large.
pub fn project_point(
    k: &Matrix3<f64>,
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    p: &Vector3<f64>,
) -> Option<(f64, f64)> {
    let pc = r * p + t;
    if pc.z <= 1e-9 {
        return None;
    }
    Some((
        k[(0, 0)] * pc.x / pc.z + k[(0, 2)],
        k[(1, 1)] * pc.y / pc.z + k[(1, 2)],
    ))
}

// ---------------------------------------------------------------------------
// The solve
// ---------------------------------------------------------------------------

/// Estimates the camera pose from tagged correspondences with a known
/// intrinsic matrix.
pub fn solve_pose(points: &[Correspondence], k: &Matrix3<f64>) -> Result<Pose> {
    solve_pose_full(points, k, false).map(|s| s.pose)
}

/// [`solve_pose`] with diagnostics, and the option to treat the focal length
/// (and therefore the FOV) as a seventh unknown.
pub fn solve_pose_full(
    points: &[Correspondence],
    k: &Matrix3<f64>,
    solve_focal: bool,
) -> Result<Solution> {
    let n = points.len();
    if n < MIN_CORRESPONDENCES {
        bail!(
            "a PnP solve needs at least {MIN_CORRESPONDENCES} correspondences, but only {n} \
             {} given",
            if n == 1 { "was" } else { "were" }
        );
    }
    if solve_focal && n < MIN_FOR_FOCAL {
        bail!(
            "solving for the FOV as well needs at least {MIN_FOR_FOCAL} correspondences, but \
             only {n} were given"
        );
    }

    let f = k[(0, 0)];
    let cx = k[(0, 2)];
    let cy = k[(1, 2)];
    if !(f.is_finite() && f > 0.0) {
        bail!("the intrinsic matrix has a non-positive focal length ({f})");
    }

    check_world_geometry(points)?;
    check_image_geometry(points)?;

    // Two independent initialisers. The DLT is the principled one; the coarse
    // sweep exists because it needs six points and because it collapses on
    // configurations the sweep handles happily. Refining both and keeping the
    // better answer costs microseconds and removes a whole class of "the DLT
    // happened to be badly conditioned today" failures.
    let mut starts: Vec<(&'static str, Matrix3<f64>, Vector3<f64>)> = Vec::new();
    let mut dlt_error = None;
    if n >= MIN_FOR_DLT {
        match dlt_initial(points, f, cx, cy) {
            Ok((r, t)) => starts.push(("DLT", r, t)),
            Err(e) => dlt_error = Some(e),
        }
    }
    if let Some((r, t)) = coarse_orientation_sweep(points, f, cx, cy) {
        starts.push(("coarse yaw/pitch sweep", r, t));
    }

    if starts.is_empty() {
        return Err(match dlt_error {
            Some(e) => e.context(
                "no usable starting pose: the linear solve failed and no orientation put every \
                 tagged point in front of the camera",
            ),
            None => anyhow!(
                "no starting pose puts every tagged point in front of the camera — check that \
                 the pixels and the world coordinates are paired up in the same order"
            ),
        });
    }

    let mut best: Option<Solution> = None;
    for (label, r0, t0) in starts {
        if let Some(sol) = refine(points, r0, t0, f, cx, cy, solve_focal, label)
            && best.as_ref().is_none_or(|b| sol.pose.rms_px < b.pose.rms_px)
        {
            best = Some(sol);
        }
    }

    let best = best.ok_or_else(|| {
        anyhow!(
            "the refinement did not converge to a pose with every point in front of the \
             camera; the correspondences are probably mismatched"
        )
    })?;

    if !best.pose.rms_px.is_finite() {
        bail!("the solve produced a non-finite reprojection error");
    }
    Ok(best)
}

/// Rejects tag sets whose *world* points cannot pin a camera down, whatever the
/// pixels say.
///
/// Coincident, collinear and coplanar sets each kill the solve in a different
/// way, and all three are easy to produce by accident — tagging four corners of
/// one block face gives a coplanar set. Bailing here with an explanation is far
/// better than returning a number: a coplanar set still *has* a best-fit pose,
/// it just is not determined by the data, and printing it would look exactly
/// like a good answer.
fn check_world_geometry(points: &[Correspondence]) -> Result<()> {
    let n = points.len() as f64;
    let centroid = points.iter().fold(Vector3::zeros(), |acc: Vector3<f64>, p| {
        acc + Vector3::new(p.x, p.y, p.z)
    }) / n;

    let mut m = DMatrix::zeros(3, points.len());
    for (i, p) in points.iter().enumerate() {
        m[(0, i)] = p.x - centroid.x;
        m[(1, i)] = p.y - centroid.y;
        m[(2, i)] = p.z - centroid.z;
    }
    let sv = m.svd(false, false).singular_values;
    let (s0, s1, s2) = (sv[0], sv[1], sv[2]);

    if s0 < 1e-9 {
        bail!("all the tagged world points are the same point, so they constrain nothing");
    }
    // A relative threshold, because block coordinates can be in the tens of
    // thousands. Genuinely non-degenerate integer block corners sit orders of
    // magnitude above this; exactly degenerate ones land at rounding noise.
    if s1 < 1e-6 * s0 {
        bail!(
            "the tagged world points are collinear — every rotation about that line reprojects \
             them identically, so the pose is not determined. Tag corners that are not all on \
             one straight edge."
        );
    }
    if s2 < 1e-6 * s0 {
        bail!(
            "the tagged world points are all coplanar, which makes the pose solve singular. \
             This usually means every corner came off one flat wall or floor — add at least one \
             corner at a clearly different depth (a block nearer the camera, or on a surface \
             facing a different way)."
        );
    }
    Ok(())
}

/// Rejects tag sets whose *pixels* are degenerate: all in one spot, or all
/// along one image line. Either way there is no perspective information left.
fn check_image_geometry(points: &[Correspondence]) -> Result<()> {
    let n = points.len() as f64;
    let (mu, mv) = points.iter().fold((0.0, 0.0), |(a, b), p| (a + p.u, b + p.v));
    let (mu, mv) = (mu / n, mv / n);

    let mut m = DMatrix::zeros(2, points.len());
    for (i, p) in points.iter().enumerate() {
        m[(0, i)] = p.u - mu;
        m[(1, i)] = p.v - mv;
    }
    let sv = m.svd(false, false).singular_values;
    if sv[0] < 1.0 {
        bail!("the tagged pixels are all within a pixel of each other, so they say nothing");
    }
    if sv[1] < 1e-3 * sv[0] {
        bail!(
            "the tagged pixels all lie on one straight line in the image, which cannot pin down \
             a pose. Spread the tags around the frame."
        );
    }
    Ok(())
}

/// Direct Linear Transform on image coordinates that have already been divided
/// through by the known intrinsics, so the 3×4 matrix it recovers is `[R|t]`
/// up to scale rather than a full projection matrix.
///
/// Both point sets get Hartley-normalised first (centroid at the origin, mean
/// distance `sqrt(2)` and `sqrt(3)` respectively). Without that, world
/// coordinates in the thousands and image coordinates around one put wildly
/// different scales into the same design matrix and the smallest singular
/// vector is dominated by rounding.
fn dlt_initial(
    points: &[Correspondence],
    f: f64,
    cx: f64,
    cy: f64,
) -> Result<(Matrix3<f64>, Vector3<f64>)> {
    let n = points.len();
    debug_assert!(n >= MIN_FOR_DLT);

    let image: Vec<(f64, f64)> = points
        .iter()
        .map(|p| ((p.u - cx) / f, (p.v - cy) / f))
        .collect();
    let world: Vec<Vector3<f64>> = points
        .iter()
        .map(|p| Vector3::new(p.x, p.y, p.z))
        .collect();

    let (ti, image_n) = hartley_2d(&image);
    let (tw, world_n) = hartley_3d(&world);

    let mut a = DMatrix::zeros(2 * n, 12);
    for i in 0..n {
        let (u, v) = image_n[i];
        let w = world_n[i];
        let xh = [w.x, w.y, w.z, 1.0];
        for (j, &c) in xh.iter().enumerate() {
            // Rows of x × (M X) = 0, dropping the linearly dependent third.
            a[(2 * i, 4 + j)] = -c;
            a[(2 * i, 8 + j)] = v * c;
            a[(2 * i + 1, j)] = c;
            a[(2 * i + 1, 8 + j)] = -u * c;
        }
    }

    let svd = a.svd(true, true);
    let sv = &svd.singular_values;
    // `SVD::new` sorts descending, and with 2n ≥ 12 rows there are 12 of them.
    let smallest = sv[sv.len() - 1];
    let second = sv[sv.len() - 2];
    if second < 1e-12 * sv[0] {
        bail!(
            "the linear pose solve is rank-deficient (singular values collapse to \
             {second:.3e} against {:.3e}); the correspondences do not constrain a unique camera",
            sv[0]
        );
    }
    // A clean solve leaves one singular value far below the rest. When the last
    // two are comparable the null space is effectively two-dimensional and any
    // vector picked from it is arbitrary — which is precisely the "confident
    // wrong answer" case worth refusing.
    if smallest > 0.2 * second {
        bail!(
            "the linear pose solve has no clear null space (smallest singular values \
             {smallest:.3e} and {second:.3e} are the same order), so the correspondences are \
             close to degenerate — most often they are nearly coplanar"
        );
    }

    let vt = svd
        .v_t
        .ok_or_else(|| anyhow!("the SVD did not return right singular vectors"))?;
    let row = vt.row(vt.nrows() - 1);
    // The null vector is a row-major flattening of the 3x4 projection matrix,
    // whereas `from_iterator` would fill column-major. `Matrix3x4::new` takes
    // its arguments row-major, so spell the entries out rather than
    // transposing a wrongly-shaped matrix afterwards.
    let n: Vec<f64> = row.iter().copied().collect();
    let m_n = Matrix3x4::new(
        n[0], n[1], n[2], n[3],
        n[4], n[5], n[6], n[7],
        n[8], n[9], n[10], n[11],
    );

    let ti_inv = ti
        .try_inverse()
        .ok_or_else(|| anyhow!("image normalisation was not invertible"))?;
    let m = ti_inv * m_n * tw;

    let mut r_tilde = Matrix3::zeros();
    for i in 0..3 {
        for j in 0..3 {
            r_tilde[(i, j)] = m[(i, j)];
        }
    }
    let t_tilde = Vector3::new(m[(0, 3)], m[(1, 3)], m[(2, 3)]);

    // The null vector is only defined up to sign; the sign that makes the
    // rotation right-handed is the physical one.
    let sign = if r_tilde.determinant() < 0.0 { -1.0 } else { 1.0 };
    let (r_tilde, t_tilde) = (r_tilde * sign, t_tilde * sign);

    let svd_r = r_tilde.svd(true, true);
    let scale = svd_r.singular_values.iter().sum::<f64>() / 3.0;
    if !(scale.is_finite() && scale > 1e-12) {
        bail!("the linear solve produced a degenerate rotation block");
    }
    let u = svd_r
        .u
        .ok_or_else(|| anyhow!("the SVD did not return left singular vectors"))?;
    let v_t = svd_r
        .v_t
        .ok_or_else(|| anyhow!("the SVD did not return right singular vectors"))?;
    // Nearest true rotation to the noisy block, in the Frobenius sense.
    let mut r = u * v_t;
    if r.determinant() < 0.0 {
        let mut flip = Matrix3::identity();
        flip[(2, 2)] = -1.0;
        r = u * flip * v_t;
    }
    let t = t_tilde / scale;

    if points
        .iter()
        .any(|p| (r * Vector3::new(p.x, p.y, p.z) + t).z <= 1e-6)
    {
        bail!("the linear solve puts tagged points behind the camera");
    }
    Ok((r, t))
}

/// Hartley normalisation of 2-D points: returns `T` such that `T·x` has its
/// centroid at the origin and mean distance `sqrt(2)`, plus the transformed
/// points.
fn hartley_2d(pts: &[(f64, f64)]) -> (Matrix3<f64>, Vec<(f64, f64)>) {
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
    let (mx, my) = (mx / n, my / n);
    let mean_d = pts
        .iter()
        .map(|p| ((p.0 - mx).powi(2) + (p.1 - my).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    let s = if mean_d > 1e-12 { 2f64.sqrt() / mean_d } else { 1.0 };
    let t = Matrix3::new(s, 0.0, -s * mx, 0.0, s, -s * my, 0.0, 0.0, 1.0);
    let out = pts
        .iter()
        .map(|p| (s * (p.0 - mx), s * (p.1 - my)))
        .collect();
    (t, out)
}

/// Hartley normalisation of 3-D points, to mean distance `sqrt(3)`.
fn hartley_3d(pts: &[Vector3<f64>]) -> (Matrix4<f64>, Vec<Vector3<f64>>) {
    let n = pts.len() as f64;
    let m = pts.iter().fold(Vector3::zeros(), |a: Vector3<f64>, p| a + p) / n;
    let mean_d = pts.iter().map(|p| (p - m).norm()).sum::<f64>() / n;
    let s = if mean_d > 1e-12 { 3f64.sqrt() / mean_d } else { 1.0 };
    let mut t = Matrix4::identity();
    for i in 0..3 {
        t[(i, i)] = s;
        t[(i, 3)] = -s * m[i];
    }
    let out = pts.iter().map(|p| (p - m) * s).collect();
    (t, out)
}

/// Fallback initialiser: sweep the only two angles a Minecraft camera has, and
/// for each one solve the translation in closed form.
///
/// With the rotation fixed the collinearity constraints are linear in `t`, so
/// each sample is a 3×3 least-squares solve. A 5° grid over yaw and pitch is
/// ~2600 of those, which is instant, and it lands close enough for
/// Levenberg–Marquardt to finish the job. Unlike the DLT this works with four
/// points and does not care about conditioning.
fn coarse_orientation_sweep(
    points: &[Correspondence],
    f: f64,
    cx: f64,
    cy: f64,
) -> Option<(Matrix3<f64>, Vector3<f64>)> {
    const STEP_DEG: f64 = 5.0;
    let mut best: Option<(f64, Matrix3<f64>, Vector3<f64>)> = None;

    let mut yaw = 0.0;
    while yaw < 360.0 {
        let mut pitch = -85.0;
        while pitch <= 85.0 {
            let r = rotation_from_yaw_pitch(yaw, pitch);
            if let Some(t) = translation_for_rotation(points, &r, f, cx, cy)
                && let Some(err) = rms_reprojection(points, &r, &t, f, cx, cy)
                && best.as_ref().is_none_or(|(b, _, _)| err < *b)
            {
                best = Some((err, r, t));
            }
            pitch += STEP_DEG;
        }
        yaw += STEP_DEG;
    }
    best.map(|(_, r, t)| (r, t))
}

/// Least-squares translation for a known rotation.
///
/// Each point contributes `(R·P + t)_x - u_n·(R·P + t)_z = 0` and the same in
/// y, both linear in `t`. This minimises the algebraic residual, not the
/// reprojection error, which is fine for something whose only job is to seed
/// the refinement.
fn translation_for_rotation(
    points: &[Correspondence],
    r: &Matrix3<f64>,
    f: f64,
    cx: f64,
    cy: f64,
) -> Option<Vector3<f64>> {
    let mut ata = Matrix3::zeros();
    let mut atb = Vector3::zeros();
    for p in points {
        let rp = r * Vector3::new(p.x, p.y, p.z);
        let un = (p.u - cx) / f;
        let vn = (p.v - cy) / f;
        for (row, rhs) in [
            (Vector3::new(1.0, 0.0, -un), -(rp.x - un * rp.z)),
            (Vector3::new(0.0, 1.0, -vn), -(rp.y - vn * rp.z)),
        ] {
            ata += row * row.transpose();
            atb += row * rhs;
        }
    }
    ata.lu().solve(&atb).filter(|t| t.iter().all(|c| c.is_finite()))
}

/// RMS reprojection error in pixels, or `None` if any point is behind the
/// camera (in which case the pose is not a candidate at all).
fn rms_reprojection(
    points: &[Correspondence],
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    f: f64,
    cx: f64,
    cy: f64,
) -> Option<f64> {
    let mut sum = 0.0;
    for p in points {
        let pc = r * Vector3::new(p.x, p.y, p.z) + t;
        if !(pc.z > 1e-6) {
            return None;
        }
        let du = f * pc.x / pc.z + cx - p.u;
        let dv = f * pc.y / pc.z + cy - p.v;
        sum += du * du + dv * dv;
    }
    let rms = (sum / points.len() as f64).sqrt();
    rms.is_finite().then_some(rms)
}

/// Levenberg–Marquardt on the reprojection error over the six pose parameters,
/// plus the focal length when it is being solved for.
///
/// The rotation increment is applied on the left as a Rodrigues vector, so the
/// update is `R ← ΔR·R`, `t ← ΔR·t + δt` and the camera-frame point derivative
/// is simply `∂p_c/∂ω = -[p_c]ₓ`. Iterations are capped: this runs behind an
/// interactive prompt and a solve that has not settled in a few dozen steps is
/// not going to.
#[allow(clippy::too_many_arguments)]
fn refine(
    points: &[Correspondence],
    r0: Matrix3<f64>,
    t0: Vector3<f64>,
    f0: f64,
    cx: f64,
    cy: f64,
    solve_focal: bool,
    init: &'static str,
) -> Option<Solution> {
    const MAX_ITERS: usize = 80;

    let n = points.len();
    let np = if solve_focal { 7 } else { 6 };
    let mut r = r0;
    let mut t = t0;
    let mut f = f0;
    let mut lambda = 1e-3;
    let mut cost = sum_squares(points, &r, &t, f, cx, cy)?;
    let mut iterations = 0;

    for _ in 0..MAX_ITERS {
        let mut j = DMatrix::zeros(2 * n, np);
        let mut res = DVector::zeros(2 * n);
        for (i, p) in points.iter().enumerate() {
            let pc = r * Vector3::new(p.x, p.y, p.z) + t;
            let z = pc.z;
            if !(z > 1e-6) {
                return None;
            }
            res[2 * i] = f * pc.x / z + cx - p.u;
            res[2 * i + 1] = f * pc.y / z + cy - p.v;

            // d(u,v)/d(p_c)
            let du = [f / z, 0.0, -f * pc.x / (z * z)];
            let dv = [0.0, f / z, -f * pc.y / (z * z)];
            // d(p_c)/dω = -[p_c]ₓ
            let sk = skew(&pc);
            for c in 0..3 {
                let mut a = 0.0;
                let mut b = 0.0;
                for e in 0..3 {
                    a -= du[e] * sk[(e, c)];
                    b -= dv[e] * sk[(e, c)];
                }
                j[(2 * i, c)] = a;
                j[(2 * i + 1, c)] = b;
                j[(2 * i, 3 + c)] = du[c];
                j[(2 * i + 1, 3 + c)] = dv[c];
            }
            if solve_focal {
                j[(2 * i, 6)] = pc.x / z;
                j[(2 * i + 1, 6)] = pc.y / z;
            }
        }

        let jtj = j.transpose() * &j;
        let jtr = j.transpose() * &res;

        let mut stepped = false;
        for _ in 0..8 {
            let mut damped = jtj.clone();
            for d in 0..np {
                // Marquardt's scaling: damp each parameter in proportion to its
                // own curvature, so radians and blocks are damped comparably.
                damped[(d, d)] += lambda * jtj[(d, d)].max(1e-12);
            }
            let Some(delta) = damped.lu().solve(&(-&jtr)) else {
                lambda *= 10.0;
                continue;
            };
            if delta.iter().any(|v| !v.is_finite()) {
                lambda *= 10.0;
                continue;
            }

            let dr = rodrigues(&Vector3::new(delta[0], delta[1], delta[2]));
            let r_try = dr * r;
            let t_try = dr * t + Vector3::new(delta[3], delta[4], delta[5]);
            let f_try = if solve_focal { f + delta[6] } else { f };
            if !(f_try.is_finite() && f_try > 1.0) {
                lambda *= 10.0;
                continue;
            }

            match sum_squares(points, &r_try, &t_try, f_try, cx, cy) {
                Some(c) if c < cost => {
                    r = r_try;
                    t = t_try;
                    f = f_try;
                    cost = c;
                    lambda = (lambda * 0.1).max(1e-12);
                    stepped = true;
                    break;
                }
                _ => lambda *= 10.0,
            }
        }

        iterations += 1;
        if !stepped {
            break;
        }
        // Converged once the fit is at the level of floating point noise on a
        // pixel; nothing hand-tagged is ever that good, so this only fires on
        // synthetic data.
        if (cost / (2.0 * n as f64)).sqrt() < 1e-12 {
            break;
        }
    }

    // Re-orthonormalise once at the end: eighty small Rodrigues products drift
    // a rotation matrix by ~1e-15, which is harmless but makes the extracted
    // angles fractionally inconsistent.
    r = orthonormalise(&r)?;

    let mut residuals = Vec::with_capacity(n);
    let mut distances = Vec::with_capacity(n);
    let centre = -(r.transpose() * t);
    for p in points {
        let pw = Vector3::new(p.x, p.y, p.z);
        let pc = r * pw + t;
        if !(pc.z > 1e-6) {
            return None;
        }
        let du = f * pc.x / pc.z + cx - p.u;
        let dv = f * pc.y / pc.z + cy - p.v;
        residuals.push((du * du + dv * dv).sqrt());
        distances.push((pw - centre).norm());
    }
    let rms = (residuals.iter().map(|e| e * e).sum::<f64>() / n as f64).sqrt();
    if !rms.is_finite() {
        return None;
    }

    let (yaw, pitch, roll) = yaw_pitch_roll_from_rotation(&r);
    // `vfov_from_focal` wants an image height; the principal point is at h/2 by
    // construction in `intrinsics`, so recover it from cy rather than plumbing
    // the size through.
    let height = (cy * 2.0).round().max(1.0) as u32;

    Some(Solution {
        pose: Pose {
            x: centre.x,
            y: centre.y,
            z: centre.z,
            yaw_deg: yaw,
            pitch_deg: pitch,
            rms_px: rms,
        },
        roll_deg: roll,
        focal_px: f,
        vfov_deg: vfov_from_focal(height, f),
        focal_was_solved: solve_focal,
        iterations,
        residuals_px: residuals,
        distances,
        init,
    })
}

fn sum_squares(
    points: &[Correspondence],
    r: &Matrix3<f64>,
    t: &Vector3<f64>,
    f: f64,
    cx: f64,
    cy: f64,
) -> Option<f64> {
    let mut sum = 0.0;
    for p in points {
        let pc = r * Vector3::new(p.x, p.y, p.z) + t;
        if !(pc.z > 1e-6) {
            return None;
        }
        let du = f * pc.x / pc.z + cx - p.u;
        let dv = f * pc.y / pc.z + cy - p.v;
        sum += du * du + dv * dv;
    }
    sum.is_finite().then_some(sum)
}

fn skew(v: &Vector3<f64>) -> Matrix3<f64> {
    Matrix3::new(0.0, -v.z, v.y, v.z, 0.0, -v.x, -v.y, v.x, 0.0)
}

/// Rodrigues' rotation formula: axis-angle 3-vector to rotation matrix.
fn rodrigues(w: &Vector3<f64>) -> Matrix3<f64> {
    let theta = w.norm();
    if theta < 1e-14 {
        return Matrix3::identity();
    }
    let k = skew(&(w / theta));
    Matrix3::identity() + k * theta.sin() + k * k * (1.0 - theta.cos())
}

/// Nearest orthonormal, right-handed matrix, via SVD.
fn orthonormalise(m: &Matrix3<f64>) -> Option<Matrix3<f64>> {
    let svd = m.svd(true, true);
    let u = svd.u?;
    let v_t = svd.v_t?;
    let mut r = u * v_t;
    if r.determinant() < 0.0 {
        let mut flip = Matrix3::identity();
        flip[(2, 2)] = -1.0;
        r = u * flip * v_t;
    }
    Some(r)
}

// ---------------------------------------------------------------------------
// Optional edge-detection assist
// ---------------------------------------------------------------------------

/// A straight edge in Hesse normal form, `u·cos θ + v·sin θ = r`, matching the
/// parameterisation `imageproc::hough` returns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeLine {
    pub r: f64,
    pub theta_deg: f64,
}

/// Intersections of transverse pairs of detected edges, within `radius` pixels
/// of `(near_u, near_v)`, nearest first.
///
/// A block corner in a screenshot is where two block edges meet, so the
/// intersections of Hough lines are exactly the corner candidates. Pairs that
/// meet at a shallow angle are dropped: their intersection slides a long way
/// for a small change in either line, so it is not a reliable thing to snap to.
pub fn corner_candidates(
    lines: &[EdgeLine],
    near_u: f64,
    near_v: f64,
    radius: f64,
) -> Vec<(f64, f64)> {
    // sin 20°: below this the two edges are nearly parallel.
    const MIN_TRANSVERSE: f64 = 0.342;

    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    for (i, a) in lines.iter().enumerate() {
        let (sa, ca) = a.theta_deg.to_radians().sin_cos();
        for b in &lines[i + 1..] {
            let (sb, cb) = b.theta_deg.to_radians().sin_cos();
            let det = ca * sb - sa * cb;
            if det.abs() < MIN_TRANSVERSE {
                continue;
            }
            let u = (a.r * sb - b.r * sa) / det;
            let v = (b.r * ca - a.r * cb) / det;
            let d = ((u - near_u).powi(2) + (v - near_v).powi(2)).sqrt();
            if d <= radius && u.is_finite() && v.is_finite() {
                found.push((d, u, v));
            }
        }
    }

    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Merge candidates within a pixel of each other: several Hough lines often
    // describe the same physical edge, producing a cluster on one corner.
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (_, u, v) in found {
        if out
            .iter()
            .all(|(pu, pv)| (u - pu).hypot(v - pv) > 1.5)
        {
            out.push((u, v));
        }
    }
    out
}

/// A loaded screenshot, ready to be asked "what corners are near here?".
pub struct EdgeAssist {
    gray: image::GrayImage,
    low: f32,
    high: f32,
}

impl EdgeAssist {
    /// Canny thresholds that work on Minecraft's flat-shaded, high-contrast
    /// block edges. The greatest possible edge strength is about 1140, so these
    /// are a low bar deliberately — block outlines are strong, and being
    /// generous costs only a few extra candidate lines.
    pub const DEFAULT_LOW: f32 = 40.0;
    pub const DEFAULT_HIGH: f32 = 100.0;

    pub fn load(path: &str) -> Result<Self> {
        let img = image::open(path).map_err(|e| anyhow!("could not read {path}: {e}"))?;
        Ok(Self {
            gray: img.to_luma8(),
            low: Self::DEFAULT_LOW,
            high: Self::DEFAULT_HIGH,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.gray.dimensions()
    }

    /// Corner candidates near a roughly-placed pixel.
    ///
    /// The Hough transform runs on a crop around the point rather than the
    /// whole screenshot: it is far faster on a 4K image, and more importantly
    /// the votes then come only from edges that are actually near the corner
    /// the user is trying to tag, instead of from the longest lines in the
    /// frame.
    pub fn candidates_near(&self, u: f64, v: f64, radius: u32) -> Vec<(f64, f64)> {
        let (w, h) = self.gray.dimensions();
        if w == 0 || h == 0 {
            return Vec::new();
        }
        let x0 = (u.round() as i64 - radius as i64).clamp(0, w as i64 - 1) as u32;
        let y0 = (v.round() as i64 - radius as i64).clamp(0, h as i64 - 1) as u32;
        let cw = (2 * radius + 1).min(w - x0);
        let ch = (2 * radius + 1).min(h - y0);
        if cw < 8 || ch < 8 {
            return Vec::new();
        }

        let crop = image::imageops::crop_imm(&self.gray, x0, y0, cw, ch).to_image();
        let edges = imageproc::edges::canny(&crop, self.low, self.high);
        let lines = imageproc::hough::detect_lines(
            &edges,
            imageproc::hough::LineDetectionOptions {
                // A line has to run across a decent fraction of the crop to
                // count, which is what separates a block edge from texture
                // noise inside a face.
                vote_threshold: (cw.min(ch) / 3).max(12),
                suppression_radius: 6,
            },
        );

        let lines: Vec<EdgeLine> = lines
            .iter()
            .map(|l| EdgeLine {
                r: l.r as f64,
                theta_deg: l.angle_in_degrees as f64,
            })
            .collect();

        corner_candidates(
            &lines,
            u - x0 as f64,
            v - y0 as f64,
            radius as f64,
        )
        .into_iter()
        .map(|(cu, cv)| (cu + x0 as f64, cv + y0 as f64))
        .take(6)
        .collect()
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 5 · Camera Pose Estimator");
    println!(
        "  Recovers where a screenshot was taken from, and which way the camera was\n  \
         looking, from a few block corners you can identify in it. The yaw it finds can\n  \
         seed mode 8's triangulation, or narrow a search in modes 2 and 11."
    );
    println!();
    ui::note("Reading a block corner off F3:");
    ui::note("  • Point the crosshair at the block and read the 'Targeted Block' line.");
    ui::note("  • Those three integers are the block's minimum corner: bottom, north, west.");
    ui::note("  • Add 1 to X for its east face, 1 to Y for its top, 1 to Z for its south face.");
    ui::note("  • So the top-south-east corner of block (100, 64, -200) is (101, 65, -199).");
    ui::note("Pick corners at clearly different depths — corners from one flat wall are");
    ui::note("coplanar and the solve is singular on them, which this will refuse to guess at.");
    println!();

    // --- the screenshot -----------------------------------------------------
    let path = ui::input_optional("Screenshot path (blank to type the resolution instead)")?;
    let path = path.trim().to_string();

    let mut assist: Option<EdgeAssist> = None;
    let (width, height) = if path.is_empty() {
        let w: u32 = ui::input_default("Screenshot width in pixels", 1920)?;
        let h: u32 = ui::input_default("Screenshot height in pixels", 1080)?;
        (w, h)
    } else {
        match EdgeAssist::load(&path) {
            Ok(a) => {
                let (w, h) = a.dimensions();
                ui::success(&format!("loaded {path} ({w} x {h})"));
                assist = Some(a);
                (w, h)
            }
            Err(e) => {
                ui::warn(&format!("{e}"));
                ui::note("carrying on without the image — the maths only needs the resolution");
                let w: u32 = ui::input_default("Screenshot width in pixels", 1920)?;
                let h: u32 = ui::input_default("Screenshot height in pixels", 1080)?;
                (w, h)
            }
        }
    };
    if width == 0 || height == 0 {
        bail!("the screenshot resolution must be non-zero");
    }

    let mut snap_radius: u32 = 0;
    if assist.is_some() {
        ui::note("The edge-detection assist can suggest a detected corner near each pixel you");
        ui::note("type, so you can snap to a real feature instead of eyeballing it.");
        if ui::confirm("Use the edge-detection assist?", true)? {
            snap_radius = ui::input_default("Search radius around each typed pixel (px)", 25u32)?;
            if snap_radius < 4 {
                ui::warn("radius too small to detect anything; assist disabled");
                assist = None;
            }
        } else {
            assist = None;
        }
    }

    // --- intrinsics ---------------------------------------------------------
    println!();
    ui::note(&format!(
        "Minecraft's FOV slider is the vertical field of view. Default {DEFAULT_FOV_DEG:.0}, \
         'Quake Pro' is {QUAKE_PRO_FOV_DEG:.0}."
    ));
    ui::note("If you were sprinting, flying or under Speed, the effective FOV was wider than");
    ui::note("the slider — with 6+ tags you can solve for it below instead of guessing.");
    let vfov: f64 = ui::input_default("Vertical FOV in degrees", DEFAULT_FOV_DEG)?;
    if !(vfov > 1.0 && vfov < 179.0) {
        bail!("a vertical FOV of {vfov} degrees is not a usable camera");
    }
    let k = intrinsics(width, height, vfov);
    ui::note(&format!(
        "focal length {:.1} px, principal point ({:.1}, {:.1})",
        k[(0, 0)],
        k[(0, 2)],
        k[(1, 2)]
    ));

    // --- the tags -----------------------------------------------------------
    println!();
    println!(
        "  Now the correspondences. At least {MIN_CORRESPONDENCES} are needed; more is better,\n  \
         and {MIN_FOR_DLT}+ unlocks the linear solve and the option to fit the FOV too.\n  \
         Type 'undo' at a pixel prompt to drop the previous tag."
    );

    let mut tags: Vec<Correspondence> = Vec::new();
    loop {
        let idx = tags.len() + 1;
        let prompt = if tags.len() >= MIN_CORRESPONDENCES {
            format!("Tag {idx} — pixel «u v» (blank to finish)")
        } else {
            format!("Tag {idx} — pixel «u v»")
        };
        let raw = ui::input_optional(&prompt)?;
        let raw = raw.trim();

        if raw.eq_ignore_ascii_case("undo") {
            match tags.pop() {
                Some(_) => ui::note("dropped the last tag"),
                None => ui::warn("nothing to undo"),
            }
            continue;
        }
        if raw.is_empty() {
            if tags.len() >= MIN_CORRESPONDENCES {
                break;
            }
            ui::warn(&format!(
                "{} more tag(s) needed before a solve is possible",
                MIN_CORRESPONDENCES - tags.len()
            ));
            continue;
        }

        let Some(px) = ui::parse_coords(raw) else {
            ui::warn("expected two numbers, e.g. «812 344»");
            continue;
        };
        let (mut u, mut v) = (px[0], px[1]);
        if u < 0.0 || v < 0.0 || u > width as f64 || v > height as f64 {
            ui::warn(&format!(
                "({u}, {v}) is outside the {width} x {height} image; using it anyway, but check it"
            ));
        }

        if let Some(a) = &assist {
            let cands = a.candidates_near(u, v, snap_radius);
            if cands.is_empty() {
                ui::note("no detected corner nearby; keeping the pixel you typed");
            } else {
                let mut items = vec![format!("keep what I typed ({u:.1}, {v:.1})")];
                for (cu, cv) in &cands {
                    items.push(format!(
                        "detected corner ({cu:.1}, {cv:.1})  —  {:.1} px away",
                        (cu - u).hypot(cv - v)
                    ));
                }
                let choice = ui::select("Snap this tag to a detected corner?", &items)?;
                if choice > 0 {
                    let (cu, cv) = cands[choice - 1];
                    u = cu;
                    v = cv;
                }
            }
        }

        let world = ui::input_optional(&format!("Tag {idx} — world corner «X Y Z»"))?;
        let Some(w) = ui::parse_coords(&world).filter(|w| w.len() >= 3) else {
            ui::warn("expected three numbers, e.g. «101 65 -199»; tag discarded");
            continue;
        };
        tags.push(Correspondence {
            u,
            v,
            x: w[0],
            y: w[1],
            z: w[2],
        });
        ui::success(&format!(
            "tag {idx}: pixel ({u:.1}, {v:.1}) ← block corner ({}, {}, {})",
            w[0], w[1], w[2]
        ));
    }

    // --- solve --------------------------------------------------------------
    let solve_focal = if tags.len() >= MIN_FOR_FOCAL {
        ui::confirm(
            "Solve for the FOV as well? (only if you are unsure of the slider value)",
            false,
        )?
    } else {
        false
    };

    println!();
    let solution = solve_pose_full(&tags, &k, solve_focal)?;
    report(&solution, &tags);

    // --- hand the heading on ------------------------------------------------
    let yaw = solution.pose.yaw_deg;
    let store = match session.heading {
        Some(existing) => {
            println!();
            ui::warn(&format!(
                "the session already holds a heading of {existing:.1}°"
            ));
            ui::confirm(&format!("Overwrite it with {yaw:.1}°?"), false)?
        }
        None => ui::confirm(
            &format!("Store yaw {yaw:.1}° as the session heading (mode 8 will use it)?"),
            true,
        )?,
    };
    if store {
        session.heading = Some(normalise_yaw(yaw));
        ui::success(&format!("session heading set to {:.2}°", normalise_yaw(yaw)));
    }

    Ok(())
}

fn report(s: &Solution, tags: &[Correspondence]) {
    let p = &s.pose;
    ui::header("Estimate");

    println!(
        "  Camera position   X {:.2}   Y {:.2}   Z {:.2}",
        p.x, p.y, p.z
    );
    ui::note(&format!(
        "for a first-person shot this is the eye, ~{PLAYER_EYE_HEIGHT} blocks above the feet, \
         so standing on y≈{:.1}",
        p.y - PLAYER_EYE_HEIGHT
    ));

    let (dx, dz) = yaw_to_vector(p.yaw_deg);
    println!();
    println!(
        "  Yaw               {:.2}°  ({:.2}° as F3 prints it)",
        p.yaw_deg,
        yaw_f3_style(p.yaw_deg)
    );
    println!("  Pitch             {:.2}°  ({})", p.pitch_deg, {
        if p.pitch_deg < -0.5 {
            "looking up"
        } else if p.pitch_deg > 0.5 {
            "looking down"
        } else {
            "level"
        }
    });
    println!("  Nearest cardinal  {}", yaw_to_cardinal(p.yaw_deg));
    println!("  Facing vector     dx {dx:+.4}   dz {dz:+.4}");

    println!();
    println!("  RMS reprojection error   {:.2} px", p.rms_px);
    ui::note(&format!(
        "from {} tag(s), {} start, {} refinement iteration(s)",
        tags.len(),
        s.init,
        s.iterations
    ));
    if s.focal_was_solved {
        println!(
            "  Solved FOV               {:.2}° vertical ({:.1} px focal length)",
            s.vfov_deg, s.focal_px
        );
    }

    println!();
    println!("  Per-tag fit:");
    println!("    #   distance (blocks)   reprojection error (px)");
    for (i, ((d, e), _)) in s
        .distances
        .iter()
        .zip(s.residuals_px.iter())
        .zip(tags.iter())
        .enumerate()
    {
        println!("    {:<3} {:>13.2}   {:>21.2}", i + 1, d, e);
    }

    // --- confidence ---------------------------------------------------------
    println!();
    let verdict = if p.rms_px < 1.5 {
        "tight — the tags are mutually consistent to about a pixel"
    } else if p.rms_px < 4.0 {
        "reasonable for hand-typed pixels"
    } else if p.rms_px < 12.0 {
        "loose — treat the yaw as ±several degrees"
    } else {
        "poor — something is wrong, most likely a mistyped coordinate or a swapped pair"
    };
    println!("  Confidence: {verdict}.");
    ui::warn(
        "This is an estimate from hand-tagged pixels, not a measurement. A pixel of tagging",
    );
    ui::warn("error at 30 blocks is roughly a tenth of a degree of yaw and a few centimetres");
    ui::warn("of position; a mistyped block corner can be worth tens of degrees. Cross-check");
    ui::warn("the cardinal against what you remember of the screenshot before trusting it.");

    if s.roll_deg.abs() > 1.0 {
        println!();
        ui::warn(&format!(
            "the fit wants {:.1}° of camera roll, and a Minecraft camera cannot roll at all",
            s.roll_deg
        ));
        ui::warn("— that is a strong sign at least one tag is wrong. Re-check them before use.");
    }

    if p.rms_px > 4.0 {
        println!();
        ui::note("Largest-error tags are the ones to re-check first; a single bad tag usually");
        ui::note("shows up as one row far worse than the rest in the table above.");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random::JavaRandom;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn yaw_vectors_match_minecrafts_convention() {
        // The single easiest thing in this file to get backwards. Minecraft's
        // yaw runs clockwise from south when seen from above.
        let cases = [
            (0.0, 0.0, 1.0),    // south, +Z
            (90.0, -1.0, 0.0),  // west,  -X
            (180.0, 0.0, -1.0), // north, -Z
            (270.0, 1.0, 0.0),  // east,  +X
        ];
        for (yaw, dx, dz) in cases {
            let (gx, gz) = yaw_to_vector(yaw);
            assert!(close(gx, dx, 1e-12), "yaw {yaw}: dx {gx} != {dx}");
            assert!(close(gz, dz, 1e-12), "yaw {yaw}: dz {gz} != {dz}");
        }
        // -90 is the same as 270, which is how F3 writes east.
        let (gx, gz) = yaw_to_vector(-90.0);
        assert!(close(gx, 1.0, 1e-12) && close(gz, 0.0, 1e-12));

        // Halfway between south and west must be south-west: -X and +Z both.
        let (gx, gz) = yaw_to_vector(45.0);
        assert!(gx < 0.0 && gz > 0.0, "yaw 45 should point south-west");
    }

    #[test]
    fn cardinals_snap_to_the_right_axis() {
        assert_eq!(yaw_to_cardinal(0.0), "+Z (south)");
        assert_eq!(yaw_to_cardinal(90.0), "-X (west)");
        assert_eq!(yaw_to_cardinal(180.0), "-Z (north)");
        assert_eq!(yaw_to_cardinal(270.0), "+X (east)");

        // Near misses round to the nearest cardinal, not the next one along.
        assert_eq!(yaw_to_cardinal(44.0), "+Z (south)");
        assert_eq!(yaw_to_cardinal(46.0), "-X (west)");
        assert_eq!(yaw_to_cardinal(134.0), "-X (west)");
        assert_eq!(yaw_to_cardinal(136.0), "-Z (north)");

        // Out-of-range yaws wrap rather than falling off the end.
        assert_eq!(yaw_to_cardinal(-90.0), "+X (east)");
        assert_eq!(yaw_to_cardinal(-180.0), "-Z (north)");
        assert_eq!(yaw_to_cardinal(450.0), "-X (west)");
        assert_eq!(yaw_to_cardinal(720.0), "+Z (south)");
    }

    #[test]
    fn yaw_normalisation_round_trips() {
        assert!(close(normalise_yaw(-90.0), 270.0, 1e-12));
        assert!(close(normalise_yaw(370.0), 10.0, 1e-12));
        assert!(close(yaw_f3_style(270.0), -90.0, 1e-12));
        assert!(close(yaw_f3_style(180.0), 180.0, 1e-12));
    }

    #[test]
    fn intrinsics_for_a_known_fov() {
        // 1080p at Minecraft's default 70° vertical FOV:
        // fy = 540 / tan(35°) = 540 / 0.7002075 = 771.20 px.
        let k = intrinsics(1920, 1080, 70.0);
        // Derived rather than hand-copied: a rounded literal here was off by
        // more than the tolerance it was checked against.
        let expected = 540.0 / (35f64.to_radians().tan());
        assert!(close(k[(0, 0)], expected, 1e-9), "fy was {}", k[(0, 0)]);
        assert!(close(k[(1, 1)], k[(0, 0)], 1e-12), "pixels must be square");
        assert!(close(k[(0, 2)], 960.0, 1e-12));
        assert!(close(k[(1, 2)], 540.0, 1e-12));
        assert!(close(k[(2, 2)], 1.0, 1e-12));

        // 90° vertical puts the focal length exactly at the half-height.
        assert!(close(intrinsics(800, 600, 90.0)[(1, 1)], 300.0, 1e-9));

        // The FOV is vertical, so the same setting on a wider window gives the
        // same focal length — a check that height, not width, drives it.
        assert!(close(
            intrinsics(3840, 1080, 70.0)[(0, 0)],
            intrinsics(1920, 1080, 70.0)[(0, 0)],
            1e-12
        ));

        // Round trip through the inverse.
        for fov in [50.0, 70.0, QUAKE_PRO_FOV_DEG] {
            assert!(close(vfov_from_focal(1080, focal_from_vfov(1080, fov)), fov, 1e-9));
        }
    }

    #[test]
    fn rotation_and_angles_round_trip() {
        for yaw in [0.0, 37.5, 123.4, 200.0, 359.9] {
            for pitch in [-80.0, -17.8, 0.0, 12.0, 75.0] {
                let r = rotation_from_yaw_pitch(yaw, pitch);
                // Orthonormal and right-handed, or the pose extraction below is
                // meaningless.
                let should_be_identity = r * r.transpose();
                for i in 0..3 {
                    for j in 0..3 {
                        let want = if i == j { 1.0 } else { 0.0 };
                        assert!(close(should_be_identity[(i, j)], want, 1e-12));
                    }
                }
                assert!(close(r.determinant(), 1.0, 1e-12));

                let (gy, gp, gr) = yaw_pitch_roll_from_rotation(&r);
                assert!(close(gy, normalise_yaw(yaw), 1e-9), "yaw {yaw} -> {gy}");
                assert!(close(gp, pitch, 1e-9), "pitch {pitch} -> {gp}");
                assert!(close(gr, 0.0, 1e-9), "unrolled camera reported roll {gr}");
            }
        }

        // The forward row must agree with the documented facing vector.
        let r = rotation_from_yaw_pitch(90.0, 0.0);
        assert!(close(r[(2, 0)], -1.0, 1e-12), "yaw 90 must face -X");
        assert!(close(r[(2, 2)], 0.0, 1e-12));
        // Positive pitch looks down, i.e. towards -Y.
        let r = rotation_from_yaw_pitch(0.0, 90.0);
        assert!(close(r[(2, 1)], -1.0, 1e-12), "pitch +90 must face -Y");
    }

    /// Builds a scene: a known camera, a set of non-coplanar world points, and
    /// the pixels they land on.
    fn synthetic_scene(
        yaw: f64,
        pitch: f64,
        cam: (f64, f64, f64),
        k: &Matrix3<f64>,
    ) -> (Vec<Correspondence>, Matrix3<f64>, Vector3<f64>) {
        let r = rotation_from_yaw_pitch(yaw, pitch);
        let centre = Vector3::new(cam.0, cam.1, cam.2);
        let t = translation_from_position(&r, &centre);

        // Block corners scattered in depth as well as across the frame — the
        // configuration the mode tells the user to aim for.
        let (fx, fz) = yaw_to_vector(yaw);
        let (rx, rz) = (-fz, fx); // perpendicular in the horizontal plane
        let mut pts = Vec::new();
        for (ahead, side, up) in [
            (12.0, -4.0, -2.0),
            (14.0, 3.0, 1.0),
            (22.0, -6.0, 3.0),
            (25.0, 5.0, -3.0),
            (35.0, -2.0, 0.0),
            (40.0, 7.0, 4.0),
            (18.0, 0.0, -5.0),
            (30.0, -8.0, 2.0),
        ] {
            let p = Vector3::new(
                centre.x + fx * ahead + rx * side,
                centre.y + up,
                centre.z + fz * ahead + rz * side,
            );
            // Snap to integers: real tags are block corners, and it keeps the
            // test honest about the conditioning of realistic inputs.
            let p = Vector3::new(p.x.round(), p.y.round(), p.z.round());
            let (u, v) = project_point(k, &r, &t, &p).expect("test point behind the camera");
            pts.push(Correspondence {
                u,
                v,
                x: p.x,
                y: p.y,
                z: p.z,
            });
        }
        (pts, r, t)
    }

    #[test]
    fn synthetic_round_trip_recovers_the_pose() {
        // The load-bearing test: project points through a known camera by hand,
        // then check the solver walks back to that exact camera. Anything that
        // merely "runs" will fail this.
        let k = intrinsics(1920, 1080, 70.0);
        let cases = [
            (123.4, -17.8, (100.5, 72.62, -340.2)),
            (0.0, 0.0, (0.5, 65.62, 0.5)),
            (270.0, 30.0, (-1204.5, 91.62, 8830.5)),
            (47.3, 5.0, (12.5, 70.62, -8.5)),
            (200.0, -60.0, (-30.5, 100.62, 44.5)),
        ];

        for (yaw, pitch, cam) in cases {
            let (pts, _, _) = synthetic_scene(yaw, pitch, cam, &k);
            let pose = solve_pose(&pts, &k)
                .unwrap_or_else(|e| panic!("solve failed for yaw {yaw} pitch {pitch}: {e}"));

            assert!(
                pose.rms_px < 1e-6,
                "yaw {yaw}: exact data should fit to machine precision, got {} px",
                pose.rms_px
            );
            assert!(
                close(pose.yaw_deg, normalise_yaw(yaw), 1e-4),
                "yaw {yaw} recovered as {}",
                pose.yaw_deg
            );
            assert!(
                close(pose.pitch_deg, pitch, 1e-4),
                "pitch {pitch} recovered as {}",
                pose.pitch_deg
            );
            assert!(
                close(pose.x, cam.0, 1e-3) && close(pose.y, cam.1, 1e-3) && close(pose.z, cam.2, 1e-3),
                "position {cam:?} recovered as ({}, {}, {})",
                pose.x,
                pose.y,
                pose.z
            );
            assert_eq!(yaw_to_cardinal(pose.yaw_deg), yaw_to_cardinal(yaw));
        }
    }

    #[test]
    fn round_trip_survives_the_minimum_four_tags() {
        // Four points skips the DLT entirely and leans on the coarse sweep.
        let k = intrinsics(1920, 1080, 70.0);
        let (pts, _, _) = synthetic_scene(88.0, -12.0, (250.5, 80.62, -60.5), &k);
        let pose = solve_pose(&pts[..4], &k).expect("four non-coplanar tags should solve");
        assert!(pose.rms_px < 1e-5, "rms {}", pose.rms_px);
        assert!(close(pose.yaw_deg, 88.0, 1e-3), "yaw {}", pose.yaw_deg);
        assert!(close(pose.pitch_deg, -12.0, 1e-3), "pitch {}", pose.pitch_deg);
    }

    #[test]
    fn round_trip_degrades_gracefully_under_tagging_noise() {
        // Hand-tagged pixels are worth about a pixel each. The estimate should
        // stay within a fraction of a degree, and the reported RMS should be
        // honest about the noise that went in rather than claiming a perfect
        // fit.
        let k = intrinsics(1920, 1080, 70.0);
        let (mut pts, _, _) = synthetic_scene(214.7, 8.3, (-500.5, 74.62, 1200.5), &k);

        // Deterministic ±1.5 px jitter, reusing the crate's Java LCG so the
        // test is reproducible without a new dependency.
        let mut rng = JavaRandom::new(20240521);
        for p in &mut pts {
            p.u += (rng.next_double() - 0.5) * 3.0;
            p.v += (rng.next_double() - 0.5) * 3.0;
        }

        let pose = solve_pose(&pts, &k).expect("noisy but well-conditioned tags should solve");
        assert!(
            pose.rms_px > 0.05 && pose.rms_px < 3.0,
            "rms should reflect the injected noise, got {}",
            pose.rms_px
        );
        assert!(
            close(pose.yaw_deg, 214.7, 1.0),
            "yaw drifted to {} under 1.5 px of noise",
            pose.yaw_deg
        );
        assert!(close(pose.pitch_deg, 8.3, 1.0), "pitch {}", pose.pitch_deg);
        assert_eq!(yaw_to_cardinal(pose.yaw_deg), "-Z (north)");
    }

    #[test]
    fn solving_for_fov_recovers_a_wrong_slider_value() {
        // Scene shot at 90° (say, sprinting with a wide slider) but handed to
        // the solver as the default 70°. With the focal length free it should
        // find its way back.
        let truth = intrinsics(1920, 1080, 90.0);
        let (pts, _, _) = synthetic_scene(15.0, -6.0, (40.5, 68.62, 90.5), &truth);

        let guess = intrinsics(1920, 1080, 70.0);
        let fixed = solve_pose(&pts, &guess).expect("fixed-FOV solve should still return");
        assert!(
            fixed.rms_px > 1.0,
            "a 20° FOV error should leave visible residuals, got {}",
            fixed.rms_px
        );

        let solved = solve_pose_full(&pts, &guess, true).expect("focal solve should converge");
        assert!(solved.focal_was_solved);
        assert!(
            close(solved.vfov_deg, 90.0, 0.2),
            "solved FOV {} should be near 90",
            solved.vfov_deg
        );
        assert!(solved.pose.rms_px < 1e-4, "rms {}", solved.pose.rms_px);
        assert!(close(solved.pose.yaw_deg, 15.0, 1e-2));
    }

    #[test]
    fn distances_to_tagged_points_are_reported() {
        let k = intrinsics(1920, 1080, 70.0);
        let cam = (100.5, 72.62, -340.2);
        let (pts, _, _) = synthetic_scene(60.0, 0.0, cam, &k);
        let s = solve_pose_full(&pts, &k, false).unwrap();
        assert_eq!(s.distances.len(), pts.len());
        for (d, p) in s.distances.iter().zip(pts.iter()) {
            let truth = ((p.x - cam.0).powi(2) + (p.y - cam.1).powi(2) + (p.z - cam.2).powi(2)).sqrt();
            assert!(close(*d, truth, 1e-3), "distance {d} != {truth}");
        }
        assert!(s.roll_deg.abs() < 1e-6, "clean data must show no roll");
    }

    #[test]
    fn coplanar_tags_are_refused_rather_than_guessed_at() {
        // Every corner off one flat wall: a best-fit pose exists, but it is not
        // determined by the data, so returning one would be a confident lie.
        let k = intrinsics(1920, 1080, 70.0);
        let r = rotation_from_yaw_pitch(180.0, 0.0);
        let t = translation_from_position(&r, &Vector3::new(0.5, 70.0, 20.5));

        let mut pts = Vec::new();
        for (x, y) in [(-6.0, 66.0), (5.0, 66.0), (-6.0, 74.0), (5.0, 74.0), (0.0, 70.0), (3.0, 68.0)] {
            let p = Vector3::new(x, y, -10.0); // all on the plane z = -10
            let (u, v) = project_point(&k, &r, &t, &p).unwrap();
            pts.push(Correspondence { u, v, x, y, z: -10.0 });
        }

        let err = solve_pose(&pts, &k).expect_err("a coplanar tag set must not return a pose");
        let msg = err.to_string().to_lowercase();
        assert!(msg.contains("coplanar"), "unhelpful error: {err}");
    }

    #[test]
    fn collinear_tags_are_refused() {
        let k = intrinsics(1920, 1080, 70.0);
        let r = rotation_from_yaw_pitch(0.0, 0.0);
        let t = translation_from_position(&r, &Vector3::new(0.0, 70.0, 0.0));

        // Corners walking straight along one block edge: the camera could be
        // anywhere on a circle around that line.
        let mut pts = Vec::new();
        for i in 0..5 {
            let p = Vector3::new(2.0, 70.0, 20.0 + i as f64);
            let (u, v) = project_point(&k, &r, &t, &p).unwrap();
            pts.push(Correspondence { u, v, x: p.x, y: p.y, z: p.z });
        }
        let err = solve_pose(&pts, &k).expect_err("a collinear tag set must not return a pose");
        assert!(
            err.to_string().to_lowercase().contains("collinear"),
            "unhelpful error: {err}"
        );
    }

    #[test]
    fn too_few_tags_and_bad_requests_are_refused() {
        let k = intrinsics(1920, 1080, 70.0);
        let (pts, _, _) = synthetic_scene(30.0, 0.0, (0.5, 70.62, 0.5), &k);

        for n in 0..MIN_CORRESPONDENCES {
            let err = solve_pose(&pts[..n], &k).expect_err("{n} tags must not solve");
            assert!(err.to_string().contains("at least"), "{err}");
        }
        assert!(solve_pose(&pts[..MIN_CORRESPONDENCES], &k).is_ok());

        // Fitting the FOV needs more than the bare minimum.
        assert!(solve_pose_full(&pts[..5], &k, true).is_err());
        assert!(solve_pose_full(&pts[..MIN_FOR_FOCAL], &k, true).is_ok());
    }

    #[test]
    fn identical_or_line_bound_pixels_are_refused() {
        // Correct-looking world points but useless pixels — the mirror image of
        // the world-geometry checks.
        let k = intrinsics(1920, 1080, 70.0);
        let world = [
            (10.0, 70.0, 30.0),
            (12.0, 72.0, 35.0),
            (8.0, 68.0, 41.0),
            (15.0, 74.0, 50.0),
            (11.0, 66.0, 60.0),
            (9.0, 71.0, 25.0),
        ];

        let same: Vec<Correspondence> = world
            .iter()
            .map(|&(x, y, z)| Correspondence { u: 960.0, v: 540.0, x, y, z })
            .collect();
        assert!(solve_pose(&same, &k).is_err());

        let on_a_row: Vec<Correspondence> = world
            .iter()
            .enumerate()
            .map(|(i, &(x, y, z))| Correspondence { u: 100.0 + 60.0 * i as f64, v: 540.0, x, y, z })
            .collect();
        let err = solve_pose(&on_a_row, &k).expect_err("pixels on one image line cannot solve");
        assert!(err.to_string().to_lowercase().contains("line"), "{err}");
    }

    #[test]
    fn corner_candidates_find_the_intersection_of_two_edges() {
        // A vertical edge at u = 100 (θ = 0 → u·1 + v·0 = 100) and a horizontal
        // edge at v = 60 (θ = 90 → u·0 + v·1 = 60) meet at (100, 60).
        let lines = [
            EdgeLine { r: 100.0, theta_deg: 0.0 },
            EdgeLine { r: 60.0, theta_deg: 90.0 },
        ];
        let found = corner_candidates(&lines, 103.0, 57.0, 20.0);
        assert_eq!(found.len(), 1, "expected one corner, got {found:?}");
        assert!(close(found[0].0, 100.0, 1e-6) && close(found[0].1, 60.0, 1e-6));

        // Out of range of the rough click: nothing offered.
        assert!(corner_candidates(&lines, 400.0, 400.0, 20.0).is_empty());

        // Near-parallel edges are dropped: their crossing is not a corner you
        // could sensibly snap to.
        let parallel = [
            EdgeLine { r: 100.0, theta_deg: 0.0 },
            EdgeLine { r: 104.0, theta_deg: 3.0 },
        ];
        assert!(corner_candidates(&parallel, 100.0, 60.0, 500.0).is_empty());

        // Duplicate detections of the same physical corner collapse to one.
        let dupes = [
            EdgeLine { r: 100.0, theta_deg: 0.0 },
            EdgeLine { r: 100.4, theta_deg: 0.0 },
            EdgeLine { r: 60.0, theta_deg: 90.0 },
        ];
        assert_eq!(corner_candidates(&dupes, 100.0, 60.0, 20.0).len(), 1);
    }

    #[test]
    fn projection_rejects_points_behind_the_camera() {
        let k = intrinsics(1920, 1080, 70.0);
        let r = rotation_from_yaw_pitch(0.0, 0.0); // facing +Z
        let t = translation_from_position(&r, &Vector3::new(0.0, 70.0, 0.0));
        assert!(project_point(&k, &r, &t, &Vector3::new(0.0, 70.0, 10.0)).is_some());
        assert!(project_point(&k, &r, &t, &Vector3::new(0.0, 70.0, -10.0)).is_none());

        // A point straight ahead lands on the principal point.
        let (u, v) = project_point(&k, &r, &t, &Vector3::new(0.0, 70.0, 10.0)).unwrap();
        assert!(close(u, 960.0, 1e-9) && close(v, 540.0, 1e-9));

        // A point to the west (-X) of a south-facing camera appears on the
        // right of the frame, because facing south your right hand is west.
        let (u, _) = project_point(&k, &r, &t, &Vector3::new(-5.0, 70.0, 10.0)).unwrap();
        assert!(u > 960.0, "west should be screen-right when facing south");
    }
}
