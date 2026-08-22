//! Mode 3 — reading coordinates straight off an F3 debug-screen screenshot.
//!
//! People rarely write their coordinates down, but they do take screenshots.
//! The F3 overlay is the densest source of position information Minecraft ever
//! puts on screen, so pulling the XYZ block out of a folder of old screenshots
//! is often the cheapest way to seed the search modes with a real location.
//!
//! Two things make this less trivial than "run Tesseract on it":
//!
//! * The overlay is light text drawn straight over the world, with no solid
//!   background, at whatever size the GUI scale happens to be. Tesseract wants
//!   large dark text on a light background, so the image has to be cropped,
//!   binarised and usually inverted first — and since we cannot know which
//!   polarity a given screenshot needs, we try both and keep whichever parses.
//! * OCR of small text is unreliable in specific, predictable ways (`O` for
//!   `0`, `l` for `1`, `S` for `5`). Rather than trust one reading, the parser
//!   accepts all three coordinate lines the overlay draws and prefers the
//!   `Block:` line, whose integers survive OCR far better than the `XYZ:`
//!   line's five decimal places.
//!
//! Tesseract itself is a system library, so it lives behind the optional `ocr`
//! cargo feature; everything above it — cropping, thresholding, parsing — is
//! plain Rust and is compiled and tested either way. Without the feature the
//! mode still runs, explains how to enable it, and takes the coordinates by
//! hand.

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage};
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

use crate::session::{BBox, Session};
use crate::ui;

/// Image extensions accepted in folder mode.
pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "bmp", "webp"];

pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

// ---------------------------------------------------------------------------
// Cropping
// ---------------------------------------------------------------------------

/// Which part of the screenshot to hand to Tesseract.
///
/// The overlay's position and size move with the window resolution and the GUI
/// scale, so this has to be configurable. [`CropSpec::Fraction`] is the form
/// that survives a resolution change halfway through a folder of screenshots,
/// which pixel rectangles do not.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CropSpec {
    /// The whole image.
    Full,
    /// The top-left quadrant, which is where vanilla draws the XYZ block.
    #[default]
    TopLeftQuadrant,
    /// An explicit pixel rectangle.
    Pixels { x: u32, y: u32, w: u32, h: u32 },
    /// A rectangle given as fractions (0.0–1.0) of the image's width and
    /// height.
    Fraction { x: f64, y: f64, w: f64, h: f64 },
}

impl CropSpec {
    /// Resolves to `(x, y, w, h)` in pixels, clamped so the rectangle always
    /// lies inside a `width` x `height` image.
    ///
    /// Clamping rather than failing is deliberate: a rectangle measured on a
    /// 1920x1080 screenshot should still do something sensible when it meets a
    /// 1280x720 one, and an empty result is reported by the caller anyway.
    pub fn resolve(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let (x, y, w, h) = match *self {
            CropSpec::Full => (0, 0, width, height),
            CropSpec::TopLeftQuadrant => (0, 0, width.div_ceil(2), height.div_ceil(2)),
            CropSpec::Pixels { x, y, w, h } => (x, y, w, h),
            CropSpec::Fraction { x, y, w, h } => {
                let px = |f: f64, span: u32| {
                    if f.is_finite() {
                        (f.clamp(0.0, 1.0) * f64::from(span)).round() as u32
                    } else {
                        0
                    }
                };
                (px(x, width), px(y, height), px(w, width), px(h, height))
            }
        };
        let x = x.min(width);
        let y = y.min(height);
        (x, y, w.min(width - x), h.min(height - y))
    }
}

impl std::fmt::Display for CropSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CropSpec::Full => write!(f, "the whole image"),
            CropSpec::TopLeftQuadrant => write!(f, "the top-left quadrant"),
            CropSpec::Pixels { x, y, w, h } => write!(f, "pixels {w}x{h} at ({x}, {y})"),
            CropSpec::Fraction { x, y, w, h } => {
                write!(f, "fractions {w:.3}x{h:.3} at ({x:.3}, {y:.3})")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Preprocessing
// ---------------------------------------------------------------------------

/// How to turn a crop of a screenshot into something Tesseract can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preprocess {
    /// Flip the threshold, so light text becomes black on white. The F3
    /// overlay is light text on a dark world, and Tesseract is trained on the
    /// opposite, so this is on by default.
    pub invert: bool,
    /// Luma cutoff. The overlay's text is close to white and the world behind
    /// it is usually much darker, so a fairly high cutoff throws most of the
    /// scenery away with it.
    pub threshold: u8,
    /// Integer upscale applied before thresholding. Tesseract degrades badly
    /// below roughly 20px glyph height, which is exactly where a GUI-scale-2
    /// overlay on a 1080p screenshot sits. Clamped to `1..=8`.
    pub scale: u32,
}

impl Default for Preprocess {
    fn default() -> Self {
        Preprocess {
            invert: true,
            threshold: 140,
            scale: 2,
        }
    }
}

impl Preprocess {
    /// The same settings with the threshold polarity flipped — the "second
    /// opinion" for a screenshot taken against a bright sky or snow, where the
    /// overlay is the darker of the two.
    pub fn flipped(self) -> Preprocess {
        Preprocess {
            invert: !self.invert,
            ..self
        }
    }
}

