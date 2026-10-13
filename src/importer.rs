use crate::model::Source;
use anyhow::{bail, Context, Result};
use image::ImageReader;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::mpsc::SyncSender,
};
use walkdir::WalkDir;

pub struct ImportedImage {
    pub source: Source,
    pub preview: image::RgbaImage,
}

pub enum ImportEvent {
    Image(ImportedImage),
    Error(String),
    Finished,
}

pub fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            ["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"]
                .contains(&ext.to_ascii_lowercase().as_str())
        })
}

/// Decodifica e miniature fuori dal thread UI. Non segue link simbolici.
pub fn import(
    paths: Vec<PathBuf>,
    recursive: bool,
    known: HashSet<PathBuf>,
    tx: SyncSender<ImportEvent>,
) {
    let mut seen = known;
    for path in paths {
        let canonical = match path.canonicalize() {
            Ok(path) => path,
            Err(err) => {
                let _ = tx.send(ImportEvent::Error(format!("{}: {err}", path.display())));
                continue;
            }
        };
        if canonical.is_dir() {
            let depth = if recursive { usize::MAX } else { 1 };
            for entry in WalkDir::new(&canonical)
                .follow_links(false)
                .max_depth(depth)
                .sort_by_file_name()
            {
                match entry {
                    Ok(entry) if entry.file_type().is_file() && supported(entry.path()) => {
                        if !emit(entry.path(), &canonical, &mut seen, &tx) {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(ImportEvent::Error(err.to_string()));
                    }
                    _ => {}
                }
            }
        } else if supported(&canonical) {
            if let Some(root) = canonical.parent() {
                if !emit(&canonical, root, &mut seen, &tx) {
                    return;
                }
            }
        } else {
            let _ = tx.send(ImportEvent::Error(format!(
                "Formato non supportato: {}",
                canonical.display()
            )));
        }
    }
    let _ = tx.send(ImportEvent::Finished);
}

fn emit(
    path: &Path,
    root: &Path,
    seen: &mut HashSet<PathBuf>,
    tx: &SyncSender<ImportEvent>,
) -> bool {
    if !seen.insert(path.to_owned()) {
        return true;
    }
    let event = match load(path, root) {
        Ok(image) => ImportEvent::Image(image),
        Err(err) => ImportEvent::Error(format!("{}: {err:#}", path.display())),
    };
    tx.send(event).is_ok()
}

pub fn reader(path: &Path) -> Result<ImageReader<std::io::BufReader<fs::File>>> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    // Limita memoria decodificata e dimensioni per file danneggiati o ostili.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader)
}

fn load(path: &Path, root: &Path) -> Result<ImportedImage> {
    let metadata = fs::metadata(path)?;
    let reader = reader(path)?;
    let format = reader.format().context("Formato sconosciuto")?;
    // Non appiattire animazioni/multipagina silenziosamente in modalità sostituzione.
    // Il formato originale rimane visibile e la UI spiega la conversione del primo frame.
    let (image, _) =
        crate::metadata::decode(path, true).context("Impossibile decodificare l'immagine")?;
    if image.width() == 0 || image.height() == 0 {
        bail!("Immagine vuota");
    }
    Ok(ImportedImage {
        source: Source {
            path: path.to_owned(),
            root: root.to_owned(),
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            format: format!("{format:?}"),
            dimensions: [image.width(), image.height()],
        },
        preview: image.thumbnail(480, 360).to_rgba8(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn collect(paths: Vec<PathBuf>, recursive: bool) -> Vec<ImportEvent> {
        let (tx, rx) = mpsc::sync_channel(8);
        let worker = std::thread::spawn(move || import(paths, recursive, HashSet::new(), tx));
        let events = rx.iter().collect();
        worker.join().unwrap();
        events
    }

    #[test]
    fn recursion_is_optional_and_preserves_root() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        fs::create_dir(&nested).unwrap();
        let image = image::RgbImage::new(10, 20);
        image.save(dir.path().join("a.png")).unwrap();
        image.save(nested.join("b.png")).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let main_only = collect(vec![root.clone()], false);
        assert_eq!(
            main_only
                .iter()
                .filter(|event| matches!(event, ImportEvent::Image(_)))
                .count(),
            1
        );
        let recursive = collect(vec![root.clone()], true);
        let images: Vec<_> = recursive
            .iter()
            .filter_map(|event| match event {
                ImportEvent::Image(image) => Some(image),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 2);
        assert!(images.iter().all(|image| image.source.root == root));
    }

    #[test]
    fn duplicate_paths_are_loaded_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbImage::new(2, 2).save(&path).unwrap();
        let events = collect(vec![path.clone(), path], false);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ImportEvent::Image(_)))
                .count(),
            1
        );
    }

    #[test]
    fn corrupted_image_reports_error_and_completes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.png");
        fs::write(&path, b"not an image").unwrap();
        let events = collect(vec![path], false);
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::Error(_))));
        assert!(matches!(events.last(), Some(ImportEvent::Finished)));
    }
}
