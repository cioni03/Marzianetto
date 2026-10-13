//! La ricodifica parte solo dai pixel. I metadati vengono reinseriti soltanto
//! quando richiesto: nessuna copia opaca del contenitore originale.
use anyhow::Result;
use image::{metadata::Orientation, DynamicImage, ImageDecoder, ImageEncoder};
use std::path::Path;

#[derive(Default)]
pub struct Metadata {
    pub exif: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,
    pub warnings: Vec<String>,
}

pub fn decode(path: &Path, strip: bool) -> Result<(DynamicImage, Metadata)> {
    let mut decoder = crate::importer::reader(path)?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut metadata = Metadata::default();
    if !strip {
        metadata.exif = decoder.exif_metadata()?;
        metadata.icc = decoder.icc_profile()?;
        if decoder.xmp_metadata()?.is_some() {
            metadata
                .warnings
                .push("XMP non conservato dall'encoder.".into());
        }
        if let Some(exif) = &mut metadata.exif {
            // Evita che un visualizzatore ruoti nuovamente i pixel già orientati.
            let _ = Orientation::remove_from_exif_chunk(exif);
        }
    }
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok((image, metadata))
}

pub fn apply(encoder: &mut impl ImageEncoder, metadata: &mut Metadata) {
    if let Some(exif) = &metadata.exif {
        if encoder.set_exif_metadata(exif.clone()).is_err() {
            metadata
                .warnings
                .push("EXIF non supportato dal formato di output: non conservato.".into());
        }
    }
    if let Some(icc) = &metadata.icc {
        if encoder.set_icc_profile(icc.clone()).is_err() {
            metadata
                .warnings
                .push("ICC non supportato dal formato di output: non conservato.".into());
        }
    }
}

pub fn unsupported(metadata: &mut Metadata) {
    if metadata.exif.is_some() {
        metadata
            .warnings
            .push("EXIF non conservato da questo encoder.".into());
    }
    if metadata.icc.is_some() {
        metadata
            .warnings
            .push("ICC non conservato da questo encoder.".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compressor,
        model::Source,
        settings::{Destination, OutputFormat, Settings},
    };
    use std::fs;

    fn fixture(dir: &Path) -> Source {
        let path = dir.join("private.jpg");
        // TIFF little-endian: Orientation=6 e ImageDescription privata.
        let description = b"private camera description\0";
        let mut exif = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 2, 0];
        exif.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
        exif.extend_from_slice(&[0x0e, 0x01, 2, 0]);
        exif.extend_from_slice(&(description.len() as u32).to_le_bytes());
        exif.extend_from_slice(&38_u32.to_le_bytes());
        exif.extend_from_slice(&[0; 4]);
        exif.extend_from_slice(description);
        let image = image::RgbImage::from_pixel(12, 8, image::Rgb([50, 100, 150]));
        let mut file = fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut file);
        encoder.set_exif_metadata(exif).unwrap();
        encoder
            .set_icc_profile(b"private ICC profile".to_vec())
            .unwrap();
        encoder
            .encode(image.as_raw(), 12, 8, image::ExtendedColorType::Rgb8)
            .unwrap();
        drop(file);
        let meta = fs::metadata(&path).unwrap();
        Source {
            path,
            root: dir.to_owned(),
            bytes: meta.len(),
            modified: meta.modified().ok(),
            format: "Jpeg".into(),
            dimensions: [8, 12],
        }
    }

    #[test]
    fn stripping_removes_exif_icc_and_normalizes_orientation() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            strip_metadata: true,
            ..Default::default()
        };
        let result = compressor::compress(&source, &settings).unwrap();
        let mut decoder = crate::importer::reader(&result.path)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.dimensions(), (8, 12));
        assert!(decoder.exif_metadata().unwrap().is_none());
        assert!(decoder.icc_profile().unwrap().is_none());
        assert!(decoder.xmp_metadata().unwrap().is_none());
        assert!(!fs::read(result.path)
            .unwrap()
            .windows(b"private".len())
            .any(|bytes| bytes == b"private"));
    }

    #[test]
    fn privacy_option_strips_metadata_for_every_output_codec() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        for format in OutputFormat::ALL {
            let settings = Settings {
                destination: Destination::SameFolder,
                format,
                strip_metadata: true,
                ..Default::default()
            };
            let result = compressor::compress(&source, &settings).unwrap();
            assert!(result.warning.is_none());
            assert!(!fs::read(&result.path)
                .unwrap()
                .windows(7)
                .any(|bytes| bytes == b"private"));
            // AVIF è solo in uscita; gli altri codec possono essere riletti.
            if format != OutputFormat::Avif {
                let mut decoder = crate::importer::reader(&result.path)
                    .unwrap()
                    .into_decoder()
                    .unwrap();
                assert!(decoder.exif_metadata().unwrap().is_none());
                assert!(decoder.icc_profile().unwrap().is_none());
                assert!(decoder.xmp_metadata().unwrap().is_none());
                assert_eq!(decoder.dimensions(), (8, 12));
            }
        }
    }

    #[test]
    fn disabled_stripping_preserves_supported_metadata_without_double_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            strip_metadata: false,
            ..Default::default()
        };
        let result = compressor::compress(&source, &settings).unwrap();
        let mut decoder = crate::importer::reader(&result.path)
            .unwrap()
            .into_decoder()
            .unwrap();
        assert_eq!(decoder.dimensions(), (8, 12));
        assert_eq!(decoder.orientation().unwrap(), Orientation::NoTransforms);
        assert!(decoder.exif_metadata().unwrap().is_some());
        assert_eq!(
            decoder.icc_profile().unwrap().unwrap(),
            b"private ICC profile"
        );
        assert!(result.warning.is_none());
    }

    #[test]
    fn unsupported_metadata_produces_warning_not_false_original_retention() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path());
        let settings = Settings {
            destination: Destination::SameFolder,
            format: OutputFormat::WebP,
            strip_metadata: false,
            ..Default::default()
        };
        let result = compressor::compress(&source, &settings).unwrap();
        assert!(result.warning.unwrap().contains("EXIF"));
        assert!(!result.original_retained);
    }
}