/// Crop, desaturate, upscale and binarise, in that order.
///
/// The upscale happens *before* the threshold on purpose: resampling an
/// already-binary image just reintroduces the grey edges the threshold existed
/// to remove, whereas Lanczos3 on the greyscale gives the threshold more
/// detail to cut against.
pub fn preprocess(img: &DynamicImage, crop: CropSpec, opts: Preprocess) -> GrayImage {
    let (x, y, w, h) = crop.resolve(img.width(), img.height());
    if w == 0 || h == 0 {
        return GrayImage::new(0, 0);
    }

    let gray = img.crop_imm(x, y, w, h).to_luma8();

    let scale = opts.scale.clamp(1, 8);
    let mut out = if scale > 1 {
        image::imageops::resize(
            &gray,
            w.saturating_mul(scale),
            h.saturating_mul(scale),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        gray
    };

    for pixel in out.pixels_mut() {
        let lit = pixel.0[0] >= opts.threshold;
        pixel.0[0] = if lit != opts.invert { 255 } else { 0 };
    }
    out
}

/// PNG bytes for a processed image, which is the form Tesseract's
/// `set_image_from_mem` wants.
pub fn encode_png(img: &GrayImage) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .context("could not encode the processed crop as PNG")?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Parsing the F3 lines
// ---------------------------------------------------------------------------

/// A position read out of an F3 overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct F3Coords {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Which line the numbers came from: `"block"`, `"xyz"`, `"chunk"`, or
    /// `"manual"` when the user typed them in. Worth carrying around because
    /// `"chunk"` is only accurate to the chunk, and the CSV should say so.
    pub source: &'static str,
}

impl F3Coords {
    /// The block the player is standing in, i.e. each coordinate floored.
    /// Flooring rather than rounding is what Minecraft itself does, which is
    /// why `XYZ: -789.012` shows up as `Block: -790`.
    pub fn block_pos(&self) -> (i32, i32, i32) {
        (
            self.x.floor() as i32,
            self.y.floor() as i32,
            self.z.floor() as i32,
        )
    }
}

impl std::fmt::Display for F3Coords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "X {:.3} / Y {:.3} / Z {:.3} (from the {} line)",
            self.x, self.y, self.z, self.source
        )
    }
}

/// One number, as OCR might have mangled it: an optional sign followed by
/// digits and the letters Tesseract most often swaps digits for.
///
/// Deliberately permissive — a token that turns out not to be a number is
/// thrown away by [`repair_number`], which is a cheaper way to be tolerant
/// than trying to encode every misreading in the pattern itself.
/// Characters that can legitimately appear inside an OCR'd number: the digits
/// themselves, a decimal separator, and every glyph [`repair_number`] knows how
/// to map back to a digit.
///
/// This must stay in step with `repair_number`'s match arms. It did not, once:
/// `@` was added to the repair table but not here, so the regex stopped
/// matching at the `@` in `-1290.50@` and the repair never got a chance to run.
/// [`tests::the_number_class_and_the_repair_table_agree`] now enforces it.
macro_rules! number_chars {
    () => {
        "0-9OoIilSsBZzGTQD@|!.,"
    };
}

const NUMBER: &str = concat!(r"[-+~–—−]?[", number_chars!(), "]+");

#[cfg(test)]
const NUMBER_CHARS: &str = number_chars!();

/// Whitespace or a comma between two numbers on the `Block:`/`Chunk:` lines.
const GAP: &str = r"[\s,;]+";

fn xyz_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // `XYZ: 123.456 / 64.00000 / -789.012`, tolerating stray spaces around
        // the slashes and a slash read as a pipe or backslash.
        build(&format!(
            r"(?i)\bx\s*y\s*z\b\s*[:;.,]?\s*({NUMBER})\s*[/\\|]\s*({NUMBER})\s*[/\\|]\s*({NUMBER})"
        ))
    })
}

fn block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(&format!(r"(?i)\bblock\b\s*[:;.,]?\s*({NUMBER}){GAP}({NUMBER}){GAP}({NUMBER})")))
}

fn chunk_in_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // `Chunk: 8 5 3 in 7 4 -50` — the offset within the chunk followed by
        // the chunk itself, which together pin the block down exactly.
        build(&format!(
            r"(?i)\bchunk\b\s*[:;.,]?\s*({NUMBER}){GAP}({NUMBER}){GAP}({NUMBER})\s+in\s+({NUMBER}){GAP}({NUMBER}){GAP}({NUMBER})"
        ))
    })
}

fn chunk_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| build(&format!(r"(?i)\bchunk\b\s*[:;.,]?\s*({NUMBER}){GAP}({NUMBER}){GAP}({NUMBER})")))
}

/// Compiles a pattern that is a compile-time constant of this module, so a
/// failure here is a bug rather than bad input.
fn build(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the F3 patterns are valid regexes")
}

/// Undoes the substitutions Tesseract makes most often on small light text,
/// then parses.
///
/// Returning `None` for anything that is not a number afterwards is what keeps
/// the permissive patterns above honest: a false positive simply fails to
/// parse and the next candidate line is tried instead.
pub fn repair_number(token: &str) -> Option<f64> {
    // A full stop at the end is punctuation or noise; one at the front is a
    // leading decimal point and has to stay.
    let token = token.trim().trim_end_matches([',', '.']);
    if token.is_empty() {
        return None;
    }

    let mut out = String::with_capacity(token.len());
    for (i, ch) in token.chars().enumerate() {
        let mapped = match ch {
            '0'..='9' | '.' => ch,
            // '@' for '0' is the single most common misread on this overlay:
            // Tesseract produced "-1290.50@", "6@ fps" and "-5@" from a
            // rendered F3 panel in `backend_tests`. 'Q' and 'D' go the same
            // way on rounder faces.
            'O' | 'o' | '@' | 'Q' | 'D' => '0',
            'I' | 'i' | 'l' | '|' | '!' => '1',
            'Z' | 'z' => '2',
            'S' | 's' => '5',
            'G' => '6',
            'T' => '7',
            'B' => '8',
            // Either a decimal comma or a full stop read as one; Minecraft
            // never groups thousands, so it is always a decimal point.
            ',' => '.',
            // Minus signs come back as any number of dashes.
            '-' | '–' | '—' | '−' | '~' if i == 0 => '-',
            '+' if i == 0 => continue,
            _ => return None,
        };
        out.push(mapped);
    }

    let value: f64 = out.parse().ok()?;
    value.is_finite().then_some(value)
}

