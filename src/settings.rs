use anyhow::{bail, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    Jpeg,
    Png,
    WebP,
    Avif,
    Bmp,
    Tiff,
    Gif,
}

impl OutputFormat {
    pub const ALL: [Self; 7] = [
        Self::Jpeg,
        Self::Png,
        Self::WebP,
        Self::Avif,
        Self::Bmp,
        Self::Tiff,
        Self::Gif,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::WebP => "WebP",
            Self::Avif => "AVIF",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Gif => "GIF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
            Self::Avif => "avif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Gif => "gif",
        }
    }

    pub fn from_source(format: &str) -> Result<Self> {
        match format {
            "Jpeg" => Ok(Self::Jpeg),
            "Png" => Ok(Self::Png),
            "WebP" => Ok(Self::WebP),
            "Avif" => Ok(Self::Avif),
            "Bmp" => Ok(Self::Bmp),
            "Tiff" => Ok(Self::Tiff),
            "Gif" => Ok(Self::Gif),
            _ => bail!(
                "Formato originale non supportato: {format}. Scegli un formato di conversione."
            ),
        }
    }
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Destination {
    SiblingFolder,
    SameFolder,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ResizeEdge {
    #[default]
    Long,
    Short,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub resize: bool,
    /// Target in pixel per il lato selezionato. Mantiene il nome JSON storico
    /// per leggere senza migrazioni anche le preferenze della prima versione.
    pub long_edge: u32,
    pub resize_edge: ResizeEdge,
    pub compression: bool,
    pub quality: u8,
    /// Le preferenze precedenti convertivano sempre nel formato selezionato.
    #[serde(default = "enabled")]
    pub convert_format: bool,
    pub format: OutputFormat,
    pub destination: Destination,
    pub suffix: String,
    pub strip_metadata: bool,
    /// Opzioni del recupero da pagina web, con valori predefiniti
    /// retrocompatibili per le preferenze delle versioni precedenti.
    pub web_min_kb: u64,
    pub web_max_pages: u32,
    pub web_download_dir: Option<PathBuf>,
    #[cfg(target_os = "macos")]
    pub skipped_update: Option<String>,
    /// Se falso, all'avvio non viene contattata la rete per cercare
    /// aggiornamenti; resta possibile il controllo manuale dalle impostazioni.
    #[cfg(target_os = "macos")]
    #[serde(default = "enabled")]
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resize: false,
            long_edge: 1920,
            resize_edge: ResizeEdge::Long,
            compression: true,
            quality: 80,
            convert_format: false,
            format: OutputFormat::Png,
            destination: Destination::SiblingFolder,
            suffix: "_compressed".into(),
            strip_metadata: true,
            web_min_kb: 50,
            web_max_pages: 10,
            web_download_dir: None,
            #[cfg(target_os = "macos")]
            skipped_update: None,
            #[cfg(target_os = "macos")]
            check_updates: true,
        }
    }
}

impl Settings {
    pub fn has_operation(&self) -> bool {
        self.compression || self.convert_format || self.resize
    }

    pub fn action_label(&self) -> &'static str {
        match (self.compression, self.convert_format, self.resize) {
            (true, true, _) => "Comprimi e converti",
            (true, false, _) => "Comprimi immagini",
            (false, true, _) => "Converti immagini",
            (false, false, true) => "Ridimensiona immagini",
            _ => "Elabora immagini",
        }
    }

    pub fn encoding_quality(&self) -> u8 {
        if self.compression {
            self.quality
        } else {
            100
        }
    }

    pub fn output_format(&self, source_format: &str) -> Result<OutputFormat> {
        if self.convert_format {
            Ok(self.format)
        } else {
            OutputFormat::from_source(source_format)
        }
    }

    pub fn output_label(&self) -> &'static str {
        if self.convert_format {
            self.format.label()
        } else {
            "Formato originale"
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.quality > 100 || self.long_edge == 0 || self.long_edge > 100_000 {
            bail!("Qualità o dimensione non valida.");
        }
        if self
            .suffix
            .chars()
            .any(|c| c.is_control() || "/\\<>:\"|?*".contains(c))
            || self.suffix.ends_with(['.', ' '])
            || self.suffix.len() > 80
        {
            bail!("Il suffisso contiene caratteri non validi (o supera 80 byte).");
        }
        if self.destination == Destination::SiblingFolder && self.suffix.trim().is_empty() {
            bail!("La cartella separata richiede un nome non vuoto.");
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = settings_path()?;
        let legacy_path = legacy_settings_path()?;
        let source_path = if path.exists() {
            path
        } else if legacy_path.exists() {
            legacy_path
        } else {
            return Ok(Self::default());
        };
        let settings: Self = serde_json::from_slice(&fs::read(source_path)?)
            .context("Impostazioni JSON non leggibili")?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = settings_path()?;
        let parent = path.parent().context("Percorso impostazioni non valido")?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(&serde_json::to_vec_pretty(self)?)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|err| err.error)?;
        Ok(())
    }
}

