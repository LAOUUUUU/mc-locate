//! The observation file format — mc-locate's interchange schema.
//!
//! Everything the tool learns lives in a [`Session`], which until now existed
//! only in memory: close the program and every coordinate you typed was gone.
//! This module gives that state a documented on-disk form, which does two jobs
//! at once.
//!
//! **Save and resume.** Collecting forty bedrock blocks is a real investment;
//! it should survive quitting.
//!
//! **A contract for other producers.** Anything that can write this JSON can
//! feed mc-locate — a Fabric mod dumping bedrock as you fly the Nether, a
//! script, a hand-written file, a future video analyser. The producer stays
//! dumb; all the maths stays here, where it is tested.
//!
//! # Stability
//!
//! The file carries a `format` tag and an integer `version`. Every field is
//! `#[serde(default)]`, so a file written by an older producer still loads and
//! an unknown future field is ignored rather than fatal. That matters most for
//! the mod case, where the writer and reader are versioned separately and will
//! drift.
//!
//! Deliberately *not* serialised as the in-memory types: this is a public
//! interchange format, so it is defined by its own structs. Internal
//! refactoring must not silently change the file on disk.

use anyhow::{Context, Result, bail};
use cubiomes::enums::StructureType;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::session::{
    BBox, BedrockObservation, Session, SlimeObservation, StructureObservation,
};
use crate::worldgen::{STRUCTURES, Version};

