//! `mc-locate` — reverse-engineering Minecraft Java Edition seeds and
//! coordinates from limited in-game observations.
//!
//! Everything here is worldgen mathematics: Java's `java.util.Random` is a
//! 48-bit LCG and is trivially reversible, so a handful of observations is
//! often enough to recover a seed or a position. Mode 14 documents every mode
//! in full, offline.

use anyhow::Result;
use mc_locate::modes::{MODES, menu_label};
use mc_locate::session::Session;
use mc_locate::{theme, ui};

fn banner() {
    println!();
    print!("{}", theme::banner(env!("CARGO_PKG_VERSION")));
    println!(
        "  {}",
        theme::dim()
            .apply_to("New here? Mode 14 explains every mode; mode 11 needs nothing to try.")
    );
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
        let bar = theme::status_bar(&session.status_fields());
        if !bar.is_empty() {
            println!("{bar}");
            println!("{}", theme::rule());
        }

        let mut items: Vec<String> = (0..MODES.len()).map(menu_label).collect();
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
            println!("\n  {}", theme::dim().apply_to("Bye."));
            return Ok(());
        }

        // A mode failing is normal — bad input, an impossible search, a
        // version that does not support the feature. Report it and go back to
        // the menu rather than tearing the whole session down.
        if let Err(e) = (MODES[choice].run)(&mut session) {
            println!();
            ui::warn(&format!("{e}"));
            for cause in e.chain().skip(1) {
                ui::note(&format!("caused by: {cause}"));
            }
        }

        ui::pause();
    }
}