pub fn settings_path() -> Result<PathBuf> {
    Ok(ProjectDirs::from("it", "Marzianetto", "marzianetto")
        .context("Cartella delle impostazioni non disponibile")?
        .config_dir()
        .join("settings.json"))
}

fn legacy_settings_path() -> Result<PathBuf> {
    Ok(
        ProjectDirs::from("it", "ImageCompressor", "ImageCompressor")
            .context("Cartella delle impostazioni precedente non disponibile")?
            .config_dir()
            .join("settings.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_suffix() {
        let settings = Settings {
            suffix: "../../escape".into(),
            ..Default::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn old_settings_receive_defaults() {
        let settings: Settings = serde_json::from_str("{\"quality\":42}").unwrap();
        assert_eq!(settings.quality, 42);
        assert_eq!(settings.long_edge, 1920);
        assert_eq!(settings.resize_edge, ResizeEdge::Long);
        assert!(settings.strip_metadata);
        assert!(settings.convert_format);
        assert!(settings.compression);
        assert_eq!(settings.web_min_kb, 50);
        assert_eq!(settings.web_max_pages, 10);
        assert!(settings.web_download_dir.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn skipped_update_is_saved_and_defaults_to_none() {
        assert!(serde_json::from_str::<Settings>("{}")
            .unwrap()
            .skipped_update
            .is_none());
        let settings = Settings {
            skipped_update: Some("v0.1.2".into()),
            ..Default::default()
        };
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(restored.skipped_update, settings.skipped_update);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn update_check_is_on_by_default_and_can_be_disabled() {
        assert!(
            serde_json::from_str::<Settings>("{}")
                .unwrap()
                .check_updates
        );
        let settings = Settings {
            check_updates: false,
            ..Default::default()
        };
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert!(!restored.check_updates);
    }

    #[test]
    fn conversion_is_optional_and_preferences_round_trip() {
        let mut settings = Settings::default();
        assert!(!settings.convert_format);
        for format in OutputFormat::ALL {
            assert_eq!(
                settings.output_format(&format!("{:?}", format)).unwrap(),
                format
            );
        }
        settings.convert_format = true;
        settings.format = OutputFormat::WebP;
        assert_eq!(settings.output_format("Jpeg").unwrap(), OutputFormat::WebP);
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored, settings);
        let legacy: Settings = serde_json::from_str(r#"{"format":"Jpeg"}"#).unwrap();
        assert!(legacy.convert_format);
        assert_eq!(legacy.output_format("Png").unwrap(), OutputFormat::Jpeg);
    }

    #[test]
    fn compression_and_conversion_are_independent_and_saved() {
        for compression in [false, true] {
            for convert_format in [false, true] {
                let settings = Settings {
                    compression,
                    convert_format,
                    quality: 17,
                    format: OutputFormat::WebP,
                    ..Default::default()
                };
                assert_eq!(settings.has_operation(), compression || convert_format);
                assert_eq!(
                    settings.encoding_quality(),
                    if compression { 17 } else { 100 }
                );
                assert_eq!(
                    settings.output_format("Png").unwrap(),
                    if convert_format {
                        OutputFormat::WebP
                    } else {
                        OutputFormat::Png
                    }
                );
                let restored: Settings =
                    serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
                assert_eq!(restored, settings);
            }
        }
        let resize_only = Settings {
            compression: false,
            convert_format: false,
            resize: true,
            ..Default::default()
        };
        assert!(resize_only.has_operation());
        assert_eq!(resize_only.action_label(), "Ridimensiona immagini");
    }
}
