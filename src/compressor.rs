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
    let parent = source
        .path
        .parent()
        .context("Cartella di origine mancante")?;
    let stem = source.path.file_stem().context("Nome file mancante")?;
    let mut name = stem.to_os_string();
    if settings.destination != Destination::Replace {
        name.push(&settings.suffix);
    }
    name.push(format!(".{}", settings.format.extension()));
    match settings.destination {
        Destination::SameFolder => Ok(parent.join(name)),
        Destination::Replace => {
            // Mantiene l'estensione originale, anche .jpeg/.JPG, se il codec non cambia.
            let same_codec = image::ImageFormat::from_path(&source.path).ok()
                == Some(image_format(settings.format));
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
    }
}

pub fn compress(source: &Source, settings: &Settings) -> Result<CompressionResult> {
    settings.validate()?;
    check_source(source)?;
    if settings.destination == Destination::Replace
        && matches!(source.format.as_str(), "Gif" | "Tiff")
    {
        bail!("Sostituzione disabilitata per GIF/TIFF: possono contenere più frame o pagine. Usa una copia.");
    }
    let (image, mut metadata) = metadata::decode(&source.path, settings.strip_metadata)?;
    let image = resize(image, settings);
    let target = output_path(source, settings)?;
    let parent = target.parent().context("Destinazione non valida")?;
    fs::create_dir_all(parent)?;
    // Scrive prima su un file temporaneo nello stesso filesystem. L'originale
    // non viene toccato finché codifica, flush e sync non sono terminati.
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    encode(&image, settings, temp.as_file_mut(), &mut metadata)?;
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
    settings: &Settings,
    file: &mut fs::File,
    metadata: &mut Metadata,
) -> Result<()> {
    let mut writer = BufWriter::new(file);
    match settings.format {
        OutputFormat::Jpeg => {
            // JPEG non supporta alpha: composizione su bianco, non su nero.
            let rgba = image.to_rgba8();
            let rgb = image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
                let p = rgba.get_pixel(x, y).0;
                let a = p[3] as u32;
                image::Rgb(std::array::from_fn(|i| {
                    ((p[i] as u32 * a + 255 * (255 - a) + 127) / 255) as u8
                }))
            });
            let mut encoder = JpegEncoder::new_with_quality(&mut writer, settings.quality.max(1));
            metadata::apply(&mut encoder, metadata);
            encoder.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        OutputFormat::Png => {
            let rgba = image.to_rgba8();
            let compression = match settings.quality {
                0..=33 => CompressionType::Fast,
                34..=66 => CompressionType::Default,
                _ => CompressionType::Best,
            };
            let mut encoder =
                PngEncoder::new_with_quality(&mut writer, compression, FilterType::Adaptive);
            metadata::apply(&mut encoder, metadata);
            encoder.write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )?;
        }
        OutputFormat::WebP => {
            metadata::unsupported(metadata);
            let rgba = image.to_rgba8();
            let encoded = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height())
                .encode(settings.quality as f32);
            writer.write_all(&encoded)?;
        }
        OutputFormat::Avif => {
            let rgba = image.to_rgba8();
            let mut encoder =
                AvifEncoder::new_with_speed_quality(&mut writer, 6, settings.quality.max(1));
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
            image.write_to(&mut writer, image_format(settings.format))?;
        }
        OutputFormat::Tiff => {
            let rgba = image.to_rgba8();
            let mut encoder = image::codecs::tiff::TiffEncoder::new(&mut writer);
            metadata::apply(&mut encoder, metadata);
            encoder.write_image(
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

    #[test]
    fn preserves_relative_structure() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = fixture(dir.path());
        source.root = PathBuf::from("/photos");
        source.path = PathBuf::from("/photos/holiday/a.png");
        let settings = Settings::default();
        assert_eq!(
            output_path(&source, &settings).unwrap(),
            PathBuf::from("/photos_compressed/holiday/a_compressed.jpg")
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
