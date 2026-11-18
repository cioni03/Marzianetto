# marzianetto

Compress, resize and convert images with a lightweight desktop app for macOS, Windows and Linux.

## Features

- Add images or folders with drag and drop or the file picker. Choose whether folder import includes subfolders.
- Preview images and compare original and compressed file sizes.
- Keep each image's original format or optionally export as JPEG/JPG, PNG, WebP, AVIF, BMP, TIFF or GIF using **Converti formato**.
- Toggle **Comprimi** and **Converti formato** independently to compress only, convert only, or do both. Controls for inactive operations are hidden; resizing is independent too.
- Preserve transparency automatically in formats that support it, otherwise composite on white without asking. PNG and TIFF also preserve 16-bit alpha.
- Adjust image quality and resize by the long or short edge while keeping the aspect ratio.
- Save copies beside the originals, in a separate folder, or replace the originals after confirmation.
- Remove image metadata, including EXIF/GPS and color profiles. This privacy option is enabled by default.
- Review progress, per-file results and total space saved.

Settings are saved automatically. Image processing runs locally on your computer.

## Run from source

Install stable Rust, then run:

```sh
cargo run --release
```

For platform-specific builds (`.dmg`, Windows `.exe`/ZIP and Linux Flatpak), see [BUILDING.md](BUILDING.md).

## Notes

- Animated GIFs and multi-page TIFFs are converted from their first frame/page. Replacing those originals is disabled.
- With **Comprimi** off, the saved quality slider is ignored: PNG is uncompressed, WebP is lossless, and JPEG/AVIF use quality 100. The target format's encoding limits still apply: JPEG/AVIF are not guaranteed lossless and GIF uses a limited palette. With compression, conversion and resizing all off, the run button is hidden.
- JPEG/JPG automatically composites transparency on white. GIF retains fully transparent pixels and composites unsupported partial transparency on white. Conversion is never blocked because of transparency, and no confirmation is needed.
- Transparent AVIF output automatically uses quality 100, since the encoder shares its color and alpha quality setting.
- Existing saved preferences retain their selected conversion format; new installations keep the original format unless conversion is enabled.
- Removing color profiles can affect how colors look. Unsupported metadata may not be preserved when metadata removal is turned off.
- Replacing originals cannot be undone. Keep backups of important images.
- The macOS package is not notarized, and Windows/Linux release builds should be verified on their target systems.