/// The world border is at 30 million blocks and the build limit is nowhere
/// near 4096, so anything outside these is a misread rather than a position.
fn plausible(x: f64, y: f64, z: f64) -> bool {
    x.abs() <= 30_000_000.0 && z.abs() <= 30_000_000.0 && (-4096.0..=4096.0).contains(&y)
}

fn coords(x: f64, y: f64, z: f64, source: &'static str) -> Option<F3Coords> {
    plausible(x, y, z).then_some(F3Coords { x, y, z, source })
}

/// Pulls `count` numbers out of the first match of `re` that parses cleanly.
fn numbers(re: &Regex, line: &str, count: usize) -> Option<Vec<f64>> {
    re.captures_iter(line).find_map(|caps| {
        let mut found = Vec::with_capacity(count);
        for i in 1..=count {
            found.push(repair_number(caps.get(i)?.as_str())?);
        }
        Some(found)
    })
}

/// Reads a position out of whatever text the OCR produced.
///
/// All three coordinate lines the overlay draws are understood, and the
/// `Block:` line wins when more than one is readable: its integers survive
/// small-text OCR much better than the `XYZ:` line's decimals, where a single
/// misread digit after the point is invisible in the output but wrong.
///
/// Returns `None` rather than a guess when nothing parses — a plausible-looking
/// wrong coordinate is worse than no coordinate at all, because every mode
/// downstream would then search the wrong place.
pub fn parse_f3_text(text: &str) -> Option<F3Coords> {
    let mut from_xyz = None;
    let mut from_block = None;
    let mut from_chunk = None;

    for line in text.lines() {
        // "Targeted Block" is wherever the crosshair happens to be pointing,
        // not where the player is standing; matching it would quietly answer a
        // different question.
        if line.to_ascii_lowercase().contains("target") {
            continue;
        }

        if from_block.is_none()
            && let Some(n) = numbers(block_re(), line, 3)
        {
            from_block = coords(n[0], n[1], n[2], "block");
        }

        if from_xyz.is_none()
            && let Some(n) = numbers(xyz_re(), line, 3)
        {
            from_xyz = coords(n[0], n[1], n[2], "xyz");
        }

        if from_chunk.is_none() {
            from_chunk = if let Some(n) = numbers(chunk_in_re(), line, 6) {
                // Offset within the chunk plus the chunk's own coordinates.
                coords(
                    n[3] * 16.0 + n[0],
                    n[4] * 16.0 + n[1],
                    n[5] * 16.0 + n[2],
                    "chunk",
                )
            } else if let Some(n) = numbers(chunk_re(), line, 3) {
                // Only the chunk, so the best we can honestly say is its
                // centre — good to ±8 blocks, which is plenty for a search box.
                coords(
                    n[0] * 16.0 + 8.0,
                    n[1] * 16.0 + 8.0,
                    n[2] * 16.0 + 8.0,
                    "chunk",
                )
            } else {
                None
            };
        }
    }

    from_block.or(from_xyz).or(from_chunk)
}

// ---------------------------------------------------------------------------
// Session hand-off, shared by both builds
// ---------------------------------------------------------------------------

/// Hands a coordinate on to the modes that search an area, as a box centred on
/// it. Modes 2 and 6 pick this up instead of asking for it again.
fn store_search_box(session: &mut Session, found: &F3Coords) -> Result<()> {
    let radius: i32 = ui::input_default("Search radius around it (blocks)", 128)?;
    let (x, _, z) = found.block_pos();
    let bbox = BBox::around(x, z, radius.max(0));
    session.search_box = Some(bbox);
    ui::success(&format!("Search box stored: {bbox}"));
    Ok(())
}

/// Types the coordinates in by hand.
///
/// The fallback both when the binary has no OCR support and when OCR misreads
/// a digit the user can plainly see on screen.
pub fn manual_entry(session: &mut Session) -> Result<()> {
    let x: f64 = ui::input("X")?;
    let y: f64 = ui::input("Y")?;
    let z: f64 = ui::input("Z")?;
    let found = F3Coords {
        x,
        y,
        z,
        source: "manual",
    };
    ui::success(&format!("{found}"));
    store_search_box(session, &found)
}

// ---------------------------------------------------------------------------
// With OCR support
// ---------------------------------------------------------------------------

#[cfg(feature = "ocr")]
mod backend {
    use super::*;
    use anyhow::{anyhow, bail};
    use std::path::PathBuf;

    /// A Tesseract instance, created once and reused across a folder. Starting
    /// it up reads the language data from disk, which is not something to do
    /// two hundred times in a row.
    pub struct Ocr {
        tess: leptess::LepTess,
    }

    impl Ocr {
        pub fn new() -> Result<Ocr> {
            let tess = leptess::LepTess::new(None, "eng").map_err(|e| {
                anyhow!(
                    "could not start Tesseract ({e}). It needs the English training data: \
                     `brew install tesseract-lang`, or point TESSDATA_PREFIX at a tessdata \
                     directory."
                )
            })?;
            Ok(Ocr { tess })
        }

        /// OCRs one already-processed PNG.
        pub fn text(&mut self, png: &[u8]) -> Result<String> {
            self.tess
                .set_image_from_mem(png)
                .map_err(|e| anyhow!("Tesseract would not accept the processed crop: {e}"))?;
            // Screenshots carry no DPI, and Tesseract prints a warning for
            // every page that does not have one.
            self.tess.set_fallback_source_resolution(300);
            self.tess
                .get_utf8_text()
                .map_err(|e| anyhow!("Tesseract returned text that was not UTF-8: {e}"))
        }
    }

    pub fn run(session: &mut Session) -> Result<()> {
        ui::header("Mode 3 — F3 screenshot OCR");
        ui::note("Reads the XYZ block out of an F3 debug screenshot, or a folder of them.");

        let raw: String = ui::input("Screenshot file, or a folder of screenshots")?;
        let path = resolve_path(&raw);
        let meta = std::fs::metadata(&path)
            .with_context(|| format!("could not open {}", path.display()))?;

        let crop = prompt_crop()?;
        let opts = prompt_preprocess()?;
        let both = ui::confirm(
            "Also try the opposite threshold polarity when the first reading does not parse?",
            true,
        )?;

        let mut ocr = Ocr::new()?;

        if meta.is_dir() {
            run_folder(session, &mut ocr, &path, crop, opts, both)
        } else {
            run_single(session, &mut ocr, &path, crop, opts, both)
        }
    }

