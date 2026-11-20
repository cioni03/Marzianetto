use crate::{
    metadata::{self, Metadata},
    model::{CompressionResult, Source},
    settings::{Destination, OutputFormat, ResizeEdge, Settings},
};
use anyhow::{bail, Context, Result};
use image::{
    codecs::{
        avif::AvifEncoder,
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType, PngEncoder},
    },
    DynamicImage, ImageEncoder, ImageFormat,
};
use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
};

pub enum CompressionEvent {
    Started(usize),
    Completed(usize, std::result::Result<CompressionResult, String>),
    Finished { cancelled: bool },
}

pub fn run(
    sources: Vec<Source>,
    settings: Settings,
    tx: Sender<CompressionEvent>,
    cancel: Arc<AtomicBool>,
) {
    for (index, source) in sources.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send(CompressionEvent::Started(index));
        let result = compress(source, &settings).map_err(|err| format!("{err:#}"));
        let _ = tx.send(CompressionEvent::Completed(index, result));
    }
    let _ = tx.send(CompressionEvent::Finished {
        cancelled: cancel.load(Ordering::Relaxed),
    });
}

fn check_source(source: &Source) -> Result<()> {
    let metadata = fs::symlink_metadata(&source.path)?;
    if !metadata.is_file()
        || metadata.len() != source.bytes
        || metadata.modified().ok() != source.modified
    {
        bail!("Il file è cambiato dall'importazione: ricaricalo prima di procedere.");
    }
    Ok(())
}

pub fn output_path(source: &Source, settings: &Settings) -> Result<PathBuf> {
    let format = settings.output_format(&source.format)?;
    let parent = source
        .path
        .parent()
        .context("Cartella di origine mancante")?;
    let stem = source.path.file_stem().context("Nome file mancante")?;
    let mut name = stem.to_os_string();
    // Nella cartella separata il suffisso nomina solo la cartella: i file
    // dentro conservano il nome originale. Accanto agli originali, invece,
    // distingue le copie dai file di partenza.
    if settings.destination == Destination::SameFolder {
        name.push(&settings.suffix);
    }
    name.push(format!(".{}", format.extension()));
    match settings.destination {
        Destination::SameFolder => Ok(parent.join(name)),
        Destination::Replace => {
            // Mantiene l'estensione originale, anche .jpeg/.JPG, se il codec non cambia.
            let same_codec =
                image::ImageFormat::from_path(&source.path).ok() == Some(image_format(format));
            Ok(if same_codec {
                source.path.clone()
            } else {
                parent.join(name)
            })
        }
        Destination::SiblingFolder => {
            let root_parent = source
                .root
                .parent()
                .context("Impossibile creare una cartella accanto alla radice del disco")?;
            let mut folder = source
                .root
                .file_name()
                .context("Nome cartella mancante")?
                .to_os_string();
            folder.push(&settings.suffix);
            let relative = parent
                .strip_prefix(&source.root)
                .context("File fuori dalla radice di importazione")?;
            Ok(root_parent.join(folder).join(relative).join(name))
        }
    }
}

fn image_format(format: OutputFormat) -> ImageFormat {
    match format {
        OutputFormat::Jpeg => ImageFormat::Jpeg,
        OutputFormat::Png => ImageFormat::Png,
        OutputFormat::WebP => ImageFormat::WebP,
        OutputFormat::Avif => ImageFormat::Avif,
        OutputFormat::Bmp => ImageFormat::Bmp,
        OutputFormat::Tiff => ImageFormat::Tiff,
        OutputFormat::Gif => ImageFormat::Gif,
    }
}

