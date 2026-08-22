//! Mode 11 — Nether ↔ Overworld coordinate conversion.
//!
//! The Nether runs at a 1:8 horizontal scale. Y is not scaled at all.
//!
//! The direction of the rounding matters and is the classic off-by-one here:
//! the game divides with `Math.floor`, not truncation-toward-zero, so
//! overworld −1290 maps to nether −162 and *not* −161. Rust's `/` truncates
//! toward zero, so every conversion below goes through `div_euclid`.
//!
//! Going the other way is exact (multiply by 8), but a portal you walk through
//! does not land on that exact block: the game looks for an existing portal
//! near the ideal destination and only builds a new one if it finds nothing.
//! That search radius is what makes this mode useful as a *search box*
//! generator rather than a point lookup.

use anyhow::Result;

use crate::session::{BBox, Session};
use crate::ui;

/// Horizontal scale factor between the dimensions.
pub const SCALE: i32 = 8;

/// Radius, in blocks, of the game's search for an existing portal when
/// travelling **to the Nether** (3x3 chunks around the destination chunk).
pub const NETHER_SEARCH_RADIUS: i32 = 128;

/// Radius, in blocks, of the search when travelling **to the Overworld**
/// (17x17 chunks — a much wider net).
pub const OVERWORLD_SEARCH_RADIUS: i32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Overworld,
    Nether,
}

impl Dim {
    pub fn label(&self) -> &'static str {
        match self {
            Dim::Overworld => "Overworld",
            Dim::Nether => "Nether",
        }
    }

    pub fn other(&self) -> Dim {
        match self {
            Dim::Overworld => Dim::Nether,
            Dim::Nether => Dim::Overworld,
        }
    }
}

/// Overworld → Nether: floor-divide X and Z by 8, leave Y alone.
#[inline]
pub fn overworld_to_nether(x: i32, z: i32) -> (i32, i32) {
    (x.div_euclid(SCALE), z.div_euclid(SCALE))
}

/// Nether → Overworld: multiply X and Z by 8, leave Y alone.
#[inline]
pub fn nether_to_overworld(x: i32, z: i32) -> (i32, i32) {
    (x.saturating_mul(SCALE), z.saturating_mul(SCALE))
}

/// Converts a coordinate from `from` into the other dimension.
pub fn convert(from: Dim, x: i32, z: i32) -> (i32, i32) {
    match from {
        Dim::Overworld => overworld_to_nether(x, z),
        Dim::Nether => nether_to_overworld(x, z),
    }
}

/// The box worth searching in the destination dimension.
///
/// Travelling to the Overworld the game searches much further than it does
/// travelling to the Nether, so the box is asymmetric.
pub fn search_box(destination: Dim, x: i32, z: i32) -> BBox {
    let radius = match destination {
        Dim::Nether => NETHER_SEARCH_RADIUS,
        Dim::Overworld => OVERWORLD_SEARCH_RADIUS,
    };
    BBox::around(x, z, radius)
}

pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 11 — Nether ↔ Overworld Portal Converter");
    ui::note("The Nether is 1:8 horizontally. Y is the same in both dimensions.");

    let from = if ui::select_str(
        "Which dimension is your coordinate in?",
        &["Nether", "Overworld"],
    )? == 0
    {
        Dim::Nether
    } else {
        Dim::Overworld
    };

    let x: i32 = ui::input("X")?;
    let y: Option<i32> = {
        let raw = ui::input_optional("Y (optional, unscaled — press Enter to skip)")?;
        raw.trim().parse::<i32>().ok()
    };
    let z: i32 = ui::input("Z")?;

    let to = from.other();
    let (nx, nz) = convert(from, x, z);

    println!();
    match y {
        Some(y) => {
            ui::success(&format!("{}: {x}, {y}, {z}", from.label()));
            ui::success(&format!("{}: {nx}, {y}, {nz}", to.label()));
        }
        None => {
            ui::success(&format!("{}: {x}, {z}", from.label()));
            ui::success(&format!("{}: {nx}, {nz}", to.label()));
        }
    }

    let bbox = search_box(to, nx, nz);
    println!();
    ui::note(&format!(
        "Travelling to the {}, the game looks for an existing portal within {} blocks of the \
         ideal destination and only builds a new one if it finds none.",
        to.label(),
        match to {
            Dim::Nether => NETHER_SEARCH_RADIUS,
            Dim::Overworld => OVERWORLD_SEARCH_RADIUS,
        }
    ));
    ui::note(&format!("So the area worth searching is {bbox}."));

    if from == Dim::Nether {
        ui::note(
            "Because each nether block covers 8 overworld blocks, a nether coordinate read off a \
             screenshot pins the overworld position only to within 8 blocks before this search \
             radius is even applied.",
        );
    }

    if ui::confirm("Store this as the session search box (for modes 2 and 6)?", true)? {
        session.search_box = Some(bbox);
        ui::success("Stored.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_coordinates_scale_by_eight() {
        assert_eq!(overworld_to_nether(800, 400), (100, 50));
        assert_eq!(nether_to_overworld(100, 50), (800, 400));
    }

    #[test]
    fn negative_coordinates_floor_rather_than_truncate() {
        // The documented case: -1290 must floor to -162, not truncate to -161.
        assert_eq!(overworld_to_nether(-1290, -1290), (-162, -162));
        // Rust's `/` would give the wrong answer here, which is the whole
        // reason div_euclid is used. Note the parentheses: unary minus binds
        // looser than a method call, so `-1290i32.div_euclid(8)` would negate
        // the result of `1290.div_euclid(8)` and quietly pass for -161.
        assert_ne!(-1290 / 8, -162);
        assert_eq!((-1290i32).div_euclid(8), -162);
        assert_eq!(-1290i32 / 8, -161);

        assert_eq!(overworld_to_nether(-1, -1), (-1, -1));
        assert_eq!(overworld_to_nether(-8, -8), (-1, -1));
        assert_eq!(overworld_to_nether(-9, -9), (-2, -2));
        assert_eq!(overworld_to_nether(0, 0), (0, 0));
    }

    #[test]
    fn round_trip_is_lossy_in_the_expected_direction() {
        // Nether -> Overworld -> Nether is exact; the other way round loses
        // the sub-8-block detail, which is exactly why mode 11 emits a box.
        for n in [-500i32, -1, 0, 1, 137, 4096] {
            let (ox, oz) = nether_to_overworld(n, n);
            assert_eq!(overworld_to_nether(ox, oz), (n, n));
        }
        for o in [-1290i32, -7, 3, 801] {
            let (nx, nz) = overworld_to_nether(o, o);
            let (bx, _) = nether_to_overworld(nx, nz);
            assert!((o - bx).abs() < 8, "{o} came back as {bx}");
        }
    }

    #[test]
    fn search_boxes_are_asymmetric_between_directions() {
        let to_nether = search_box(Dim::Nether, 100, 50);
        let to_overworld = search_box(Dim::Overworld, 800, 400);
        assert_eq!(to_nether.width(), 2 * NETHER_SEARCH_RADIUS as i64 + 1);
        assert_eq!(to_overworld.width(), 2 * OVERWORLD_SEARCH_RADIUS as i64 + 1);
        assert!(to_overworld.area() > to_nether.area());
    }

    #[test]
    fn convert_dispatches_on_the_source_dimension() {
        assert_eq!(convert(Dim::Overworld, 800, 400), (100, 50));
        assert_eq!(convert(Dim::Nether, 100, 50), (800, 400));
        assert_eq!(Dim::Nether.other(), Dim::Overworld);
    }

    #[test]
    fn extreme_nether_coordinates_saturate_rather_than_overflow() {
        let (x, _) = nether_to_overworld(i32::MAX, 0);
        assert_eq!(x, i32::MAX);
    }
}