    /// Trims the quotes and `~` a path picks up on its way through a shell or a
    /// drag-and-drop.
    fn resolve_path(raw: &str) -> PathBuf {
        let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
        if let Some(rest) = trimmed.strip_prefix("~/")
            && let Ok(home) = std::env::var("HOME")
        {
            return PathBuf::from(home).join(rest);
        }
        PathBuf::from(trimmed)
    }

    fn prompt_crop() -> Result<CropSpec> {
        ui::note(
            "The overlay's position depends on resolution and GUI scale, so the crop is \
             configurable. The default is the top-left quadrant, which is where vanilla \
             draws the XYZ block.",
        );
        let choice = ui::select_str(
            "Which part of the image holds the F3 text?",
            &[
                "Top-left quadrant (default)",
                "The whole image",
                "An explicit pixel rectangle",
                "A fractional rectangle (survives a resolution change)",
            ],
        )?;

        let crop = match choice {
            0 => CropSpec::TopLeftQuadrant,
            1 => CropSpec::Full,
            2 => CropSpec::Pixels {
                x: ui::input_default("Left edge (px)", 0u32)?,
                y: ui::input_default("Top edge (px)", 0u32)?,
                w: ui::input_default("Width (px)", 960u32)?,
                h: ui::input_default("Height (px)", 540u32)?,
            },
            _ => CropSpec::Fraction {
                x: ui::input_default("Left edge (0.0–1.0)", 0.0f64)?,
                y: ui::input_default("Top edge (0.0–1.0)", 0.0f64)?,
                w: ui::input_default("Width (0.0–1.0)", 0.5f64)?,
                h: ui::input_default("Height (0.0–1.0)", 0.5f64)?,
            },
        };
        ui::note(&format!("Cropping to {crop}."));
        Ok(crop)
    }

    fn prompt_preprocess() -> Result<Preprocess> {
        let default = Preprocess::default();
        ui::note(
            "The overlay is light text over a darker world and Tesseract expects the \
             opposite, so the threshold is inverted by default.",
        );
        let invert = ui::confirm("Invert (light text becomes black on white)?", default.invert)?;
        let threshold: u8 = ui::input_default("Luma threshold (0–255)", default.threshold)?;
        let scale: u32 = ui::input_default(
            "Upscale before thresholding (1–8; Tesseract is poor below ~20px glyphs)",
            default.scale,
        )?;
        Ok(Preprocess {
            invert,
            threshold,
            scale: scale.clamp(1, 8),
        })
    }

    fn load_image(path: &Path) -> Result<DynamicImage> {
        // Guess from the contents rather than trusting the extension: plenty of
        // screenshots have been renamed to .png on their way through a chat app.
        image::ImageReader::open(path)
            .with_context(|| format!("could not open {}", path.display()))?
            .with_guessed_format()
            .with_context(|| format!("could not identify the image format of {}", path.display()))?
            .decode()
            .with_context(|| format!("could not decode {}", path.display()))
    }

    /// OCRs one image, optionally taking a second reading with the threshold
    /// flipped. Returns the text of the last attempt as well, since that is
    /// what the user needs to see when nothing parsed.
    fn read_one(
        ocr: &mut Ocr,
        img: &DynamicImage,
        crop: CropSpec,
        opts: Preprocess,
        both: bool,
    ) -> Result<(Option<F3Coords>, String)> {
        let attempts = if both {
            vec![opts, opts.flipped()]
        } else {
            vec![opts]
        };

        let mut last = String::new();
        for attempt in attempts {
            let processed = preprocess(img, crop, attempt);
            if processed.width() == 0 || processed.height() == 0 {
                bail!(
                    "the crop region is empty for this {}x{} image",
                    img.width(),
                    img.height()
                );
            }
            let png = encode_png(&processed)?;
            let text = ocr.text(&png)?;
            if let Some(found) = parse_f3_text(&text) {
                return Ok((Some(found), text));
            }
            last = text;
        }
        Ok((None, last))
    }

    fn run_single(
        session: &mut Session,
        ocr: &mut Ocr,
        path: &Path,
        crop: CropSpec,
        opts: Preprocess,
        both: bool,
    ) -> Result<()> {
        let spinner = ui::spinner("reading the screenshot");
        let img = load_image(path)?;
        let (found, text) = read_one(ocr, &img, crop, opts, both)?;
        spinner.finish_and_clear();

        match found {
            Some(found) => {
                ui::success(&format!("{}: {found}", display_name(path)));
                let (bx, by, bz) = found.block_pos();
                ui::note(&format!("Block position: {bx} {by} {bz}"));
                if ui::confirm("Store this as the session's search box?", true)? {
                    store_search_box(session, &found)?;
                }
            }
            None => {
                ui::warn("No coordinates in the OCR output.");
                show_text(&text);
                ui::note(
                    "Try a tighter crop, a different threshold, or a larger upscale; the \
                     overlay needs to end up as clean black text on white.",
                );
                if ui::is_interactive()
                    && ui::confirm("Type the coordinates in by hand instead?", true)?
                {
                    manual_entry(session)?;
                }
            }
        }
        Ok(())
    }

