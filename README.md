<p align="center">
  <img src="https://github.com/cioni03/Marzianetto/blob/main/packaging/assets/marzianetto.png?raw=true" alt="marzianetto logo" width="160">
</p>

<h1 align="center">marzianetto</h1>

<p align="center">A simple desktop app for compressing and resizing images.</p>

<p align="center">
  <a href="https://cioni03.github.io/Marzianetto/">
    <img src="https://img.shields.io/badge/Website-Visit%20Site-2ea44f?style=for-the-badge&logo=google-chrome&logoColor=white" alt="Website">
  </a>
  <a href="https://github.com/cioni03/Marzianetto/releases/latest">
    <img src="https://img.shields.io/github/v/release/cioni03/Marzianetto?style=for-the-badge&label=Latest%20Release&color=blue" alt="Latest Release">
  </a>
</p>

<p align="center">
  <a href="https://github.com/cioni03/Marzianetto/releases/latest/download/marzianetto-x86_64-pc-windows-msvc.zip">
    <img src="https://img.shields.io/badge/Windows-Download%20(.zip)-0078D6?style=flat-square&logo=windows&logoColor=white" alt="Download for Windows">
  </a>
  <a href="https://github.com/cioni03/Marzianetto/releases/latest/download/marzianetto-aarch64-apple-darwin.dmg">
    <img src="https://img.shields.io/badge/macOS-Download%20(.dmg)-000000?style=flat-square&logo=apple&logoColor=white" alt="Download for macOS">
  </a>
  <a href="https://github.com/cioni03/Marzianetto/releases/latest/download/marzianetto-linux-x86_64.AppImage">
    <img src="https://img.shields.io/badge/Linux-Download%20(.AppImage)-FCC624?style=flat-square&logo=linux&logoColor=black" alt="Download for Linux">
  </a>
</p>

## What it does

- Add images or folders by browsing or dragging and dropping them into the app.
- Compress images in their original format, or optionally convert to JPEG/JPG, PNG, WebP, AVIF, BMP, TIFF, or GIF.
- Use independent **Comprimi** and **Converti formato** checkboxes: compress only, convert only, or both. Inactive quality, format, and resize controls are hidden.
- Handle transparency automatically. In the settings, enable **Converti formato** to choose another format; leave it off to keep each image's original format.
- Choose the output quality and resize images while keeping their proportions.
- Save compressed images in another folder, next to the originals, or replace the originals.
- Preview images and compare file sizes before and after compression.
- See the results and any errors when the work is complete.
- Remove image metadata, such as location and camera details, for extra privacy.
- Open images from your file manager using the system context menu.

The app supports JPEG, PNG, WebP, BMP, GIF, and TIFF as input. Animated GIFs and multi-page TIFFs use their first frame or page.

With **Comprimi** off, the quality slider is ignored: PNG is uncompressed, WebP is lossless, and JPEG/AVIF use quality 100. Encoding required by the chosen format still applies, so JPEG/AVIF are not guaranteed lossless and GIF is limited to a palette. Resizing remains independent. With all three operations off, the run button is hidden.

**Transparency:** PNG, WebP, AVIF, BMP, and TIFF retain transparency automatically. PNG and TIFF also retain 16-bit alpha. JPEG/JPG composites transparent pixels on a white background. GIF retains fully transparent pixels and composites unsupported partial transparency on white. No confirmation or extra setting is needed. Transparent AVIF output automatically uses quality 100 because the encoder shares the color and alpha quality setting. Existing saved preferences retain their selected conversion format.

Available for **Windows, macOS, and Linux**. Compressed copies are saved where you choose; your originals are left untouched unless you select the replace option.