/// Tag identifying the file, so a wrong file produces a clear error rather
/// than a confusing parse failure.
pub const FORMAT_TAG: &str = "mc-locate-observations";
/// Bumped only for a breaking change; additive fields do not need it.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservationFile {
    pub format: String,
    pub version: u32,

    /// Minecraft version label, e.g. `"1.21.3"`. Absent means "ask the user".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minecraft_version: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,

    /// The server's hashed seed (SHA-256 based), which pins the full 64-bit
    /// world seed given a structure seed. See [`crate::hashseed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hashed_seed: Option<i64>,

    /// The doubly-hashed biome-zoom seed the client stores; same pinning power,
    /// and readable by the exporter mod without a mixin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biome_hash: Option<i64>,

    /// Set when the session's version is past what the generator supports.
    /// Recorded so a saved session round-trips honestly rather than silently
    /// losing the fact that it is a 26.x world.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_version: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_box: Option<BoxDto>,

    /// Candidate structure seeds carried between runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<i64>,

    /// The ten End pillar heights in generation order; `null` for unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pillar_heights: Option<Vec<Option<i32>>>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slime: Vec<SlimeDto>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bedrock: Vec<BedrockDto>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structures: Vec<StructureDto>,

    /// Eye-of-ender bearings for stronghold triangulation.
    ///
    /// A bearing taken from the eye entity itself is far better than one read
    /// off the F3 screen: the eye flies exactly at the stronghold, while the
    /// player's yaw is only roughly where they were looking and is quantised
    /// by mouse resolution. The exporter mod produces these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eye_throws: Vec<EyeThrowDto>,

    /// Free-form note about where the file came from — useful when a mod, a
    /// script and a person are all writing these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// One eye-of-ender bearing: where it was thrown from, and the yaw it flew.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EyeThrowDto {
    pub x: f64,
    pub z: f64,
    /// Minecraft yaw in degrees; 0 faces +Z, 90 faces -X.
    pub yaw: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoxDto {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlimeDto {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub is_slime: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BedrockDto {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub is_bedrock: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructureDto {
    /// cubiomes' canonical lower-case name, e.g. `"desert_pyramid"`.
    #[serde(rename = "type")]
    pub structure: String,
    pub x: i32,
    pub z: i32,
}

/// Resolves a structure name as written in a file.
///
/// Accepts cubiomes' canonical form and the human labels used in the menus, so
/// a hand-written file saying "Desert pyramid" works as well as one saying
/// "desert_pyramid".
pub fn parse_structure(name: &str) -> Result<StructureType> {
    let wanted = name.trim().to_lowercase().replace([' ', '-'], "_");
    for (stype, label, _) in STRUCTURES {
        if stype.to_string().to_lowercase() == wanted
            || label.to_lowercase().replace([' ', '-'], "_") == wanted
        {
            return Ok(*stype);
        }
    }
    bail!("{name:?} is not a structure this tool knows")
}

impl ObservationFile {
    /// Snapshots a session.
    pub fn from_session(session: &Session, source: Option<String>) -> ObservationFile {
        ObservationFile {
            format: FORMAT_TAG.to_string(),
            version: FORMAT_VERSION,
            minecraft_version: session.version.map(|v| v.label().to_string()),
            unsupported_version: session.newer_version.clone(),
            seed: session.seed,
            hashed_seed: session.hashed_seed,
            biome_hash: session.biome_hash,
            heading: session.heading,
            search_box: session.search_box.map(|b| BoxDto {
                min_x: b.min_x,
                min_z: b.min_z,
                max_x: b.max_x,
                max_z: b.max_z,
            }),
            candidates: session.candidates.clone(),
            pillar_heights: session.pillar_heights.map(|p| p.to_vec()),
            slime: session
                .slime
                .iter()
                .map(|o| SlimeDto {
                    chunk_x: o.chunk_x,
                    chunk_z: o.chunk_z,
                    is_slime: o.is_slime,
                })
                .collect(),
            bedrock: session
                .bedrock
                .iter()
                .map(|o| BedrockDto {
                    x: o.x,
                    y: o.y,
                    z: o.z,
                    is_bedrock: o.is_bedrock,
                })
                .collect(),
            eye_throws: session
                .eye_throws
                .iter()
                .map(|t| EyeThrowDto { x: t.x, z: t.z, yaw: t.yaw })
                .collect(),
            structures: session
                .structures
                .iter()
                .map(|o| StructureDto {
                    structure: o.structure.to_string(),
                    x: o.x,
                    z: o.z,
                })
                .collect(),
            source,
        }
    }

    /// Merges this file into a session.
    ///
    /// Observations are *added*, not replaced, and duplicates are dropped —
    /// importing the same mod dump twice must not double every constraint or
    /// make a contradiction out of nothing. Scalars (seed, version, heading)
    /// only fill gaps unless `overwrite` is set, so importing a coordinate
    /// dump cannot silently clobber a seed you already cracked.
    pub fn apply_to_session(&self, session: &mut Session, overwrite: bool) -> Result<ImportSummary> {
        self.check_header()?;
        let mut summary = ImportSummary::default();

        if let Some(label) = &self.minecraft_version {
            let found = Version::ALL.iter().find(|v| v.label() == label).copied();
            match found {
                Some(v) if overwrite || session.version.is_none() => session.version = Some(v),
                Some(_) => {}
                None => summary.warnings.push(format!(
                    "unknown Minecraft version {label:?} in file; leaving the session's alone"
                )),
            }
        }
        if let Some(v) = &self.unsupported_version
            && (overwrite || session.newer_version.is_none())
        {
            session.newer_version = Some(v.clone());
        }
        if let Some(h) = self.hashed_seed
            && (overwrite || session.hashed_seed.is_none())
        {
            session.hashed_seed = Some(h);
        }
        if let Some(h) = self.biome_hash
            && (overwrite || session.biome_hash.is_none())
        {
            session.biome_hash = Some(h);
        }
        if let Some(s) = self.seed
            && (overwrite || session.seed.is_none())
        {
            session.seed = Some(s);
        }
        if let Some(h) = self.heading
            && (overwrite || session.heading.is_none())
        {
            session.heading = Some(h);
        }
        if let Some(b) = self.search_box
            && (overwrite || session.search_box.is_none())
        {
            session.search_box = Some(BBox {
                min_x: b.min_x,
                min_z: b.min_z,
                max_x: b.max_x,
                max_z: b.max_z,
            });
        }
        if let Some(p) = &self.pillar_heights {
            if p.len() != 10 {
                summary
                    .warnings
                    .push(format!("expected 10 pillar heights, found {}; ignored", p.len()));
            } else if overwrite || session.pillar_heights.is_none() {
                let mut arr = [None; 10];
                for (i, v) in p.iter().enumerate() {
                    arr[i] = *v;
                }
                session.pillar_heights = Some(arr);
                summary.pillars = true;
            }
        }

        for o in &self.slime {
            let obs = SlimeObservation {
                chunk_x: o.chunk_x,
                chunk_z: o.chunk_z,
                is_slime: o.is_slime,
            };
            if session.slime.contains(&obs) {
                summary.duplicates += 1;
            } else {
                session.slime.push(obs);
                summary.slime += 1;
            }
        }

        for o in &self.bedrock {
            let obs = BedrockObservation {
                x: o.x,
                y: o.y,
                z: o.z,
                is_bedrock: o.is_bedrock,
            };
            if session.bedrock.contains(&obs) {
                summary.duplicates += 1;
            } else {
                session.bedrock.push(obs);
                summary.bedrock += 1;
            }
        }

        for o in &self.eye_throws {
            if !o.x.is_finite() || !o.z.is_finite() || !o.yaw.is_finite() {
                summary
                    .warnings
                    .push("skipped an eye throw with a non-finite value".to_string());
                continue;
            }
            let t = crate::stronghold::Throw { x: o.x, z: o.z, yaw: o.yaw };
            // Exact-equality dedup is right here: these come from a file, not
            // from arithmetic, so a repeat is a byte-identical repeat.
            if session
                .eye_throws
                .iter()
                .any(|e| e.x == t.x && e.z == t.z && e.yaw == t.yaw)
            {
                summary.duplicates += 1;
            } else {
                session.eye_throws.push(t);
                summary.eye_throws += 1;
            }
        }

        for o in &self.structures {
            match parse_structure(&o.structure) {
                Ok(stype) => {
                    let already = session
                        .structures
                        .iter()
                        .any(|e| e.x == o.x && e.z == o.z && e.structure == stype);
                    if already {
                        summary.duplicates += 1;
                    } else {
                        session.structures.push(StructureObservation {
                            structure: stype,
                            x: o.x,
                            z: o.z,
                        });
                        summary.structures += 1;
                    }
                }
                Err(e) => summary.warnings.push(e.to_string()),
            }
        }

        for c in &self.candidates {
            if !session.candidates.contains(c) {
                session.candidates.push(*c);
                summary.candidates += 1;
            }
        }

        Ok(summary)
    }

    fn check_header(&self) -> Result<()> {
        if self.format != FORMAT_TAG {
            bail!(
                "this is not an mc-locate observation file (format tag was {:?}, expected {FORMAT_TAG:?})",
                self.format
            );
        }
        if self.version > FORMAT_VERSION {
            bail!(
                "the file is format version {} but this build only understands up to \
                 {FORMAT_VERSION} — update mc-locate",
                self.version
            );
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("could not serialise observations")
    }

    pub fn from_json(text: &str) -> Result<ObservationFile> {
        let parsed: ObservationFile =
            serde_json::from_str(text).context("could not parse the observation file as JSON")?;
        parsed.check_header()?;
        Ok(parsed)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        std::fs::write(path, self.to_json()?)
            .with_context(|| format!("could not write {}", path.display()))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<ObservationFile> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        ObservationFile::from_json(&text)
    }

    /// Total observations carried, for reporting.
    ///
    /// Counts observations only. Use [`ObservationFile::is_empty`] to decide
    /// whether there is anything worth saving — a session can hold a search
    /// box or a heading and no observations at all.
    pub fn count(&self) -> usize {
        self.slime.len() + self.bedrock.len() + self.structures.len() + self.eye_throws.len()
    }

    /// True when this file would carry nothing at all.
    ///
    /// Deliberately checks every field rather than just `count()`: coming
    /// straight from mode 11 with a portal-derived search box and nothing else
    /// is a perfectly normal thing to want to save.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
            && self.seed.is_none()
            && self.heading.is_none()
            && self.search_box.is_none()
            && self.candidates.is_empty()
            && self.pillar_heights.is_none()
            && self.minecraft_version.is_none()
            && self.unsupported_version.is_none()
    }
}

/// What an import actually changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportSummary {
    pub slime: usize,
    pub bedrock: usize,
    pub structures: usize,
    pub eye_throws: usize,
    pub candidates: usize,
    pub pillars: bool,
    pub duplicates: usize,
    pub warnings: Vec<String>,
}

impl ImportSummary {
    pub fn total(&self) -> usize {
        self.slime + self.bedrock + self.structures + self.eye_throws
    }
}

impl std::fmt::Display for ImportSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.slime > 0 {
            parts.push(format!("{} slime", self.slime));
        }
        if self.bedrock > 0 {
            parts.push(format!("{} bedrock", self.bedrock));
        }
        if self.structures > 0 {
            parts.push(format!("{} structure", self.structures));
        }
        if self.eye_throws > 0 {
            parts.push(format!("{} eye throw", self.eye_throws));
        }
        if self.candidates > 0 {
            parts.push(format!("{} candidate seed", self.candidates));
        }
        if self.pillars {
            parts.push("pillar heights".to_string());
        }
        if parts.is_empty() {
            write!(f, "nothing new")?;
        } else {
            write!(f, "added {}", parts.join(", "))?;
        }
        if self.duplicates > 0 {
            write!(f, " ({} duplicate(s) skipped)", self.duplicates)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_session() -> Session {
        Session {
            seed: Some(1234),
            hashed_seed: Some(8794265229978523055),
            biome_hash: Some(4978243150091466422),
            version: Some(Version::V1_21_1),
            heading: Some(137.5),
            search_box: Some(BBox::around(100, -200, 256)),
            candidates: vec![11, 22, 33],
            slime: vec![
                SlimeObservation { chunk_x: 1, chunk_z: 2, is_slime: true },
                SlimeObservation { chunk_x: -5, chunk_z: 9, is_slime: false },
            ],
            bedrock: vec![BedrockObservation { x: 11, y: 4, z: -97, is_bedrock: true }],
            structures: vec![StructureObservation {
                structure: StructureType::Desert_Pyramid,
                x: 1384,
                z: -2952,
            }],
            pillar_heights: Some([
                Some(76), None, Some(82), None, None, Some(94), None, None, None, Some(103),
            ]),
            eye_throws: vec![crate::stronghold::Throw { x: 8.0, z: -16.0, yaw: 33.25 }],
            newer_version: None,
        }
    }

    #[test]
    fn a_session_holding_only_a_search_box_is_not_empty() {
        // The mode 11 -> mode 13 path: a portal conversion leaves a search box
        // and nothing else, and that is worth saving.
        let session = Session {
            search_box: Some(BBox::around(800, 400, 1024)),
            ..Default::default()
        };
        let file = ObservationFile::from_session(&session, None);
        assert_eq!(file.count(), 0, "no observations, correctly");
        assert!(!file.is_empty(), "but the search box makes it worth saving");

        assert!(ObservationFile::from_session(&Session::default(), None).is_empty());

        let heading_only = Session {
            heading: Some(90.0),
            ..Default::default()
        };
        assert!(!ObservationFile::from_session(&heading_only, None).is_empty());
    }

    #[test]
    fn an_unsupported_version_survives_a_save_and_load() {
        // A 26.x session must not come back looking like it has no version at
        // all, or the next run silently offers generator-backed modes.
        let session = Session {
            newer_version: Some("26.2".to_string()),
            bedrock: vec![BedrockObservation { x: 1, y: 4, z: 2, is_bedrock: true }],
            ..Default::default()
        };
        let json = ObservationFile::from_session(&session, None).to_json().unwrap();
        assert!(json.contains("26.2"));

        let mut back = Session::default();
        ObservationFile::from_json(&json)
            .unwrap()
            .apply_to_session(&mut back, false)
            .unwrap();
        assert_eq!(back.newer_version.as_deref(), Some("26.2"));
        assert_eq!(back.version, None);
    }

    #[test]
    fn a_session_round_trips_through_json() {
        let original = sample_session();
        let json = ObservationFile::from_session(&original, Some("test".into()))
            .to_json()
            .unwrap();

        let mut restored = Session::default();
        let summary = ObservationFile::from_json(&json)
            .unwrap()
            .apply_to_session(&mut restored, false)
            .unwrap();

        assert!(summary.warnings.is_empty(), "unexpected warnings: {:?}", summary.warnings);
        assert_eq!(restored.seed, original.seed);
        assert_eq!(restored.version, original.version);
        assert_eq!(restored.heading, original.heading);
        assert_eq!(restored.search_box, original.search_box);
        assert_eq!(restored.candidates, original.candidates);
        assert_eq!(restored.slime, original.slime);
        assert_eq!(restored.bedrock, original.bedrock);
        assert_eq!(restored.pillar_heights, original.pillar_heights);
        assert_eq!(restored.structures.len(), 1);
        assert_eq!(restored.structures[0].structure, StructureType::Desert_Pyramid);
        assert_eq!((restored.structures[0].x, restored.structures[0].z), (1384, -2952));
    }

    #[test]
    fn importing_twice_does_not_double_anything() {
        // The mod case: re-importing a dump must be idempotent, or every
        // constraint silently doubles and the sieve slows for no reason.
        let file = ObservationFile::from_session(&sample_session(), None);
        let mut s = Session::default();
        let first = file.apply_to_session(&mut s, false).unwrap();
        assert_eq!(first.total(), 5);
        assert_eq!(first.duplicates, 0);

        let second = file.apply_to_session(&mut s, false).unwrap();
        assert_eq!(second.total(), 0, "nothing new the second time");
        assert_eq!(second.duplicates, 5);
        assert_eq!(s.slime.len(), 2);
        assert_eq!(s.bedrock.len(), 1);
        assert_eq!(s.structures.len(), 1);
        assert_eq!(s.eye_throws.len(), 1);
        assert_eq!(s.candidates.len(), 3);
    }

    #[test]
    fn an_import_does_not_clobber_a_seed_you_already_have() {
        let mut s = Session {
            seed: Some(999),
            version: Some(Version::V1_16_5),
            ..Default::default()
        };

        let file = ObservationFile::from_session(&sample_session(), None);
        file.apply_to_session(&mut s, false).unwrap();
        assert_eq!(s.seed, Some(999), "an import must not overwrite a known seed");
        assert_eq!(s.version, Some(Version::V1_16_5));

        // Unless explicitly asked to.
        file.apply_to_session(&mut s, true).unwrap();
        assert_eq!(s.seed, Some(1234));
        assert_eq!(s.version, Some(Version::V1_21_1));
    }

    #[test]
    fn a_minimal_hand_written_file_loads() {
        // What a mod author would write on day one: the header, and one kind
        // of observation. Everything else must default.
        let json = r#"{
            "format": "mc-locate-observations",
            "version": 1,
            "bedrock": [
                {"x": 11, "y": 4, "z": -97, "is_bedrock": true},
                {"x": 14, "y": 123, "z": -96, "is_bedrock": false}
            ]
        }"#;
        let mut s = Session::default();
        let summary = ObservationFile::from_json(json)
            .unwrap()
            .apply_to_session(&mut s, false)
            .unwrap();
        assert_eq!(summary.bedrock, 2);
        assert_eq!(s.bedrock.len(), 2);
        assert!(s.seed.is_none());
    }

    #[test]
    fn unknown_fields_from_a_newer_producer_are_ignored() {
        // Forward compatibility matters most for the mod, which will be
        // versioned separately and will drift ahead.
        let json = r#"{
            "format": "mc-locate-observations",
            "version": 1,
            "slime": [{"chunk_x": 3, "chunk_z": 4, "is_slime": true}],
            "some_future_field": {"nested": [1, 2, 3]},
            "another": "value"
        }"#;
        let mut s = Session::default();
        let summary = ObservationFile::from_json(json).unwrap().apply_to_session(&mut s, false).unwrap();
        assert_eq!(summary.slime, 1);
    }

    #[test]
    fn a_newer_format_version_is_refused_with_advice() {
        let json = r#"{"format": "mc-locate-observations", "version": 99}"#;
        let err = ObservationFile::from_json(json).unwrap_err().to_string();
        assert!(err.contains("update mc-locate"), "unhelpful: {err}");
    }

    #[test]
    fn the_wrong_kind_of_json_is_refused_clearly() {
        let err = ObservationFile::from_json(r#"{"format": "something-else", "version": 1}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an mc-locate observation file"), "unhelpful: {err}");

        assert!(ObservationFile::from_json("not json at all").is_err());
    }

    #[test]
    fn structure_names_accept_both_spellings() {
        assert_eq!(parse_structure("desert_pyramid").unwrap(), StructureType::Desert_Pyramid);
        assert_eq!(parse_structure("Desert pyramid").unwrap(), StructureType::Desert_Pyramid);
        // Both the cubiomes name and the menu label resolve, in any case and
        // with spaces or underscores — a hand-written file should not have to
        // guess which spelling the tool wants.
        assert_eq!(parse_structure("monument").unwrap(), StructureType::Monument);
        assert_eq!(parse_structure("Ocean monument").unwrap(), StructureType::Monument);
        assert_eq!(parse_structure("OCEAN_MONUMENT").unwrap(), StructureType::Monument);
        assert_eq!(parse_structure("  ocean-monument  ").unwrap(), StructureType::Monument);
        assert!(parse_structure("dragon lair").is_err());
    }

    #[test]
    fn a_bad_structure_name_warns_without_losing_the_rest() {
        let json = r#"{
            "format": "mc-locate-observations",
            "version": 1,
            "structures": [
                {"type": "desert_pyramid", "x": 100, "z": 200},
                {"type": "dragon_lair", "x": 0, "z": 0},
                {"type": "village", "x": 300, "z": 400}
            ]
        }"#;
        let mut s = Session::default();
        let summary = ObservationFile::from_json(json).unwrap().apply_to_session(&mut s, false).unwrap();
        assert_eq!(summary.structures, 2, "the two good ones should still land");
        assert_eq!(summary.warnings.len(), 1);
        assert!(summary.warnings[0].contains("dragon_lair"));
    }

    #[test]
    fn wrong_length_pillar_lists_are_rejected_not_padded() {
        let json = r#"{
            "format": "mc-locate-observations",
            "version": 1,
            "pillar_heights": [76, 79, 82]
        }"#;
        let mut s = Session::default();
        let summary = ObservationFile::from_json(json).unwrap().apply_to_session(&mut s, false).unwrap();
        assert!(s.pillar_heights.is_none(), "a short list must not be silently padded");
        assert_eq!(summary.warnings.len(), 1);
    }

    #[test]
    fn files_written_now_stay_loadable_by_this_reader() {
        let dir = std::env::temp_dir().join("mc-locate-obs-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("obs.json");

        ObservationFile::from_session(&sample_session(), Some("unit test".into()))
            .save(&path)
            .unwrap();
        let back = ObservationFile::load(&path).unwrap();
        assert_eq!(back.count(), 5);
        assert_eq!(back.source.as_deref(), Some("unit test"));

        // Empty collections are omitted, so the file stays readable by hand.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"format\""));
        assert!(!text.contains("\"warnings\""));
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod eye_throw_tests {
    use super::*;

    /// The mod computes yaw as `toDegrees(atan2(-dx, dz))`. This pins the same
    /// convention on the Rust side, so a sign flip in either place fails here
    /// rather than silently aiming the triangulation at a mirrored bearing.
    #[test]
    fn java_yaw_convention_matches_bearing_to() {
        let cases: [(f64, f64, f64); 4] = [
            // (dx, dz, expected yaw)
            (0.0, 1.0, 0.0),     // +Z is south, yaw 0
            (-1.0, 0.0, 90.0),   // -X is west, yaw 90
            (0.0, -1.0, 180.0),  // -Z is north
            (1.0, 0.0, -90.0),   // +X is east
        ];
        for (dx, dz, want) in cases {
            // atan2 returns -180 for due north where the fold in the mod's
            // yawOf returns +180; they are the same bearing, so compare with
            // angle_difference rather than raw subtraction.
            let java_side = (-dx).atan2(dz).to_degrees();
            assert!(
                crate::stronghold::angle_difference(java_side, want).abs() < 1e-9,
                "dx={dx} dz={dz}: java yaw {java_side} != {want}"
            );
            // And the same bearing derived from two points must agree.
            let rust_side = crate::stronghold::bearing_to(0.0, 0.0, dx * 100.0, dz * 100.0);
            assert!(
                crate::stronghold::angle_difference(rust_side, want).abs() < 1e-9,
                "bearing_to disagrees: {rust_side} vs {want}"
            );
        }
    }

    #[test]
    fn eye_throws_round_trip_and_dedup() {
        let json = r#"{
            "format": "mc-locate-observations", "version": 1,
            "eye_throws": [
                {"x": 12.5, "z": -40.25, "yaw": 41.7},
                {"x": 12.5, "z": -40.25, "yaw": 41.7},
                {"x": 900.0, "z": 100.0, "yaw": -12.0}
            ]
        }"#;
        let file: ObservationFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.eye_throws.len(), 3);

        let mut session = Session::default();
        let summary = file.apply_to_session(&mut session, false).unwrap();
        assert_eq!(summary.eye_throws, 2, "the exact repeat should be a duplicate");
        assert_eq!(summary.duplicates, 1);
        assert_eq!(session.eye_throws.len(), 2);
        assert!((session.eye_throws[0].yaw - 41.7).abs() < 1e-12);
    }

    #[test]
    fn non_finite_throws_cannot_enter_through_json() {
        // JSON has no infinity literal, and serde_json rejects an
        // out-of-range magnitude rather than rounding it to inf. So the
        // apply-time finite check is unreachable via a real file; this test
        // pins the reason, so a future switch to a lenient parser is noticed.
        let json = r#"{"format":"mc-locate-observations","version":1,
            "eye_throws":[{"x": 0.0, "z": 0.0, "yaw": 1e999}]}"#;
        assert!(serde_json::from_str::<ObservationFile>(json).is_err());

        // And a value built in memory is still filtered, which is what the
        // guard actually protects.
        let file = ObservationFile {
            eye_throws: vec![EyeThrowDto { x: 0.0, z: 0.0, yaw: f64::INFINITY }],
            ..ObservationFile::from_session(&Session::default(), None)
        };
        let mut session = Session::default();
        let summary = file.apply_to_session(&mut session, false).unwrap();
        assert_eq!(summary.eye_throws, 0);
        assert!(session.eye_throws.is_empty());
        assert_eq!(summary.warnings.len(), 1);
    }
}

