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
}

impl OutputFormat {
    pub const ALL: [Self; 6] = [
        Self::Jpeg,
        Self::Png,
        Self::WebP,
        Self::Avif,
        Self::Bmp,
        Self::Tiff,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::WebP => "WebP",
            Self::Avif => "AVIF",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
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
        }
    }
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
    pub quality: u8,
    pub format: OutputFormat,
    pub destination: Destination,
    pub suffix: String,
    pub strip_metadata: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resize: false,
            long_edge: 1920,
            resize_edge: ResizeEdge::Long,
            quality: 80,
            format: OutputFormat::Jpeg,
            destination: Destination::SiblingFolder,
            suffix: "_compressed".into(),
            strip_metadata: true,
        }
    }
}

impl Settings {
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
            bail!("Per la cartella separata è necessario un suffisso non vuoto.");
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
    }
}