    fn run_folder(
        session: &mut Session,
        ocr: &mut Ocr,
        dir: &Path,
        crop: CropSpec,
        opts: Preprocess,
        both: bool,
    ) -> Result<()> {
        let files = image_files(dir)?;
        if files.is_empty() {
            bail!(
                "no {} files in {}",
                IMAGE_EXTENSIONS.join("/"),
                dir.display()
            );
        }
        ui::note(&format!("{} image(s) to read.", files.len()));

        let out = resolve_path(&ui::input_default(
            "CSV output path",
            "f3_coords.csv".to_string(),
        )?);
        let mut writer = csv::Writer::from_path(&out)
            .with_context(|| format!("could not write to {}", out.display()))?;
        writer.write_record(["file", "x", "y", "z", "source"])?;

        let progress = ui::progress_bar(files.len() as u64, "screenshots");
        let mut last: Option<F3Coords> = None;
        let mut read = 0usize;
        let mut problems: Vec<String> = Vec::new();

        for file in &files {
            let name = display_name(file);
            let outcome = load_image(file).and_then(|img| read_one(ocr, &img, crop, opts, both));
            match outcome {
                Ok((Some(found), _)) => {
                    writer.write_record([
                        &name,
                        &found.x.to_string(),
                        &found.y.to_string(),
                        &found.z.to_string(),
                        &found.source.to_string(),
                    ])?;
                    read += 1;
                    last = Some(found);
                }
                Ok((None, _)) => problems.push(format!("{name}: nothing parsed")),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
            progress.inc(1);
        }
        progress.finish_and_clear();
        writer.flush().context("could not flush the CSV")?;

        ui::success(&format!(
            "Read {read} of {} screenshot(s); wrote {}",
            files.len(),
            out.display()
        ));
        if !problems.is_empty() {
            ui::warn(&format!(
                "{} screenshot(s) produced nothing and were left out of the CSV:",
                problems.len()
            ));
            for problem in problems.iter().take(10) {
                ui::note(problem);
            }
            if problems.len() > 10 {
                ui::note(&format!("… and {} more", problems.len() - 10));
            }
        }

        if let Some(found) = last {
            ui::note(&format!("Last coordinate read: {found}"));
            if ui::confirm("Store it as the session's search box?", true)? {
                store_search_box(session, &found)?;
            }
        }
        Ok(())
    }

    fn image_files(dir: &Path) -> Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .with_context(|| format!("could not list {}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && has_image_extension(p))
            .collect();
        // Screenshot names are timestamps, so sorting them puts the batch in
        // the order the screenshots were taken.
        files.sort();
        Ok(files)
    }

    fn display_name(path: &Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    /// Shows the first few lines of OCR output, which is usually enough to see
    /// whether the crop was wrong or the threshold was.
    fn show_text(text: &str) {
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            ui::note("(Tesseract found no text at all in that crop)");
            return;
        }
        ui::note("OCR output:");
        for line in lines.iter().take(8) {
            ui::note(&format!("  {line}"));
        }
    }
}

#[cfg(feature = "ocr")]
pub fn run(session: &mut Session) -> Result<()> {
    backend::run(session)
}

// ---------------------------------------------------------------------------
// Without OCR support
// ---------------------------------------------------------------------------

/// Explains that this binary cannot OCR anything, and falls back to typing the
/// coordinates in.
///
/// Building without Tesseract is the normal case rather than an error: it is a
/// system library, and every other mode in the tool works fine without it.
#[cfg(not(feature = "ocr"))]
pub fn run(session: &mut Session) -> Result<()> {
    ui::header("Mode 3 — F3 screenshot OCR");
    ui::warn("This binary was built without OCR support, so it cannot read a screenshot.");
    ui::note("Tesseract and Leptonica are system libraries, so they sit behind an optional");
    ui::note("cargo feature rather than being required by every build. To enable them:");
    println!();
    println!("      brew install tesseract leptonica pkg-config");
    println!("      cargo build --release --features ocr");
    println!();
    ui::note("Everything else in this mode — cropping, thresholding, parsing the F3 lines —");
    ui::note("is already compiled in; only the call into Tesseract is missing.");

    if !ui::is_interactive() {
        return Ok(());
    }
    if ui::confirm("Read the coordinates off the screenshot yourself and type them in?", true)? {
        manual_entry(session)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parsing ------------------------------------------------------------

    #[test]
    fn the_number_class_and_the_repair_table_agree() {
        // Every character the regex will swallow must be one the repair table
        // can turn into a digit — otherwise the regex matches a token that
        // then fails to parse, and the reading is silently dropped. (The
        // reverse drift is the one that actually bit: a repairable character
        // missing from the class means the regex stops early and never
        // produces the token at all.)
        let mut chars: Vec<char> = Vec::new();
        let mut it = NUMBER_CHARS.chars().peekable();
        while let Some(c) = it.next() {
            if it.peek() == Some(&'-') {
                // a range like `0-9`
                it.next();
                let end = it.next().expect("range needs an end");
                for c in c..=end {
                    chars.push(c);
                }
            } else {
                chars.push(c);
            }
        }
        assert!(chars.contains(&'@'), "the class should cover the '@' misread");

        for c in chars {
            if c == '.' || c == ',' {
                continue;
            }
            let token: String = format!("1{c}");
            assert!(
                repair_number(&token).is_some(),
                "regex accepts {c:?} inside a number but repair_number rejects {token:?}"
            );
        }
    }

    #[test]
    fn repairs_the_misreads_tesseract_actually_makes() {
        // Every one of these came out of a real OCR run over a rendered F3
        // panel, not from a list of plausible-looking substitutions.
        assert_eq!(repair_number("-1290.50@"), Some(-1290.50));
        assert_eq!(repair_number("6@"), Some(60.0));
        assert_eq!(repair_number("-5@"), Some(-50.0));
        assert_eq!(repair_number("l23"), Some(123.0));
        assert_eq!(repair_number("|23"), Some(123.0));
        assert_eq!(repair_number("64.OO"), Some(64.0));
        // Still refuses things that are not numbers at all.
        assert_eq!(repair_number("north"), None);
        assert_eq!(repair_number(""), None);
    }

    #[test]
    fn reads_the_real_xyz_line() {
        let found = parse_f3_text("XYZ: 123.456 / 64.00000 / -789.012").unwrap();
        assert_eq!(found.source, "xyz");
        assert!((found.x - 123.456).abs() < 1e-9);
        assert!((found.y - 64.0).abs() < 1e-9);
        assert!((found.z - -789.012).abs() < 1e-9);
    }

    #[test]
    fn reads_the_block_line() {
        let found = parse_f3_text("Block: 123 64 -789").unwrap();
        assert_eq!(found.source, "block");
        assert_eq!((found.x, found.y, found.z), (123.0, 64.0, -789.0));
    }

    #[test]
    fn reads_the_chunk_line_as_the_chunk_centre() {
        let found = parse_f3_text("Chunk: 11 4 -50").unwrap();
        assert_eq!(found.source, "chunk");
        // Chunk 11 spans X 176..191, so its centre is 184.
        assert_eq!((found.x, found.y, found.z), (184.0, 72.0, -792.0));
    }

    #[test]
    fn the_chunk_line_with_an_offset_is_exact() {
        // "8 5 3 in 7 4 -50" — offset inside the chunk, then the chunk itself.
        let found = parse_f3_text("Chunk: 8 5 3 in 7 4 -50").unwrap();
        assert_eq!(found.source, "chunk");
        assert_eq!((found.x, found.y, found.z), (120.0, 69.0, -797.0));
    }

    #[test]
    fn the_block_line_wins_over_the_others() {
        // Integers survive OCR better than five decimal places, so a full F3
        // dump should be read off the Block line even though XYZ comes first.
        let text = "\
Minecraft 1.21.1 (1.21.1/vanilla)
XYZ: 999.111 / 12.00000 / 42.500
Block: 123 64 -789
Chunk: 7 4 -50";
        let found = parse_f3_text(text).unwrap();
        assert_eq!(found.source, "block");
        assert_eq!((found.x, found.y, found.z), (123.0, 64.0, -789.0));
    }

    #[test]
    fn falls_back_from_xyz_to_chunk() {
        let found = parse_f3_text("Facing: north\nChunk: 0 4 0").unwrap();
        assert_eq!(found.source, "chunk");
        let found = parse_f3_text("XYZ: 1.5 / 2.5 / 3.5\nChunk: 0 4 0").unwrap();
        assert_eq!(found.source, "xyz");
    }

    #[test]
    fn handles_negatives_and_decimals() {
        let found = parse_f3_text("XYZ: -1234.5 / -59.00000 / -0.25").unwrap();
        assert!((found.x - -1234.5).abs() < 1e-9);
        assert!((found.y - -59.0).abs() < 1e-9);
        assert!((found.z - -0.25).abs() < 1e-9);

        let found = parse_f3_text("Block: -1235 -59 -1").unwrap();
        assert_eq!((found.x, found.y, found.z), (-1235.0, -59.0, -1.0));
    }

    #[test]
    fn tolerates_ocr_mangling() {
        let found = parse_f3_text("XYZ: l23.4S6 / 64.O0 / -789.0").unwrap();
        assert_eq!(found.source, "xyz");
        assert!((found.x - 123.456).abs() < 1e-9);
        assert!((found.y - 64.0).abs() < 1e-9);
        assert!((found.z - -789.0).abs() < 1e-9);
    }

    #[test]
    fn tolerates_stray_spacing_and_case() {
        let found = parse_f3_text("xyz:123.456/64.0/-789.012").unwrap();
        assert!((found.x - 123.456).abs() < 1e-9);
        let found = parse_f3_text("XYZ :  12.0  /  64.0  /  -7.0").unwrap();
        assert!((found.x - 12.0).abs() < 1e-9);
        let found = parse_f3_text("BLOCK:  1O0   6S   -2OO").unwrap();
        assert_eq!((found.x, found.y, found.z), (100.0, 65.0, -200.0));
    }

    #[test]
    fn ignores_the_targeted_block() {
        // The crosshair's block is a different question from the player's
        // position, and answering it silently would send every later mode to
        // the wrong place.
        let text = "Targeted Block: 500 70 500\nXYZ: 1.0 / 2.0 / 3.0";
        let found = parse_f3_text(text).unwrap();
        assert_eq!(found.source, "xyz");
        assert_eq!((found.x, found.y, found.z), (1.0, 2.0, 3.0));

        assert!(parse_f3_text("Targeted Block: 500 70 500").is_none());
    }

    #[test]
    fn reads_a_whole_f3_overlay() {
        // A full left-hand column as 1.21 draws it, including the lines that
        // look enough like coordinates to be dangerous: "Client Chunk Cache"
        // starts with the word Chunk, and the light line ends with the word
        // block.
        let text = "\
Minecraft 1.21.1 (1.21.1/vanilla)
120 fps T: 120 vsync fancy-clouds B: 2
Integrated server @ 3 ms ticks, 0 tx, 0 rx
C: 1234/20000 (s) D: 12, pC: 000, pU: 00, aB: 32
E: 12/34, B: 0, SD: 12
Client Chunk Cache: 2500, 1849
ServerChunkCache: 1849
XYZ: -1234.567 / 71.00000 / 890.123
Block: -1235 71 890 [13 7 2]
Chunk: -78 4 55 [-3 3 in -1 0]
Facing: south (Towards positive Z) (-12.3 / 45.6)
Client Light: 15 (15 sky, 0 block)
Biome: minecraft:plains";
        let found = parse_f3_text(text).unwrap();
        assert_eq!(found.source, "block");
        assert_eq!((found.x, found.y, found.z), (-1235.0, 71.0, 890.0));
    }

    #[test]
    fn text_without_coordinates_is_none() {
        assert!(parse_f3_text("").is_none());
        assert!(parse_f3_text("Minecraft 1.21.1\n60 fps T: inf vsync").is_none());
        assert!(parse_f3_text("no numbers here at all").is_none());
        // A label with no usable numbers after it.
        assert!(parse_f3_text("Block: minecraft:stone").is_none());
        assert!(parse_f3_text("XYZ: ??? / ??? / ???").is_none());
    }

    #[test]
    fn implausible_readings_are_rejected() {
        // A misread sign or a doubled digit can put the position outside the
        // world; better to report nothing than to search 400 million blocks out.
        assert!(parse_f3_text("Block: 123 99999 -789").is_none());
        assert!(parse_f3_text("Block: 999999999 64 0").is_none());
    }

    #[test]
    fn number_repair() {
        assert_eq!(repair_number("123"), Some(123.0));
        assert_eq!(repair_number("l23"), Some(123.0));
        assert_eq!(repair_number("-O.5"), Some(-0.5));
        assert_eq!(repair_number("+42"), Some(42.0));
        assert_eq!(repair_number("64."), Some(64.0));
        assert_eq!(repair_number("12,5"), Some(12.5));
        assert_eq!(repair_number("−7"), Some(-7.0)); // unicode minus
        assert_eq!(repair_number(" 8 "), Some(8.0));
        assert_eq!(repair_number(""), None);
        assert_eq!(repair_number("."), None);
        assert_eq!(repair_number("-"), None);
        assert_eq!(repair_number("12.3.4"), None);
        assert_eq!(repair_number("stone"), None);
    }

    #[test]
    fn block_position_floors_like_minecraft() {
        let found = F3Coords {
            x: 123.456,
            y: 64.0,
            z: -789.012,
            source: "xyz",
        };
        assert_eq!(found.block_pos(), (123, 64, -790));
    }

    // -- crop ---------------------------------------------------------------

    #[test]
    fn crops_resolve_and_clamp() {
        assert_eq!(CropSpec::Full.resolve(100, 50), (0, 0, 100, 50));
        assert_eq!(CropSpec::TopLeftQuadrant.resolve(100, 50), (0, 0, 50, 25));
        // Odd sizes round up rather than losing the middle column.
        assert_eq!(CropSpec::TopLeftQuadrant.resolve(101, 51), (0, 0, 51, 26));
        assert_eq!(
            CropSpec::Pixels {
                x: 10,
                y: 10,
                w: 500,
                h: 500
            }
            .resolve(100, 50),
            (10, 10, 90, 40)
        );
        assert_eq!(
            CropSpec::Fraction {
                x: 0.5,
                y: 0.0,
                w: 0.5,
                h: 1.0
            }
            .resolve(100, 50),
            (50, 0, 50, 50)
        );
        // Out-of-range fractions clamp instead of panicking.
        assert_eq!(
            CropSpec::Fraction {
                x: 2.0,
                y: -1.0,
                w: 9.0,
                h: 9.0
            }
            .resolve(100, 50),
            (100, 0, 0, 50)
        );
    }

    // -- preprocessing ------------------------------------------------------

    /// A 8x4 image: the left half dark, the right half light, which is the
    /// smallest thing that can show whether the threshold ran the right way
    /// round.
    fn synthetic() -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_fn(8, 4, |x, _| {
            image::Luma([if x < 4 { 20 } else { 230 }])
        }))
    }

    #[test]
    fn preprocess_keeps_the_crop_dimensions() {
        let img = synthetic();
        let opts = Preprocess {
            invert: false,
            threshold: 128,
            scale: 1,
        };
        assert_eq!(preprocess(&img, CropSpec::Full, opts).dimensions(), (8, 4));
        assert_eq!(
            preprocess(&img, CropSpec::TopLeftQuadrant, opts).dimensions(),
            (4, 2)
        );
        assert_eq!(
            preprocess(
                &img,
                CropSpec::Pixels {
                    x: 2,
                    y: 1,
                    w: 3,
                    h: 2
                },
                opts
            )
            .dimensions(),
            (3, 2)
        );
        assert_eq!(
            preprocess(
                &img,
                CropSpec::Fraction {
                    x: 0.5,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0
                },
                opts
            )
            .dimensions(),
            (4, 4)
        );
    }

    #[test]
    fn upscaling_multiplies_the_dimensions() {
        let img = synthetic();
        let out = preprocess(
            &img,
            CropSpec::Full,
            Preprocess {
                invert: false,
                threshold: 128,
                scale: 3,
            },
        );
        assert_eq!(out.dimensions(), (24, 12));
        // Still binary after Lanczos3 has smeared the edges.
        assert!(out.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));

        // The scale is clamped, so a fat-fingered 500 cannot ask for a
        // gigapixel image.
        let out = preprocess(
            &img,
            CropSpec::Full,
            Preprocess {
                invert: false,
                threshold: 128,
                scale: 500,
            },
        );
        assert_eq!(out.dimensions(), (64, 32));
    }