#[cfg(test)]
mod mod_contract_tests {
    use super::*;

    /// A document shaped exactly like what the Fabric exporter writes — same
    /// field names, same nesting, including the singleplayer `seed`. This is the
    /// contract between the two halves; if the mod's emitter and this reader
    /// ever drift, this fails.
    #[test]
    fn exporter_document_imports_fully() {
        let json = r#"{
            "format": "mc-locate-observations",
            "version": 1,
            "source": "mc-locate-exporter",
            "seed": -4172144997902289642,
            "minecraft_version": "26.2",
            "bedrock": [
                {"x": 10, "y": 4, "z": -20, "is_bedrock": true},
                {"x": 14, "y": 4, "z": -20, "is_bedrock": false},
                {"x": 10, "y": 123, "z": -20, "is_bedrock": true}
            ],
            "slime": [
                {"chunk_x": 3, "chunk_z": -7, "is_slime": true}
            ],
            "structures": [
                {"type": "bastion_remnant", "x": 128, "z": 256}
            ],
            "eye_throws": [
                {"x": 12.5, "z": -40.25, "yaw": 41.7},
                {"x": 900.0, "z": 100.0, "yaw": -12.0}
            ],
            "pillar_heights": [76, null, 82, null, null, 94, null, null, null, 103]
        }"#;

