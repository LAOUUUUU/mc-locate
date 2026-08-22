# Mode 3 — F3 Screenshot OCR Reader

Reads coordinates straight out of the F3 debug overlay in a screenshot.

**Needs:** an image, or a folder of images for batch mode (which writes CSV).

**Where it comes from:** any screenshot with F3 open. Old recordings and
screenshots you took for other reasons all work.

**How it works.** Crop to where the overlay is, convert to greyscale, threshold
it, optionally upscale, then OCR. The crop is configurable because the overlay
moves with resolution and GUI scale; the fractional option survives a
resolution change part-way through a folder.

It reads three line kinds and **prefers `Block:`** when present, because
integers survive OCR far better than the decimals on the `XYZ:` line.

The digit-repair table fixes what Tesseract actually gets wrong on this
overlay, not what seems plausible: `@` for `0` is the most common by a distance
(observed as `-1290.50@`, `6@ fps`, `-5@`), along with `O`, `l`, `S`, `B`.

**Limits.** Needs system Tesseract, so it is behind a cargo feature and is
**not in the prebuilt binaries**. Without it the mode explains how to enable it
and offers manual entry. Build with `--features ocr` after installing Tesseract
and Leptonica.

**Feeds:** a search box around the read coordinate. Mode 13 can watch your
screenshots folder and do this automatically as you press F2.