    #[test]
    fn thresholding_is_binary_and_the_right_way_round() {
        let img = synthetic();

        let plain = preprocess(
            &img,
            CropSpec::Full,
            Preprocess {
                invert: false,
                threshold: 128,
                scale: 1,
            },
        );
        assert!(plain.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));
        assert_eq!(plain.get_pixel(0, 0).0[0], 0, "dark stays dark");
        assert_eq!(plain.get_pixel(7, 0).0[0], 255, "light stays light");

        // Inverted is what the F3 overlay actually needs: its light text has to
        // come out black on white before Tesseract will read it.
        let inverted = preprocess(
            &img,
            CropSpec::Full,
            Preprocess {
                invert: true,
                threshold: 128,
                scale: 1,
            },
        );
        assert!(inverted.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));
        assert_eq!(inverted.get_pixel(0, 0).0[0], 255);
        assert_eq!(inverted.get_pixel(7, 0).0[0], 0);

        // Every pixel differs between the two, which is the whole point of
        // offering both.
        assert!(
            plain
                .pixels()
                .zip(inverted.pixels())
                .all(|(a, b)| a.0[0] != b.0[0])
        );
    }

    #[test]
    fn the_threshold_cutoff_moves() {
        let img = DynamicImage::ImageLuma8(GrayImage::from_fn(2, 1, |x, _| {
            image::Luma([if x == 0 { 100 } else { 200 }])
        }));
        let opts = |threshold| Preprocess {
            invert: false,
            threshold,
            scale: 1,
        };
        let low = preprocess(&img, CropSpec::Full, opts(50));
        assert_eq!((low.get_pixel(0, 0).0[0], low.get_pixel(1, 0).0[0]), (255, 255));
        let mid = preprocess(&img, CropSpec::Full, opts(150));
        assert_eq!((mid.get_pixel(0, 0).0[0], mid.get_pixel(1, 0).0[0]), (0, 255));
        let high = preprocess(&img, CropSpec::Full, opts(250));
        assert_eq!((high.get_pixel(0, 0).0[0], high.get_pixel(1, 0).0[0]), (0, 0));
    }

    #[test]
    fn an_empty_crop_gives_an_empty_image_rather_than_panicking() {
        let img = synthetic();
        let out = preprocess(
            &img,
            CropSpec::Pixels {
                x: 500,
                y: 500,
                w: 10,
                h: 10
            },
            Preprocess::default(),
        );
        assert_eq!(out.dimensions(), (0, 0));
    }

    #[test]
    fn processed_images_encode_as_png() {
        let out = preprocess(&synthetic(), CropSpec::Full, Preprocess::default());
        let png = encode_png(&out).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // And it round-trips, which is what Leptonica will be doing with it.
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!(decoded.width(), out.width());
        assert_eq!(decoded.height(), out.height());
    }

    // -- folder mode --------------------------------------------------------

    #[test]
    fn image_extensions_are_matched_case_insensitively() {
        assert!(has_image_extension(Path::new("shot.png")));
        assert!(has_image_extension(Path::new("shot.JPG")));
        assert!(has_image_extension(Path::new("a/b/shot.jpeg")));
        assert!(has_image_extension(Path::new("shot.WebP")));
        assert!(!has_image_extension(Path::new("shot.txt")));
        assert!(!has_image_extension(Path::new("shot")));
    }
}