pub fn compress(source: &Source, settings: &Settings) -> Result<CompressionResult> {
    settings.validate()?;
    if !settings.has_operation() {
        bail!("Attiva almeno un'operazione: compressione, conversione o ridimensionamento.");
    }
    check_source(source)?;
    if settings.destination == Destination::Replace
        && matches!(source.format.as_str(), "Gif" | "Tiff")
    {
        bail!("Sostituzione disabilitata per GIF/TIFF: possono contenere più frame o pagine. Usa una copia.");
    }
    let (image, mut metadata) = metadata::decode(&source.path, settings.strip_metadata)?;
    let format = settings.output_format(&source.format)?;
    let image = resize(image, settings);
    let target = output_path(source, settings)?;
    let parent = target.parent().context("Destinazione non valida")?;
    fs::create_dir_all(parent)?;
    // Scrive prima su un file temporaneo nello stesso filesystem. L'originale
    // non viene toccato finché codifica, flush e sync non sono terminati.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    encode(&image, format, settings, temp.as_file_mut(), &mut metadata)?;
    temp.as_file().sync_all()?;
    let output_bytes = temp.as_file().metadata()?.len();
    check_source(source)?;
    let (path, warning) = if settings.destination == Destination::Replace && target == source.path {
        fs::set_permissions(temp.path(), fs::metadata(&source.path)?.permissions())?;
        temp.persist(&target)
            .map_err(|err| err.error)
            .context("Sostituzione atomica non riuscita")?;
        (target, None)
    } else {
        let path = persist_unique(temp, &target)?;
        let warning = if settings.destination == Destination::Replace {
            // Solo il file sorgente selezionato viene eliminato, mai un file di destinazione preesistente.
            check_source(source)
                .and_then(|()| fs::remove_file(&source.path).map_err(Into::into))
                .err()
                .map(|err| format!("Output salvato, ma originale conservato: {err}"))
        } else {
            None
        };
        (path, warning)
    };
    let original_retained = warning.is_some();
    if let Some(warning) = warning {
        metadata.warnings.push(warning);
    }
    Ok(CompressionResult {
        path,
        input_bytes: source.bytes,
        output_bytes,
        warning: if metadata.warnings.is_empty() {
            None
        } else {
            Some(metadata.warnings.join("\n"))
        },
        original_retained,
    })
}

/// Controlla anche alpha a 16 bit senza allocare una seconda immagine.
fn has_transparency(image: &DynamicImage) -> bool {
    macro_rules! check {
        ($buffer:expr, $alpha:expr, $max:expr) => {
            $buffer.pixels().any(|p| p[$alpha] < $max)
        };
    }
    match image {
        DynamicImage::ImageLumaA8(buffer) => check!(buffer, 1, u8::MAX),
        DynamicImage::ImageRgba8(buffer) => check!(buffer, 3, u8::MAX),
        DynamicImage::ImageLumaA16(buffer) => check!(buffer, 1, u16::MAX),
        DynamicImage::ImageRgba16(buffer) => check!(buffer, 3, u16::MAX),
        DynamicImage::ImageRgba32F(buffer) => check!(buffer, 3, 1.0),
        _ => false,
    }
}

fn composite_on_white(pixel: image::Rgba<u8>) -> image::Rgb<u8> {
    let alpha = pixel[3] as u32;
    image::Rgb(std::array::from_fn(|i| {
        ((pixel[i] as u32 * alpha + 255 * (255 - alpha) + 127) / 255) as u8
    }))
}

fn resize(image: DynamicImage, settings: &Settings) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    let edge = match settings.resize_edge {
        ResizeEdge::Long => width.max(height),
        ResizeEdge::Short => width.min(height),
    };
    if !settings.resize || edge <= settings.long_edge || edge == 0 {
        return image;
    }
    // Calcolo intero arrotondato: il lato scelto è esatto e l'altro mantiene
    // le proporzioni al pixel più vicino. Non aumenta mai le dimensioni.
    let scale = |value: u32| {
        ((value as u64 * settings.long_edge as u64 + edge as u64 / 2) / edge as u64).max(1) as u32
    };
    image.resize_exact(
        scale(width),
        scale(height),
        image::imageops::FilterType::Lanczos3,
    )
}