        let file: ObservationFile = serde_json::from_str(json).expect("mod document must parse");
        assert_eq!(file.seed, Some(-4172144997902289642));

        let mut session = Session::default();
        let summary = file.apply_to_session(&mut session, false).unwrap();

        assert_eq!(session.seed, Some(-4172144997902289642), "seed must be imported");
        assert_eq!(session.bedrock.len(), 3);
        assert_eq!(session.slime.len(), 1);
        assert_eq!(session.structures.len(), 1);
        assert_eq!(session.eye_throws.len(), 2);
        assert!(session.pillar_heights.is_some());
        assert_eq!(summary.duplicates, 0);
    }
}

#[cfg(test)]
mod exporter_structure_names {
    /// The exact structure list the Fabric exporter offers in `/mclocate mark`.
    /// Must stay identical to KNOWN_STRUCTURES in ExporterClient.java — every
    /// name here must be one parse_structure accepts, or a marked structure is
    /// silently dropped on import.
    const EXPORTER_NAMES: &[&str] = &[
        "village", "desert_pyramid", "jungle_temple", "swamp_hut", "igloo",
        "ocean_ruin", "shipwreck", "ocean_monument", "woodland_mansion",
        "pillager_outpost", "ruined_portal", "ancient_city", "buried_treasure",
        "mineshaft", "trail_ruins", "trial_chambers", "nether_fortress",
        "bastion_remnant", "ruined_portal_nether", "end_city", "end_gateway",
    ];

    #[test]
    fn every_exporter_structure_name_parses() {
        for n in EXPORTER_NAMES {
            assert!(
                crate::observations::parse_structure(n).is_ok(),
                "exporter emits {n:?} but the CLI rejects it"
            );
        }
    }
}