/// End-to-end exercise of the real Tesseract path.
///
/// The rest of this module's tests cover parsing and preprocessing as pure
/// functions, which is most of the risk — but none of them touch `leptess`.
/// This one renders a synthetic F3 overlay (light monospace on the dark
/// translucent panel vanilla draws), pushes it through the exact pipeline the
/// mode uses, and checks the coordinates come back. It only builds under the
/// `ocr` feature, since that is when the backend exists at all.
// Gated to macOS as well as the feature: the synthetic overlay is drawn with
// Monaco, which only ships with macOS. The parser and preprocessing tests that
// carry most of the risk are platform-independent and always run.
#[cfg(all(test, feature = "ocr", target_os = "macos"))]
mod backend_tests {
    use super::*;
    use ab_glyph::{FontRef, PxScale};
    use image::{Rgb, RgbImage};

    /// Draws an F3-style overlay: light grey text on a dark panel.
    fn render_f3(lines: &[&str]) -> DynamicImage {
        // Monaco ships with macOS and is a plain TTF, so ab_glyph can read it
        // directly (Menlo is a .ttc collection, which it cannot).
        let bytes = std::fs::read("/System/Library/Fonts/Monaco.ttf")
            .expect("Monaco.ttf should exist on macOS");
        let font = FontRef::try_from_slice(&bytes).expect("Monaco.ttf should parse");

        let mut img = RgbImage::from_pixel(900, 500, Rgb([28, 32, 40]));
        // The panel vanilla draws behind the text.
        for y in 10..(20 + 34 * lines.len() as u32) {
            for x in 8..880 {
                img.put_pixel(x, y, Rgb([16, 16, 16]));
            }
        }
        let scale = PxScale::from(28.0);
        for (i, line) in lines.iter().enumerate() {
            imageproc::drawing::draw_text_mut(
                &mut img,
                Rgb([222, 222, 222]),
                14,
                16 + 34 * i as i32,
                scale,
                &font,
                line,
            );
        }
        DynamicImage::ImageRgb8(img)
    }

