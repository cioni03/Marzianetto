# marzianetto

Compress, resize and convert images with a lightweight desktop app for macOS, Windows and Linux.

## Features

- Add images or folders with drag and drop or the file picker. Choose whether folder import includes subfolders.
- Preview images and compare original and compressed file sizes.
- Export as JPEG, PNG, WebP, AVIF, BMP or TIFF.
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
- Removing color profiles can affect how colors look. Unsupported metadata may not be preserved when metadata removal is turned off.
- Replacing originals cannot be undone. Keep backups of important images.
- The macOS package is not notarized, and Windows/Linux release builds should be verified on their target systems.
