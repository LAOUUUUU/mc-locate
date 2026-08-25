//! State shared between modes, so that a value discovered in one mode does not
//! have to be typed again in the next.

use crate::worldgen::Version;

/// A block-coordinate bounding box, used to hand a narrowed search area from
/// one mode to another (modes 6, 8 and 11 produce these; mode 2 consumes them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl BBox {
    pub fn around(x: i32, z: i32, radius: i32) -> Self {
        Self {
            min_x: x.saturating_sub(radius),
            min_z: z.saturating_sub(radius),
            max_x: x.saturating_add(radius),
            max_z: z.saturating_add(radius),
        }
    }

    /// A box from explicit corners, normalising them if given the wrong way round.
    pub fn around_rect(min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_z: min_z.min(max_z),
            max_x: min_x.max(max_x),
            max_z: min_z.max(max_z),
        }
    }

    pub fn width(&self) -> i64 {
        (self.max_x as i64 - self.min_x as i64 + 1).max(0)
    }

    pub fn depth(&self) -> i64 {
        (self.max_z as i64 - self.min_z as i64 + 1).max(0)
    }

    pub fn area(&self) -> i64 {
        self.width() * self.depth()
    }
}

impl std::fmt::Display for BBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "X {}..{}, Z {}..{} ({} x {} blocks)",
            self.min_x,
            self.max_x,
            self.min_z,
            self.max_z,
            self.width(),
            self.depth()
        )
    }
}

/// A chunk the user has confirmed is, or is not, a slime chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlimeObservation {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub is_slime: bool,
}

/// A single bedrock block observation in the Nether.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BedrockObservation {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// True for bedrock, false for anything else (air, lava, netherrack…).
    pub is_bedrock: bool,
}

/// An observed structure position, for use as a seed constraint.
#[derive(Debug, Clone, Copy)]
pub struct StructureObservation {
    pub structure: cubiomes::enums::StructureType,
    pub x: i32,
    pub z: i32,
}

#[derive(Debug, Default)]
pub struct Session {
    pub seed: Option<i64>,
    /// The hashed seed the server derives from the full world seed (SHA-256
    /// based). Disambiguates the 65,536 world seeds a structure seed lifts to.
    pub hashed_seed: Option<i64>,
    /// The Minecraft version, when the bundled generator can generate it.
    pub version: Option<Version>,
    /// A version *newer* than the generator supports, recorded verbatim.
    ///
    /// Minecraft outruns the worldgen library — cubiomes stops at 1.21.4 while
    /// the game is on 26.x. Without somewhere to put that, a player on a newer
    /// version has to pick the closest supported one, and every
    /// structure-and-biome answer is then silently wrong. Recording it lets
    /// generator-backed modes refuse outright while the version-independent
    /// ones (slime chunks, nether bedrock, End pillars, portal maths) carry on.
    pub newer_version: Option<String>,
    /// Absolute compass heading in degrees, as Minecraft yaw
    /// (0 = +Z / south, 90 = -X / west, 180 = -Z / north, 270 = +X / east).
    pub heading: Option<f64>,
    pub search_box: Option<BBox>,
    /// Candidate structure seeds carried between cracking modes.
    pub candidates: Vec<i64>,
    pub slime: Vec<SlimeObservation>,
    pub bedrock: Vec<BedrockObservation>,
    pub structures: Vec<StructureObservation>,
    /// Observed End pillar heights, in the fixed pillar order used by
    /// [`crate::multicrack::PILLAR_POSITIONS`]. `None` for unobserved pillars.
    pub pillar_heights: Option<[Option<i32>; 10]>,
    /// Eye-of-ender bearings imported from the exporter mod, offered as a
    /// starting point by the stronghold mode.
    pub eye_throws: Vec<crate::stronghold::Throw>,
}

impl Session {
    /// Status-bar fields: short label, value.
    ///
    /// Structured rather than a pre-joined string so the presentation layer
    /// decides how to render it — [`Session::summary`] stays for plain-text
    /// contexts like the saved-file note.
    pub fn status_fields(&self) -> Vec<(&'static str, String)> {
        let mut out: Vec<(&'static str, String)> = Vec::new();
        if let Some(s) = self.seed {
            out.push(("seed", s.to_string()));
        }
        if let Some(v) = self.version {
            out.push(("mc", v.label().to_string()));
        } else if let Some(v) = &self.newer_version {
            out.push(("mc", format!("{v} (unsupported)")));
        }
        if let Some(h) = self.heading {
            out.push(("heading", format!("{h:.1}°")));
        }
        if let Some(b) = self.search_box {
            out.push(("box", format!("{}x{}", b.width(), b.depth())));
        }
        if !self.candidates.is_empty() {
            out.push(("candidates", self.candidates.len().to_string()));
        }
        let obs = self.slime.len() + self.bedrock.len() + self.structures.len();
        if obs > 0 {
            out.push(("observations", obs.to_string()));
        }
        if self.pillar_heights.is_some() {
            out.push(("pillars", "recorded".to_string()));
        }
        out
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.seed {
            Some(s) => parts.push(format!("seed {s}")),
            None => parts.push("no seed".to_string()),
        }
        if let Some(v) = self.version {
            parts.push(v.label().to_string());
        }
        if let Some(h) = self.heading {
            parts.push(format!("heading {h:.1}°"));
        }
        if let Some(b) = self.search_box {
            parts.push(format!("box {b}"));
        }
        if !self.candidates.is_empty() {
            parts.push(format!("{} candidate seed(s)", self.candidates.len()));
        }
        if !self.slime.is_empty() {
            parts.push(format!("{} slime obs", self.slime.len()));
        }
        if !self.bedrock.is_empty() {
            parts.push(format!("{} bedrock obs", self.bedrock.len()));
        }
        if !self.structures.is_empty() {
            parts.push(format!("{} structure obs", self.structures.len()));
        }
        if self.pillar_heights.is_some() {
            parts.push("pillars recorded".to_string());
        }
        parts.join(" | ")
    }
}
