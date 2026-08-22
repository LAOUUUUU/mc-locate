# Mode 5 — Camera Pose Estimator

Works out which way a screenshot was taken from, and roughly where.

**Needs:** the screenshot, plus at least four correspondences — a pixel
position and the world coordinate of the block corner it shows.

**Where it comes from:** pick recognisable block corners in the shot and read
their coordinates off F3, or from a map you already have.

**How it works.** Build the camera intrinsics from image height and vertical
FOV (Minecraft's FOV slider is vertical; 70 is default, "Quake Pro" is 110),
solve for pose by DLT, orthonormalise the rotation via SVD, then refine by
minimising reprojection error. With six or more points the FOV itself can be
solved for, which is useful when you do not know what the slider was on.

Optionally uses Canny edge detection and Hough lines to help you snap a tag to
a detected corner rather than eyeballing the pixel.

**Output** is yaw and pitch in Minecraft's convention — yaw 0 faces +Z (south),
90 faces -X (west), 180 faces -Z (north), 270 faces +X (east) — plus the nearest
cardinal, estimated distances, and the RMS reprojection error so you can judge
how much to trust it.

**Limits.** Degenerate inputs (nearly coplanar points) are refused with an
explanation rather than answered confidently. It is an estimate from
hand-tagged pixels and reports its error accordingly.

**Feeds:** the session heading, which mode 8 consumes directly.