/// Persistenza senza sovrascrittura, con retry anche in caso di collisioni concorrenti.
fn persist_unique(mut temp: tempfile::NamedTempFile, target: &Path) -> Result<PathBuf> {
    for index in 0..10_000 {
        let candidate = if index == 0 {
            target.to_owned()
        } else {
            let mut name = target
                .file_stem()
                .context("Nome output mancante")?
                .to_os_string();
            name.push(format!("_{index}"));
            if let Some(ext) = target.extension() {
                name.push(".");
                name.push(ext);
            }
            target.with_file_name(name)
        };
        match temp.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = err.file;
            }
            Err(err) => return Err(err.error.into()),
        }
    }
    bail!("Troppe collisioni di nomi nella cartella di destinazione")
}

fn encode(
    image: &DynamicImage,
    format: OutputFormat,
    settings: &Settings,
    file: &mut fs::File,
    metadata: &mut Metadata,
) -> Result<()> {
    let mut writer = BufWriter::new(file);
    match format {
        OutputFormat::Jpeg => {
            // JPEG non supporta alpha: composizione su bianco, non su nero.
            let rgba = image.to_rgba8();
            let rgb = image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
                composite_on_white(*rgba.get_pixel(x, y))
            });
            let mut encoder =
                JpegEncoder::new_with_quality(&mut writer, settings.encoding_quality().max(1));
            metadata::apply(&mut encoder, metadata);
            encoder.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        OutputFormat::Png => {
            let compression = if !settings.compression {
                CompressionType::Uncompressed
            } else {
                match settings.quality {
                    0..=33 => CompressionType::Fast,
                    34..=66 => CompressionType::Default,
                    _ => CompressionType::Best,
                }
            };
            let filter = if settings.compression {
                FilterType::Adaptive
            } else {
                FilterType::NoFilter
            };
            let mut encoder = PngEncoder::new_with_quality(&mut writer, compression, filter);
            metadata::apply(&mut encoder, metadata);
            // PNG conserva anche i livelli di alpha a 16 bit degli originali.
            let rgba = if matches!(
                image.color(),
                image::ColorType::L16
                    | image::ColorType::La16
                    | image::ColorType::Rgb16
                    | image::ColorType::Rgba16
            ) {
                DynamicImage::ImageRgba16(image.to_rgba16())
            } else {
                DynamicImage::ImageRgba8(image.to_rgba8())
            };
            encoder.write_image(
                rgba.as_bytes(),
                rgba.width(),
                rgba.height(),
                rgba.color().into(),
            )?;
        }
        OutputFormat::WebP => {
            metadata::unsupported(metadata);
            let rgba = image.to_rgba8();
            let encoder = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
            let encoded = if settings.compression {
                encoder.encode(settings.quality as f32)
            } else {
                encoder.encode_lossless()
            };
            writer.write_all(&encoded)?;
        }
        OutputFormat::Avif => {
            let rgba = image.to_rgba8();
            // Questo encoder lega qualità colore e alpha: usa il massimo per
            // non degradare la trasparenza, senza richiedere scelte all'utente.
            let quality = if has_transparency(image) {
                100
            } else {
                settings.encoding_quality().max(1)
            };
            let mut encoder = AvifEncoder::new_with_speed_quality(&mut writer, 6, quality);
            metadata::apply(&mut encoder, metadata);
            encoder.write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        OutputFormat::Bmp => {
            metadata::unsupported(metadata);
            // RGBA forza il contenitore BMP con canale alpha anche per input
            // in scala di grigi con trasparenza.
            DynamicImage::ImageRgba8(image.to_rgba8())
                .write_to(&mut writer, image_format(format))?;
        }
        OutputFormat::Tiff => {
            let rgba = if matches!(
                image.color(),
                image::ColorType::L16
                    | image::ColorType::La16
                    | image::ColorType::Rgb16
                    | image::ColorType::Rgba16
            ) {
                DynamicImage::ImageRgba16(image.to_rgba16())
            } else {
                DynamicImage::ImageRgba8(image.to_rgba8())
            };
            let mut encoder = image::codecs::tiff::TiffEncoder::new(&mut writer);
            metadata::apply(&mut encoder, metadata);
            encoder.write_image(
                rgba.as_bytes(),
                rgba.width(),
                rgba.height(),
                rgba.color().into(),
            )?;
        }
        OutputFormat::Gif => {
            metadata::unsupported(metadata);
            let mut rgba = image.to_rgba8();
            // GIF conserva la trasparenza totale. I livelli parziali non
            // supportati vengono composti su bianco, anche dopo il resize.
            for pixel in rgba.pixels_mut() {
                if pixel[3] > 0 && pixel[3] < 255 {
                    let rgb = composite_on_white(*pixel);
                    *pixel = image::Rgba([rgb[0], rgb[1], rgb[2], 255]);
                }
            }
            image::codecs::gif::GifEncoder::new(&mut writer).encode(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_edge_resize_handles_landscape_portrait_and_rounding() {
        let settings = Settings {
            resize: true,
            resize_edge: ResizeEdge::Short,
            long_edge: 100,
            ..Default::default()
        };
        for (width, height, expected) in [
            (600, 400, (150, 100)),
            (400, 600, (100, 150)),
            (601, 400, (150, 100)),
            (400, 400, (100, 100)),
        ] {
            let output = resize(DynamicImage::new_rgb8(width, height), &settings);
            assert_eq!((output.width(), output.height()), expected);
        }
    }

    #[test]
    fn selected_edge_never_upscales() {
        for edge in [ResizeEdge::Long, ResizeEdge::Short] {
            let settings = Settings {
                resize: true,
                resize_edge: edge,
                long_edge: 200,
                ..Default::default()
            };
            let output = resize(DynamicImage::new_rgb8(100, 50), &settings);
            assert_eq!((output.width(), output.height()), (100, 50));
        }
        // Il lato lungo può essere > target: in modalità lato corto non
        // ridimensioniamo se il lato corto è già al di sotto del limite.
        let settings = Settings {
            resize: true,
            resize_edge: ResizeEdge::Short,
            long_edge: 100,
            ..Default::default()
        };
        let output = resize(DynamicImage::new_rgb8(1000, 50), &settings);
        assert_eq!((output.width(), output.height()), (1000, 50));
    }

    fn fixture(dir: &Path) -> Source {
        let path = dir.join("test.png");
        image::RgbaImage::from_pixel(32, 16, image::Rgba([255, 0, 0, 128]))
            .save(&path)
            .unwrap();
        let meta = fs::metadata(&path).unwrap();
        Source {
            path,
            root: dir.to_owned(),
            bytes: meta.len(),
            modified: meta.modified().ok(),
            format: "Png".into(),
            dimensions: [32, 16],
        }
    }

    fn alpha_fixture(dir: &Path) -> Source {
        let mut source = fixture(dir);
        image::RgbaImage::from_fn(16, 16, |x, y| {
            image::Rgba([40, 120, 200, (x + y * 16) as u8])
        })
        .save(&source.path)
        .unwrap();
        let meta = fs::metadata(&source.path).unwrap();
        source.bytes = meta.len();
        source.modified = meta.modified().ok();
        source.dimensions = [16, 16];
        source
    }

    #[test]
    fn conversion_only_ignores_hidden_compression_quality_for_every_codec() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        for format in OutputFormat::ALL {
            let mut settings = Settings {
                compression: false,
                convert_format: true,
                quality: 1,
                destination: Destination::SameFolder,
                format,
                ..Default::default()
            };
            assert_eq!(settings.action_label(), "Converti immagini");
            let low = compress(&source, &settings).unwrap();
            settings.quality = 100;
            let high = compress(&source, &settings).unwrap();
            assert_eq!(
                fs::read(low.path).unwrap(),
                fs::read(high.path).unwrap(),
                "Il cursore nascosto non deve influire sulla conversione {format:?}"
            );
        }
    }

    #[test]
    fn conversion_only_webp_is_lossless_and_preserves_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        let settings = Settings {
            compression: false,
            convert_format: true,
            format: OutputFormat::WebP,
            quality: 1,
            destination: Destination::SameFolder,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        let original = image::open(&source.path).unwrap().to_rgba8();
        let output = image::open(&result.path).unwrap().to_rgba8();
        for (input, output) in original.pixels().zip(output.pixels()) {
            assert_eq!(input[3], output[3]);
            if input[3] != 0 {
                assert_eq!(input, output);
            }
        }
    }

    #[test]
    fn conversion_only_png_disables_compression_without_changing_pixels() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let mut settings = Settings {
            compression: false,
            convert_format: true,
            format: OutputFormat::Png,
            destination: Destination::SameFolder,
            ..Default::default()
        };
        let uncompressed = compress(&source, &settings).unwrap();
        settings.compression = true;
        let compressed = compress(&source, &settings).unwrap();
        assert!(uncompressed.output_bytes > compressed.output_bytes);
        assert_eq!(
            image::open(uncompressed.path).unwrap().to_rgba8(),
            image::open(compressed.path).unwrap().to_rgba8()
        );
    }

    #[test]
    fn disabled_operations_do_not_write_and_resize_only_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let mut settings = Settings {
            compression: false,
            convert_format: false,
            destination: Destination::SameFolder,
            ..Default::default()
        };
        assert!(compress(&source, &settings).is_err());
        assert!(!output_path(&source, &settings).unwrap().exists());
        settings.resize = true;
        settings.long_edge = 16;
        let output = compress(&source, &settings).unwrap();
        assert_eq!(image::image_dimensions(output.path).unwrap(), (16, 8));
        assert!(source.path.exists());
    }

    #[test]
    fn optional_conversion_preserves_each_original_format() {
        let dir = tempfile::tempdir().unwrap();
        for format in OutputFormat::ALL {
            // AVIF è solo in uscita.
            if format == OutputFormat::Avif {
                continue;
            }
            let path = dir.path().join(format!("input.{}", format.extension()));
            DynamicImage::new_rgb8(8, 4).save(&path).unwrap();
            let meta = fs::metadata(&path).unwrap();
            let source = Source {
                path,
                root: dir.path().to_owned(),
                bytes: meta.len(),
                modified: meta.modified().ok(),
                format: format!("{format:?}"),
                dimensions: [8, 4],
            };
            let settings = Settings {
                destination: Destination::SameFolder,
                // Ignorato se l'utente non sceglie di convertire.
                format: OutputFormat::WebP,
                ..Default::default()
            };
            let result = compress(&source, &settings).unwrap();
            assert_eq!(
                image::ImageReader::open(&result.path).unwrap().format(),
                Some(image_format(format))
            );
            assert!(source.path.exists());
        }
    }

    #[test]
    fn conversion_preserves_all_eight_bit_alpha_levels() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        let expected = image::open(&source.path).unwrap().to_rgba8();
        for format in [
            OutputFormat::Png,
            OutputFormat::WebP,
            OutputFormat::Bmp,
            OutputFormat::Tiff,
        ] {
            let settings = Settings {
                destination: Destination::SameFolder,
                convert_format: true,
                format,
                ..Default::default()
            };
            let result = compress(&source, &settings).unwrap();
            let output = image::open(&result.path).unwrap().to_rgba8();
            assert_eq!(output.dimensions(), expected.dimensions());
            for (input, output) in expected.pixels().zip(output.pixels()) {
                assert_eq!(input[3], output[3], "Alpha non conservato in {format:?}");
            }
            assert!(result.warning.is_none());
        }
    }

    #[test]
    fn png_and_tiff_keep_sixteen_bit_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture(dir.path());
        let original = image::ImageBuffer::from_fn(4, 1, |x, _| {
            image::Rgba([1000_u16, 2000, 3000, [0, 1, 32768, 65534][x as usize]])
        });
        DynamicImage::ImageRgba16(original.clone())
            .save(&source.path)
            .unwrap();
        let meta = fs::metadata(&source.path).unwrap();
        source.bytes = meta.len();
        source.modified = meta.modified().ok();
        for format in [OutputFormat::Png, OutputFormat::Tiff] {
            let settings = Settings {
                destination: Destination::SameFolder,
                convert_format: true,
                format,
                ..Default::default()
            };
            let result = compress(&source, &settings).unwrap();
            let output = image::open(result.path).unwrap();
            assert_eq!(output.color(), image::ColorType::Rgba16);
            assert_eq!(output.to_rgba16(), original);
        }
    }

    #[test]
    fn transparent_images_convert_immediately_to_jpeg_and_gif() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        let original = fs::read(&source.path).unwrap();
        for format in [OutputFormat::Jpeg, OutputFormat::Gif] {
            let settings = Settings {
                destination: Destination::SameFolder,
                convert_format: true,
                format,
                ..Default::default()
            };
            let result = compress(&source, &settings).unwrap();
            assert!(result.path.exists());
            assert!(result.warning.is_none());
            assert_eq!(fs::read(&source.path).unwrap(), original);
        }
    }

    #[test]
    fn transparency_detection_checks_actual_alpha_at_native_precision() {
        let barely_transparent = DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
            1,
            1,
            image::Rgba([0, 0, 0, 65534]),
        ));
        assert!(has_transparency(&barely_transparent));
        let opaque = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([40, 120, 200, 255]),
        ));
        assert!(!has_transparency(&opaque));
        let grayscale = DynamicImage::ImageLumaA8(image::ImageBuffer::from_pixel(
            1,
            1,
            image::LumaA([100, 128]),
        ));
        assert!(has_transparency(&grayscale));
    }

    #[test]
    fn jpeg_automatically_composites_transparency_on_white() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            convert_format: true,
            format: OutputFormat::Jpeg,
            quality: 100,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        assert!(result.warning.is_none());
        let output = image::open(result.path).unwrap().to_rgb8();
        let pixel = output.get_pixel(0, 0);
        assert!(pixel[0] >= 250);
        assert!((pixel[1] as i32 - 127).abs() <= 3);
        assert!((pixel[2] as i32 - 127).abs() <= 3);
        assert!(source.path.exists());
    }

    #[test]
    fn jpeg_fully_transparent_pixels_become_white() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture(dir.path());
        image::RgbaImage::from_pixel(8, 4, image::Rgba([0, 0, 0, 0]))
            .save(&source.path)
            .unwrap();
        let meta = fs::metadata(&source.path).unwrap();
        source.bytes = meta.len();
        source.modified = meta.modified().ok();
        let settings = Settings {
            destination: Destination::SameFolder,
            convert_format: true,
            format: OutputFormat::Jpeg,
            quality: 100,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        let output = image::open(result.path).unwrap().to_rgb8();
        assert!(output.pixels().all(|p| p.0.iter().all(|&c| c >= 254)));
        assert!(result.warning.is_none());
    }

    #[test]
    fn gif_automatically_composites_partial_alpha_on_white() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        let original = image::open(&source.path).unwrap().to_rgba8();
        let settings = Settings {
            destination: Destination::SameFolder,
            convert_format: true,
            format: OutputFormat::Gif,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        let output = image::open(result.path).unwrap().to_rgba8();
        for (input, output) in original.pixels().zip(output.pixels()) {
            if input[3] == 0 {
                assert_eq!(output[3], 0);
            } else {
                assert_eq!(output[3], 255);
                assert_eq!(output.0[..3], composite_on_white(*input).0);
            }
        }
        assert!(result.warning.is_none());
    }

    #[test]
    fn gif_preserves_binary_transparency() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture(dir.path());
        image::RgbaImage::from_fn(8, 4, |x, _| {
            image::Rgba([40, 120, 200, if x < 4 { 0 } else { 255 }])
        })
        .save(&source.path)
        .unwrap();
        let meta = fs::metadata(&source.path).unwrap();
        source.bytes = meta.len();
        source.modified = meta.modified().ok();
        let settings = Settings {
            destination: Destination::SameFolder,
            convert_format: true,
            format: OutputFormat::Gif,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        let output = image::open(result.path).unwrap().to_rgba8();
        for (x, _, pixel) in output.enumerate_pixels() {
            assert_eq!(pixel[3], if x < 4 { 0 } else { 255 });
        }
        assert!(result.warning.is_none());
    }

    #[test]
    fn transparent_avif_converts_automatically() {
        let dir = tempfile::tempdir().unwrap();
        let source = alpha_fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            convert_format: true,
            format: OutputFormat::Avif,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        assert!(result.output_bytes > 0);
        assert!(result.warning.is_none());
    }

    #[test]
    fn preserves_relative_structure() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture(dir.path());
        source.root = PathBuf::from("/photos");
        source.path = PathBuf::from("/photos/holiday/a.png");
        // Cartella separata: il suffisso nomina la cartella, non i file.
        assert_eq!(
            output_path(&source, &Settings::default()).unwrap(),
            PathBuf::from("/photos_compressed/holiday/a.png")
        );
        // Accanto agli originali: il suffisso distingue le copie.
        let mut settings = Settings::default();
        settings.destination = Destination::SameFolder;
        assert_eq!(
            output_path(&source, &settings).unwrap(),
            PathBuf::from("/photos/holiday/a_compressed.png")
        );
    }

    #[test]
    fn collision_does_not_overwrite_and_resize_keeps_ratio() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            format: OutputFormat::Png,
            resize: true,
            long_edge: 16,
            ..Default::default()
        };
        let first = compress(&source, &settings).unwrap();
        let second = compress(&source, &settings).unwrap();
        assert_ne!(first.path, second.path);
        assert!(source.path.exists());
        assert_eq!(image::image_dimensions(second.path).unwrap(), (16, 8));
    }

    #[test]
    fn encodes_all_formats() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        for format in OutputFormat::ALL {
            let settings = Settings {
                destination: Destination::SameFolder,
                convert_format: true,
                format,
                ..Default::default()
            };
            let result = compress(&source, &settings).unwrap();
            assert!(result.output_bytes > 0);
            assert_eq!(
                image::ImageReader::open(result.path)
                    .unwrap()
                    .with_guessed_format()
                    .unwrap()
                    .format(),
                Some(image_format(format))
            );
        }
    }

    #[test]
    fn replacement_with_conversion_removes_only_original() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let occupied = dir.path().join("test.jpg");
        fs::write(&occupied, b"keep me").unwrap();
        let settings = Settings {
            destination: Destination::Replace,
            convert_format: true,
            format: OutputFormat::Jpeg,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        assert!(!source.path.exists());
        assert!(result.path.exists());
        assert_eq!(fs::read(occupied).unwrap(), b"keep me");
    }

    #[test]
    fn changed_source_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        fs::write(&source.path, b"changed").unwrap();
        assert!(compress(&source, &Settings::default()).is_err());
        assert_eq!(fs::read(&source.path).unwrap(), b"changed");
    }

    #[test]
    fn same_format_replacement_is_decodable() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::Replace,
            format: OutputFormat::Png,
            ..Default::default()
        };
        let result = compress(&source, &settings).unwrap();
        assert_eq!(result.path, source.path);
        assert!(image::open(result.path).is_ok());
    }
}