    fn read_back(lines: &[&str]) -> Option<F3Coords> {
        let img = render_f3(lines);
        let processed = preprocess(
            &img,
            CropSpec::Full,
            Preprocess {
                // Light text on a dark panel: invert so Tesseract sees the
                // dark-on-light it expects.
                invert: true,
                threshold: 128,
                scale: 2,
            },
        );
        let png = encode_png(&processed).expect("the processed crop should encode");
        let mut ocr = backend::Ocr::new().expect("Tesseract should start with eng data present");
        let text = ocr.text(&png).expect("Tesseract should return text");
        println!("OCR returned: {text:?}");
        parse_f3_text(&text)
    }

    #[test]
    fn reads_coordinates_out_of_a_rendered_f3_overlay() {
        let got = read_back(&[
            "XYZ: 123.456 / 64.00000 / -789.012",
            "Block: 123 64 -789",
            "Chunk: 7 4 -50",
        ])
        .expect("the pipeline should recover coordinates");

        assert!((got.x - 123.0).abs() < 1.0, "x was {}", got.x);
        assert!((got.y - 64.0).abs() < 1.0, "y was {}", got.y);
        assert!((got.z - -789.0).abs() < 1.0, "z was {}", got.z);
    }

    #[test]
    fn survives_an_overlay_with_only_the_xyz_line() {
        let got = read_back(&["XYZ: -1290.500 / 71.00000 / 2048.250"])
            .expect("a lone XYZ line should still parse");
        assert!((got.x - -1290.5).abs() < 2.0, "x was {}", got.x);
        assert!((got.z - 2048.25).abs() < 2.0, "z was {}", got.z);
    }

    #[test]
    fn an_overlay_with_no_coordinates_yields_nothing() {
        assert!(read_back(&["Minecraft 1.21.3", "60 fps T: inf vsync"]).is_none());
    }
}
