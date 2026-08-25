//! The single registry of modes.
//!
//! The menu, the documentation browser and the tests all read this one list.
//! Keeping it here rather than in `main.rs` is what makes it possible to
//! assert that every mode has documentation — a mode added without a doc page
//! fails the test suite rather than shipping with a gap.

use anyhow::Result;

use crate::session::Session;
use crate::{
    advisor, bedrock, compass, decorator, docs, logscrape, multicrack, ocr, portal, pose,
    sessionfile, slime, stronghold, structure, terrain,
};

/// One entry in the main menu.
pub struct Mode {
    /// Menu name, without the number.
    pub name: &'static str,
    /// What the mode is for, in one line.
    pub summary: &'static str,
    /// The full write-up, embedded from `docs/`.
    pub doc: &'static str,
    pub run: fn(&mut Session) -> Result<()>,
}

pub const MODES: &[Mode] = &[
    Mode {
        name: "Nether Bedrock Toolkit",
        summary: "Find where you are from a bedrock pattern, or crack the seed from one",
        doc: include_str!("../docs/mode-01.md"),
        run: bedrock::run,
    },
    Mode {
        name: "Overworld Terrain Shape Matcher",
        summary: "Find where a transcribed biome or height patch fits in a known seed",
        doc: include_str!("../docs/mode-02.md"),
        run: terrain::run,
    },
    Mode {
        name: "F3 Screenshot OCR Reader",
        summary: "Read coordinates out of the F3 overlay in screenshots",
        doc: include_str!("../docs/mode-03.md"),
        run: ocr::run,
    },
    Mode {
        name: "Slime Chunk Seed Cracker",
        summary: "Recover a structure seed from confirmed slime chunks",
        doc: include_str!("../docs/mode-04.md"),
        run: slime::run,
    },
    Mode {
        name: "Camera Pose Estimator",
        summary: "Work out which way a screenshot was taken from",
        doc: include_str!("../docs/mode-05.md"),
        run: pose::run,
    },
    Mode {
        name: "Structure-Relative Search Narrower",
        summary: "Turn a known seed and a visible structure into a small search box",
        doc: include_str!("../docs/mode-06.md"),
        run: structure::run,
    },
    Mode {
        name: "Chat/Log Coordinate Scraper (live or file)",
        summary: "Scrape coordinates from logs and chat, from a file or live",
        doc: include_str!("../docs/mode-07.md"),
        run: logscrape::run,
    },
    Mode {
        name: "Compass + Biome Triangulation Estimator",
        summary: "Estimate position from a heading and the biomes you crossed",
        doc: include_str!("../docs/mode-08.md"),
        run: compass::run,
    },
    Mode {
        name: "Multi-Source Seed Cracker (combine everything)",
        summary: "Combine every kind of observation into one seed",
        doc: include_str!("../docs/mode-09.md"),
        run: multicrack::run,
    },
    Mode {
        name: "Stronghold Ring Triangulator (Bayesian)",
        summary: "Locate a stronghold from eye-of-ender throws",
        doc: include_str!("../docs/mode-10.md"),
        run: stronghold::run,
    },
    Mode {
        name: "Nether <-> Overworld Portal Converter",
        summary: "Convert between dimensions and get the area worth searching",
        doc: include_str!("../docs/mode-11.md"),
        run: portal::run,
    },
    Mode {
        name: "Observation Advisor (what to look at next)",
        summary: "Rank what to observe next, and explain why a candidate survives",
        doc: include_str!("../docs/mode-12.md"),
        run: advisor::run,
    },
    Mode {
        name: "Session & Observations (save / load / watch)",
        summary: "Save and load your work, and watch a screenshots folder",
        doc: include_str!("../docs/mode-13.md"),
        run: sessionfile::run,
    },
    Mode {
        name: "Documentation (how every mode works)",
        summary: "The full write-up for every mode, offline",
        doc: include_str!("../docs/mode-14.md"),
        run: docs::run,
    },
    Mode {
        name: "Decorator / Population-Seed Crack",
        summary: "Narrow the seed from a decorated feature (ores, plants, a dungeon)",
        doc: include_str!("../docs/mode-15.md"),
        run: decorator::run,
    },
];

/// Menu label including the number, e.g. `" 4. Slime Chunk Seed Cracker"`.
pub fn menu_label(index: usize) -> String {
    format!("{:>2}. {}", index + 1, MODES[index].name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_real_documentation() {
        // The reason the registry lives in the library at all: a mode added
        // without a doc page should fail here rather than ship with a gap.
        for (i, m) in MODES.iter().enumerate() {
            let doc = m.doc.trim();
            assert!(
                doc.len() > 600,
                "mode {} ({}) has only {} chars of documentation",
                i + 1,
                m.name,
                doc.len()
            );
            assert!(
                doc.starts_with("# "),
                "mode {} ({}) should open with a markdown heading",
                i + 1,
                m.name
            );
            assert!(
                !m.summary.is_empty() && m.summary.len() < 90,
                "mode {} needs a one-line summary",
                i + 1
            );
        }
    }

    #[test]
    fn documentation_names_the_mode_it_belongs_to() {
        // Guards against a copy-paste that points two entries at one file.
        for (i, m) in MODES.iter().enumerate() {
            let heading = m.doc.lines().next().unwrap_or_default();
            assert!(
                heading.contains(&format!("Mode {}", i + 1)),
                "mode {} ({}) is documented by a page headed {heading:?}",
                i + 1,
                m.name
            );
        }
    }

    #[test]
    fn menu_labels_are_numbered_from_one() {
        assert!(menu_label(0).starts_with(" 1. "));
        assert_eq!(menu_label(0), " 1. Nether Bedrock Toolkit");
        assert!(menu_label(MODES.len() - 1).starts_with(&format!("{}. ", MODES.len())));
    }

    #[test]
    fn the_registry_is_not_accidentally_duplicated() {
        let mut names: Vec<&str> = MODES.iter().map(|m| m.name).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate mode names in the registry");
    }
}
