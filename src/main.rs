//! `mc-locate` — reverse-engineering Minecraft Java Edition seeds and
//! coordinates from limited in-game observations.
//!
//! Everything here is worldgen mathematics: Java's `java.util.Random` is a
//! 48-bit LCG and is trivially reversible, so a handful of observations is
//! often enough to recover a seed or a position. See the README for what each
//! mode needs and where that input comes from in game.

use anyhow::Result;
use mc_locate::session::Session;
use mc_locate::{
    bedrock, compass, logscrape, multicrack, ocr, portal, pose, slime, stronghold, structure,
    terrain, ui,
};

const MODES: &[(&str, fn(&mut Session) -> Result<()>)] = &[
    ("Nether Bedrock Toolkit", bedrock::run),
    ("Overworld Terrain Shape Matcher", terrain::run),
    ("F3 Screenshot OCR Reader", ocr::run),
    ("Slime Chunk Seed Cracker", slime::run),
    ("Camera Pose Estimator", pose::run),
    ("Structure-Relative Search Narrower", structure::run),
    ("Chat/Log Coordinate Scraper (live or file)", logscrape::run),
    ("Compass + Biome Triangulation Estimator", compass::run),
    ("Multi-Source Seed Cracker (combine everything)", multicrack::run),
    ("Stronghold Ring Triangulator (Bayesian)", stronghold::run),
    ("Nether <-> Overworld Portal Converter", portal::run),
];

fn banner() {
    println!();
    println!("\x1b[1;36m  mc-locate\x1b[0m \x1b[2mv{}\x1b[0m", env!("CARGO_PKG_VERSION"));
    println!("  \x1b[2mSeed and coordinate recovery for Minecraft Java Edition\x1b[0m");
}

fn main() -> Result<()> {
    banner();

    if !ui::is_interactive() {
        eprintln!(
            "\nmc-locate is an interactive tool and needs a terminal on stdin.\n\
             Run it directly rather than through a pipe."
        );
        std::process::exit(1);
    }

    let mut session = Session::default();

    loop {
        println!();
        let summary = session.summary();
        if !summary.is_empty() {
            println!("  \x1b[2mSession: {summary}\x1b[0m");
        }

        let mut items: Vec<String> = MODES
            .iter()
            .enumerate()
            .map(|(i, (name, _))| format!("{:>2}. {name}", i + 1))
            .collect();
        items.push(format!("{:>2}. Quit", MODES.len() + 1));

        let choice = match ui::select("Choose a mode", &items) {
            Ok(c) => c,
            // A Ctrl-C at the menu should exit quietly, not print a backtrace.
            Err(_) => {
                println!();
                return Ok(());
            }
        };

        if choice >= MODES.len() {
            println!("\n  Bye.");
            return Ok(());
        }

        // A mode failing is normal — bad input, an impossible search, a
        // version that does not support the feature. Report it and go back to
        // the menu rather than tearing the whole session down.
        if let Err(e) = (MODES[choice].1)(&mut session) {
            println!();
            ui::warn(&format!("{e}"));
            for cause in e.chain().skip(1) {
                ui::note(&format!("caused by: {cause}"));
            }
        }

        ui::pause();
    }
}
