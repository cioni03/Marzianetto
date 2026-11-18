use crate::{
    compressor::{self, CompressionEvent},
    importer::{self, ImportEvent},
    integration,
    model::{format_bytes, format_savings, savings_percent, CompressionResult, Source},
    settings::{Destination, OutputFormat, ResizeEdge, Settings},
    theme,
};
use eframe::egui::{self, Color32, RichText, TextureHandle, Vec2};
use egui_extras::{Column, TableBuilder};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::{Duration, Instant},
};

enum Status {
    Ready,
    Working,
    Done(CompressionResult),
    Error(String),
}

struct Row {
    source: Source,
    texture: TextureHandle,
    status: Status,
    replaced: bool,
}

#[derive(Default)]
struct Summary {
    input: u64,
    output: u64,
    succeeded: usize,
    failed: usize,
    retained: usize,
    cancelled: bool,
}

pub struct CompressorApp {
    logo: TextureHandle,
    settings: Settings,
    save_due: Option<Instant>,
    settings_error: Option<String>,
    rows: Vec<Row>,
    import_rx: Option<Receiver<ImportEvent>>,
    work_rx: Option<Receiver<CompressionEvent>>,
    dialog_rx: Option<Receiver<Option<Vec<PathBuf>>>>,
    pending_folders: VecDeque<Vec<PathBuf>>,
    confirm_replace: bool,
    confirm_close: bool,
    close_after_work: bool,
    cancel: Arc<AtomicBool>,
    completed: usize,
    summary: Option<Summary>,
    run_destination: Destination,
    messages: Vec<String>,
    show_messages: bool,
    show_integration: bool,
    integration_installed: bool,
    integration_rx: Option<Receiver<(bool, Result<String, String>)>>,
    integration_message: Option<String>,
}

impl CompressorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx);
        let logo_image =
            image::load_from_memory(include_bytes!("../packaging/assets/marzianetto.png"))
                .expect("embedded marzianetto icon must be a valid PNG")
                .to_rgba8();
        let logo = cc.egui_ctx.load_texture(
            "marzianetto-app-icon",
            egui::ColorImage::from_rgba_unmultiplied(
                [logo_image.width() as usize, logo_image.height() as usize],
                logo_image.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        );
        let (settings, settings_error) = match Settings::load() {
            Ok(settings) => (settings, None),
            Err(err) => (
                Settings::default(),
                Some(format!("Impostazioni non caricate: {err:#}")),
            ),
        };
        let mut app = Self {
            logo,
            settings,
            settings_error,
            save_due: None,
            rows: Vec::new(),
            import_rx: None,
            work_rx: None,
            dialog_rx: None,
            pending_folders: VecDeque::new(),
            confirm_replace: false,
            confirm_close: false,
            close_after_work: false,
            cancel: Arc::new(AtomicBool::new(false)),
            completed: 0,
            summary: None,
            run_destination: Destination::SiblingFolder,
            messages: Vec::new(),
            show_messages: false,
            show_integration: false,
            integration_installed: integration::installed(),
            integration_rx: None,
            integration_message: None,
        };
        if !paths.is_empty() {
            app.receive_paths(paths);
        }
        app
    }

    fn busy(&self) -> bool {
        self.import_rx.is_some()
            || self.work_rx.is_some()
            || self.dialog_rx.is_some()
            || self.integration_rx.is_some()
    }

    fn message(&mut self, text: String) {
        if self.messages.len() >= 200 {
            self.messages.remove(0);
        }
        self.messages.push(text);
    }

    fn choose(&mut self, folder: bool) {
        let (tx, rx) = mpsc::channel();
        self.dialog_rx = Some(rx);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_title(if folder {
                "Aggiungi cartella"
            } else {
                "Aggiungi immagini"
            });
            let paths = if folder {
                dialog.pick_folder().map(|path| vec![path])
            } else {
                dialog
                    .add_filter(
                        "Immagini",
                        &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"],
                    )
                    .pick_files()
            };
            let _ = tx.send(paths);
        });
    }

    fn receive_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.iter().any(|path| path.is_dir()) {
            self.pending_folders.push_back(paths);
        } else {
            self.start_import(paths, false);
        }
    }

    fn start_import(&mut self, paths: Vec<PathBuf>, recursive: bool) {
        let known: HashSet<_> = self
            .rows
            .iter()
            .map(|row| row.source.path.clone())
            .collect();
        // Backpressure: al massimo otto anteprime attendono l'upload sulla GPU.
        let (tx, rx) = mpsc::sync_channel(8);
        self.import_rx = Some(rx);
        self.summary = None;
        std::thread::spawn(move || importer::import(paths, recursive, known, tx));
    }

    fn start_compression(&mut self) {
        if let Err(err) = self.settings.validate() {
            self.message(err.to_string());
            self.show_messages = true;
            return;
        }
        let sources = self.rows.iter().map(|row| row.source.clone()).collect();
        for row in &mut self.rows {
            row.status = Status::Ready;
        }
        self.completed = 0;
        self.summary = None;
        self.run_destination = self.settings.destination;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let settings = self.settings.clone();
        let (tx, rx) = mpsc::channel();
        self.work_rx = Some(rx);
        std::thread::spawn(move || compressor::run(sources, settings, tx, cancel));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.integration_rx {
            match rx.try_recv() {
                Ok((installed, result)) => {
                    self.integration_rx = None;
                    self.integration_installed = installed;
                    self.integration_message = Some(
                        result.unwrap_or_else(|error| format!("Operazione non riuscita: {error}")),
                    );
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.integration_rx = None;
                    self.integration_message =
                        Some("Operazione interrotta inaspettatamente.".into());
                }
                _ => {}
            }
        }
        if let Some(rx) = &self.dialog_rx {
            match rx.try_recv() {
                Ok(paths) => {
                    self.dialog_rx = None;
                    if let Some(paths) = paths {
                        self.receive_paths(paths);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.dialog_rx = None;
                }
                _ => {}
            }
        }
        // Un numero limitato di upload per frame evita di bloccare il rendering.
        for _ in 0..12 {
            let event = self.import_rx.as_ref().map(|rx| rx.try_recv());
            match event {
                Some(Ok(ImportEvent::Image(imported))) => {
                    let size = [
                        imported.preview.width() as usize,
                        imported.preview.height() as usize,
                    ];
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(size, imported.preview.as_raw());
                    let texture = ctx.load_texture(
                        imported.source.path.to_string_lossy(),
                        image,
                        egui::TextureOptions::LINEAR,
                    );
                    self.rows.push(Row {
                        source: imported.source,
                        texture,
                        status: Status::Ready,
                        replaced: false,
                    });
                }
                Some(Ok(ImportEvent::Error(error))) => self.message(error),
                Some(Ok(ImportEvent::Finished)) => {
                    self.import_rx = None;
                    break;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.import_rx = None;
                    self.message("Importazione interrotta inaspettatamente.".into());
                    break;
                }
                _ => break,
            }
        }
        loop {
            let event = self.work_rx.as_ref().map(|rx| rx.try_recv());
            match event {
                Some(Ok(CompressionEvent::Started(index))) => {
                    if let Some(row) = self.rows.get_mut(index) {
                        row.status = Status::Working;
                    }
                }
                Some(Ok(CompressionEvent::Completed(index, result))) => {
                    self.completed += 1;
                    if let Some(row) = self.rows.get_mut(index) {
                        row.status = match result {
                            Ok(result) => {
                                row.replaced = self.run_destination == Destination::Replace;
                                Status::Done(result)
                            }
                            Err(error) => Status::Error(error),
                        };
                    }
                }
                Some(Ok(CompressionEvent::Finished { cancelled })) => {
                    self.work_rx = None;
                    self.finish_summary(cancelled);
                    break;
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.work_rx = None;
                    self.message("Elaborazione interrotta inaspettatamente.".into());
                    self.finish_summary(true);
                    break;
                }
                _ => break,
            }
        }
        if self
            .save_due
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.save_due = None;
            self.settings_error = self
                .settings
                .save()
                .err()
                .map(|err| format!("Salvataggio impostazioni: {err:#}"));
        }
        if self.busy() || self.save_due.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
        if self.close_after_work && self.work_rx.is_none() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn finish_summary(&mut self, cancelled: bool) {
        let mut summary = Summary {
            cancelled,
            ..Default::default()
        };
        for row in &self.rows {
            match &row.status {
                Status::Done(result) => {
                    summary.input += result.input_bytes;
                    summary.output += result.output_bytes;
                    summary.succeeded += 1;
                    summary.retained += usize::from(result.original_retained);
                }
                Status::Error(_) => summary.failed += 1,
                _ => {}
            }
        }
        self.summary = Some(summary);
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Impostazioni");
        ui.label(
            RichText::new("Preferenze salvate automaticamente")
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(theme::GAP);
        theme::card(ui, "01", "Operazioni", |ui| {
            ui.checkbox(&mut self.settings.convert_format, "Converti formato");
            if self.settings.convert_format {
                egui::ComboBox::from_id_salt("output_format")
                    .selected_text(self.settings.format.label())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for format in OutputFormat::ALL {
                            ui.selectable_value(&mut self.settings.format, format, format.label());
                        }
                    });
            } else {
                ui.label(
                    RichText::new("Ogni immagine mantiene il proprio formato originale.")
                        .small()
                        .color(theme::MUTED),
                );
            }
            ui.label(
                RichText::new(
                    "Trasparenza mantenuta dove supportata; altrimenti sfondo bianco automatico.",
                )
                .small()
                .color(theme::MUTED),
            );
            ui.checkbox(&mut self.settings.compression, "Comprimi");
            if self.settings.compression {
                ui.horizontal(|ui| {
                    ui.label(
                        if self.settings.convert_format && self.settings.format == OutputFormat::Png
                        {
                            "Compressione"
                        } else {
                            "Qualità"
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} / 100", self.settings.quality))
                                .strong()
                                .color(theme::ACCENT),
                        );
                    });
                });
                ui.spacing_mut().slider_width = ui.available_width();
                ui.add_enabled(
                    !self.settings.convert_format
                        || !matches!(
                            self.settings.format,
                            OutputFormat::Bmp | OutputFormat::Tiff | OutputFormat::Gif
                        ),
                    egui::Slider::new(&mut self.settings.quality, 0..=100).show_value(false),
                );
                let hint = match (self.settings.convert_format, self.settings.format) {
                    (false, _) => {
                        "Qualità applicata dove supportata. PNG è senza perdita; BMP/TIFF/GIF hanno qualità fissa."
                    }
                    (true, OutputFormat::Png) => {
                        "Senza perdita: veloce 0–33, normale 34–66, massima 67–100."
                    }
                    (true, OutputFormat::Bmp | OutputFormat::Tiff | OutputFormat::Gif) => {
                        "Qualità fissa. Il file potrebbe diventare più grande."
                    }
                    (true, OutputFormat::Jpeg) => "Più qualità, più dettaglio. Trasparenza su bianco.",
                    (true, OutputFormat::Avif) => {
                        "Con trasparenza, qualità 100 automatica per il canale alpha."
                    }
                    _ => "Più qualità, più dettaglio. Trasparenza conservata.",
                };
                ui.label(RichText::new(hint).small().color(theme::MUTED));
            } else if self.settings.convert_format || self.settings.resize {
                ui.label(
                    RichText::new("Solo codifica del formato: PNG non compresso, WebP lossless; JPEG/AVIF a qualità 100, non necessariamente senza perdita. GIF usa una palette limitata.")
                        .small()
                        .color(theme::MUTED),
                );
            }
        });
        theme::card(ui, "02", "Dimensioni", |ui| {
            ui.checkbox(&mut self.settings.resize, "Ridimensiona immagine");
            if self.settings.resize {
                ui.columns(2, |columns| {
                    for (column, (edge, label)) in columns.iter_mut().zip([
                        (ResizeEdge::Long, "Lato lungo"),
                        (ResizeEdge::Short, "Lato corto"),
                    ]) {
                        if column
                            .add_sized(
                                [column.available_width(), 34.0],
                                egui::Button::new(label)
                                    .selected(self.settings.resize_edge == edge),
                            )
                            .clicked()
                        {
                            self.settings.resize_edge = edge;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Dimensione massima");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_sized(
                            [100.0, 34.0],
                            egui::DragValue::new(&mut self.settings.long_edge)
                                .range(1..=100_000)
                                .suffix(" px")
                                .speed(10),
                        );
                    });
                });
            }
            ui.label(
                RichText::new("Proporzioni preservate, senza ingrandimenti.")
                    .small()
                    .color(theme::MUTED),
            );
        });
        theme::card(ui, "03", "Dove salvare", |ui| {
            for (destination, label) in [
                (Destination::SiblingFolder, "Cartella separata"),
                (Destination::SameFolder, "Accanto agli originali"),
                (Destination::Replace, "Sostituisci gli originali"),
            ] {
                let text = RichText::new(label).color(if destination == Destination::Replace {
                    theme::WARNING
                } else {
                    theme::TEXT
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(text).selected(self.settings.destination == destination),
                    )
                    .clicked()
                {
                    self.settings.destination = destination;
                }
            }
            ui.label(
                RichText::new(match self.settings.destination {
                    Destination::SiblingFolder => {
                        "Crea una cartella accanto all'origine e conserva le sottocartelle."
                    }
                    Destination::SameFolder => "Crea copie. Gli originali rimangono intatti.",
                    Destination::Replace => "Irreversibile: richiede conferma. Conserva un backup.",
                })
                .small()
                .color(if self.settings.destination == Destination::Replace {
                    theme::WARNING
                } else {
                    theme::MUTED
                }),
            );
            ui.label("Suffisso cartella / file");
            ui.add_enabled_ui(self.settings.destination != Destination::Replace, |ui| {
                ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::TextEdit::singleline(&mut self.settings.suffix),
                );
            });
            if self.settings.destination != Destination::Replace {
                ui.label(
                    RichText::new(format!(
                        "Esempio: foto{}.{}",
                        self.settings.suffix,
                        if self.settings.convert_format {
                            self.settings.format.extension()
                        } else {
                            "estensione originale"
                        }
                    ))
                    .small()
                    .color(theme::BLUE),
                );
            }
        });
        if let Err(err) = self.settings.validate() {
            ui.colored_label(theme::DANGER, err.to_string());
        }
        theme::card(ui, "04", "Privacy e metadati", |ui| {
            ui.checkbox(
                &mut self.settings.strip_metadata,
                "Rimuovi tutti i metadati",
            );
            ui.label(RichText::new(if self.settings.strip_metadata {
                "Non copia EXIF, GPS, ICC, XMP, commenti o miniature incorporate. Rimuovere ICC può cambiare la resa dei colori."
            } else {
                "Conserva EXIF e ICC dove supportati. Altri metadati non sono garantiti: verifica gli avvisi del risultato."
            }).small().color(theme::MUTED));
        });
        egui::CollapsingHeader::new("Note sui formati e preferenze").show(ui, |ui| {
            ui.label(RichText::new("GIF/TIFF: solo primo frame/pagina, senza sostituzione. L'orientamento viene applicato ai pixel prima della conversione.").small().color(theme::MUTED));
            if let Ok(path) = crate::settings::settings_path() {
                ui.label(RichText::new("Percorso impostazioni").small().color(theme::BLUE)).on_hover_text(path.display().to_string());
            }
        });
        if let Some(error) = &self.settings_error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn table(&self, ui: &mut egui::Ui) {
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(64.0))
            .column(Column::remainder().at_least(130.0))
            .column(Column::initial(68.0))
            .column(Column::initial(85.0))
            .column(Column::initial(85.0))
            .column(Column::initial(112.0))
            .header(36.0, |mut header| {
                for name in [
                    "Foto",
                    "Nome / percorso",
                    "Formato",
                    "Originale",
                    "Output",
                    "Risparmio",
                ] {
                    header.col(|ui| {
                        ui.label(RichText::new(name).small().strong().color(theme::MUTED));
                    });
                }
            })
            .body(|body| {
                body.rows(76.0, self.rows.len(), |mut table_row| {
                    let row = &self.rows[table_row.index()];
                    table_row.col(|ui| {
                        ui.add(
                            egui::Image::new(&row.texture)
                                .max_size(Vec2::new(56.0, 56.0))
                                .corner_radius(6),
                        )
                        .on_hover_ui(|ui| {
                            ui.add(
                                egui::Image::new(&row.texture).max_size(Vec2::new(480.0, 360.0)),
                            );
                            ui.strong(
                                row.source
                                    .path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                            );
                            ui.label(format!(
                                "Peso esatto: {} byte ({})",
                                row.source.bytes,
                                format_bytes(row.source.bytes)
                            ));
                            ui.label(format!(
                                "{} × {} px • {}",
                                row.source.dimensions[0],
                                row.source.dimensions[1],
                                row.source.format
                            ));
                            if row.replaced {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    "Anteprima dell'originale, ora sostituito.",
                                );
                            }
                        });
                    });
                    table_row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        row.source
                                            .path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy(),
                                    )
                                    .strong(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(row.source.path.display().to_string());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        row.source
                                            .path
                                            .parent()
                                            .unwrap_or(&row.source.path)
                                            .to_string_lossy(),
                                    )
                                    .small()
                                    .weak(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(row.source.path.display().to_string());
                        });
                    });
                    table_row.col(|ui| {
                        ui.label(RichText::new(&row.source.format).small().color(theme::BLUE));
                    });
                    table_row.col(|ui| {
                        ui.label(format_bytes(row.source.bytes))
                            .on_hover_text(format!("{} byte", row.source.bytes));
                    });
                    table_row.col(|ui| {
                        if let Status::Done(result) = &row.status {
                            ui.label(format_bytes(result.output_bytes))
                                .on_hover_text(format!(
                                    "{}\n{} byte",
                                    result.path.display(),
                                    result.output_bytes
                                ));
                        } else {
                            ui.weak("—");
                        }
                    });
                    table_row.col(|ui| match &row.status {
                        Status::Ready => {
                            ui.weak("In attesa");
                        }
                        Status::Working => {
                            ui.spinner();
                            ui.label("In corso");
                        }
                        Status::Error(error) => {
                            ui.colored_label(theme::DANGER, "Errore")
                                .on_hover_text(error);
                        }
                        Status::Done(result) => {
                            ui.vertical(|ui| {
                                let color = if result.saved_bytes() >= 0 {
                                    theme::ACCENT
                                } else {
                                    theme::WARNING
                                };
                                ui.colored_label(color, format!("{:.1}%", result.saved_percent()));
                                ui.label(
                                    RichText::new(format_savings(result.saved_bytes())).small(),
                                );
                                if let Some(warning) = &result.warning {
                                    ui.colored_label(Color32::YELLOW, "Avviso")
                                        .on_hover_text(warning);
                                }
                            });
                        }
                    });
                });
            });
    }

    fn popups(&mut self, ctx: &egui::Context) {
        if self.show_integration {
            let mut open = self.show_integration;
            egui::Window::new("Integrazione con il sistema").open(&mut open)
                .collapsible(false).resizable(false).default_width(480.0).show(ctx, |ui| {
                    ui.heading("Invia immagini dal menu contestuale");
                    ui.label(integration::description());
                    ui.add_space(theme::GAP);
                    ui.colored_label(if self.integration_installed { theme::ACCENT } else { theme::MUTED },
                        if self.integration_installed { "Installato per questo utente" } else { "Non ancora installato" });
                    ui.label(RichText::new("Nessun privilegio amministrativo. Le associazioni predefinite non cambiano. Se sposti l'eseguibile, reinstalla il menu.").small().color(theme::MUTED));
                    ui.horizontal(|ui| {
                        let ready = !self.busy() && std::env::var_os("FLATPAK_ID").is_none();
                        if ui.add_enabled(ready, theme::primary(if self.integration_installed { "Reinstalla menu" } else { "Installa menu" })).clicked() {
                            self.change_integration(true);
                        }
                        if ui.add_enabled(ready && self.integration_installed, egui::Button::new("Rimuovi").min_size(Vec2::new(120.0, 40.0))).clicked() {
                            self.change_integration(false);
                        }
                    });
                    if self.integration_rx.is_some() { ui.spinner(); }
                    if let Some(message) = &self.integration_message { ui.separator(); ui.label(message); }
                });
            self.show_integration = open;
        }
        if self.confirm_close {
            egui::Modal::new(egui::Id::new("close_prompt")).show(ctx, |ui| {
                ui.heading("Elaborazione in corso");
                ui.label("Attendi la fine del file in corso prima di chiudere.");
                ui.horizontal(|ui| {
                    if ui.button("Continua elaborazione").clicked() {
                        self.confirm_close = false;
                    }
                    if ui.button("Interrompi e chiudi dopo questo file").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                        self.close_after_work = true;
                        self.confirm_close = false;
                    }
                });
            });
        }
        if self.pending_folders.front().is_some() {
            egui::Modal::new(egui::Id::new("folder_prompt")).show(ctx, |ui| {
                ui.heading("Importa cartelle");
                ui.label("Includere le sottocartelle o solo la cartella principale?");
                if let Some(paths) = self.pending_folders.front() {
                    for path in paths.iter().filter(|path| path.is_dir()).take(4) {
                        ui.label(RichText::new(path.display().to_string()).small().weak());
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Includi sottocartelle").clicked() {
                        if let Some(paths) = self.pending_folders.pop_front() {
                            self.start_import(paths, true);
                        }
                    }
                    if ui.button("Solo principale").clicked() {
                        if let Some(paths) = self.pending_folders.pop_front() {
                            self.start_import(paths, false);
                        }
                    }
                    if ui.button("Annulla").clicked() {
                        self.pending_folders.pop_front();
                    }
                });
            });
        }
        if self.confirm_replace {
            egui::Modal::new(egui::Id::new("replace_prompt")).show(ctx, |ui| {
                ui.heading("Sostituire gli originali?");
                ui.label(format!("Stai per sostituire {} immagini. L'operazione non è annullabile.", self.rows.len()));
                ui.label("Conserva una copia di backup. Anche file più grandi possono sostituire gli originali.");
                ui.horizontal(|ui| {
                    if ui.button("Annulla").clicked() { self.confirm_replace = false; }
                    if ui.button(RichText::new("Conferma sostituzione").color(Color32::LIGHT_RED)).clicked() {
                        self.confirm_replace = false;
                        self.start_compression();
                    }
                });
            });
        }
        if self.show_messages {
            egui::Window::new("Messaggi e problemi di importazione")
                .open(&mut self.show_messages)
                .default_width(600.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(350.0)
                        .show(ui, |ui| {
                            for message in &self.messages {
                                ui.label(message);
                                ui.separator();
                            }
                        });
                });
        }
    }

    fn change_integration(&mut self, install: bool) {
        let (tx, rx) = mpsc::channel();
        self.integration_rx = Some(rx);
        self.integration_message = None;
        std::thread::spawn(move || {
            let result = if install {
                integration::install()
            } else {
                integration::uninstall()
            }
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send((integration::installed(), result));
        });
    }
}

impl eframe::App for CompressorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|input| input.viewport().close_requested()) && self.work_rx.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = !self.close_after_work;
        }
        let dropped: Vec<_> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            if self.busy() || !self.pending_folders.is_empty() || self.confirm_replace {
                self.message(
                    "Attendi il completamento dell'operazione prima di aggiungere file.".into(),
                );
            } else {
                self.receive_paths(dropped);
            }
        }
        let blocked = self.busy()
            || !self.pending_folders.is_empty()
            || self.confirm_replace
            || self.close_after_work;
        egui::TopBottomPanel::top("toolbar")
            .frame(theme::panel_frame())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    theme::logo(ui, &self.logo, 42.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("marzianetto").size(21.0).strong());
                        ui.label(
                            RichText::new(format!(
                                "Simple image compressor · build {}",
                                env!("MARZIANETTO_BUILD_NUMBER")
                            ))
                            .small()
                            .color(theme::MUTED),
                        );
                    });
                    ui.add_space(16.0);
                    if ui
                        .add_enabled(
                            !blocked,
                            egui::Button::new("+ Immagini").min_size(Vec2::new(112.0, 40.0)),
                        )
                        .clicked()
                    {
                        self.choose(false);
                    }
                    if ui
                        .add_enabled(
                            !blocked,
                            egui::Button::new("+ Cartella").min_size(Vec2::new(112.0, 40.0)),
                        )
                        .clicked()
                    {
                        self.choose(true);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !blocked,
                                egui::Button::new("Menu di sistema")
                                    .min_size(Vec2::new(132.0, 40.0)),
                            )
                            .clicked()
                        {
                            self.show_integration = true;
                        }
                        if ui
                            .add_enabled(
                                !blocked && !self.rows.is_empty(),
                                egui::Button::new("Svuota lista").min_size(Vec2::new(104.0, 40.0)),
                            )
                            .clicked()
                        {
                            self.rows.clear();
                            self.summary = None;
                        }
                        if !self.messages.is_empty()
                            && ui
                                .button(
                                    RichText::new(format!("Messaggi ({})", self.messages.len()))
                                        .color(theme::WARNING),
                                )
                                .clicked()
                        {
                            self.show_messages = true;
                        }
                    });
                });
            });
        egui::TopBottomPanel::bottom("status").frame(theme::panel_frame()).show(ctx, |ui| {
            if self.work_rx.is_some() {
                ui.horizontal(|ui| {
                    let progress = self.completed as f32 / self.rows.len().max(1) as f32;
                    ui.add(egui::ProgressBar::new(progress).fill(theme::ACCENT).text(format!("{} / {} immagini", self.completed, self.rows.len())).desired_width(ui.available_width() - 270.0));
                    if ui.add_enabled(!self.cancel.load(Ordering::Relaxed), egui::Button::new("Interrompi dopo questo file")).clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let replaced = self.rows.iter().any(|row| row.replaced);
                    let valid = self.settings.validate().is_ok();
                    if self.settings.has_operation() {
                        if ui.add_enabled(!blocked && !self.rows.is_empty() && !replaced && valid,
                            theme::primary(self.settings.action_label())).clicked() {
                            if self.settings.destination == Destination::Replace { self.confirm_replace = true; }
                            else { self.start_compression(); }
                        }
                    } else {
                        ui.label("Attiva un'operazione nelle impostazioni.");
                    }
                    if self.import_rx.is_some() { ui.spinner(); ui.label("Importazione e anteprime…"); }
                    else if self.dialog_rx.is_some() { ui.label("Selezione file…"); }
                    else if replaced { ui.label("Originali sostituiti: svuota e ricarica la lista per un altro ciclo."); }
                    else {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("{} immagini • {}", self.rows.len(), format_bytes(self.rows.iter().map(|row| row.source.bytes).sum()))).strong());
                            ui.label(RichText::new(format!("Output {} • {}", self.settings.output_label(), if self.settings.resize { "Ridimensionamento attivo" } else { "Dimensioni originali" })).small().color(theme::MUTED));
                        });
                    }
                });
            }
            if let Some(summary) = &self.summary {
                ui.add_space(theme::GAP);
                ui.columns(3, |columns| {
                    for (column, (label, value, color)) in columns.iter_mut().zip([
                        (if summary.cancelled { "INTERROTTO" } else { "COMPLETATO" }, format!("{} immagini · {} errori", summary.succeeded, summary.failed), theme::TEXT),
                        ("PRIMA → DOPO", format!("{} → {}", format_bytes(summary.input), format_bytes(summary.output)), theme::BLUE),
                        ("RISPARMIO TOTALE", format!("{} · {:.1}%", format_savings(summary.input as i128 - summary.output as i128), savings_percent(summary.input, summary.output)),
                            if summary.output <= summary.input { theme::ACCENT } else { theme::WARNING }),
                    ]) {
                        egui::Frame::new().fill(theme::SURFACE).corner_radius(8).inner_margin(12).show(column, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(label).small().color(theme::MUTED));
                            ui.label(RichText::new(value).size(18.0).strong().color(color));
                        });
                    }
                });
                ui.label(RichText::new(if self.run_destination != Destination::Replace {
                    "Risparmio rispetto agli originali: le copie non liberano spazio finché gli originali restano sul disco."
                } else if summary.retained > 0 {
                    "Alcuni originali sono stati conservati: verifica gli avvisi prima di eliminare file."
                } else {
                    "Gli originali elaborati correttamente sono stati sostituiti. Valori negativi indicano un aumento di spazio."
                }).small().weak());
            }
        });
        let before = self.settings.clone();
        egui::SidePanel::right("settings")
            .exact_width(340.0)
            .resizable(false)
            .frame(theme::panel_frame())
            .show(ctx, |ui| {
                ui.add_enabled_ui(!blocked, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.settings_panel(ui));
                });
            });
        if before != self.settings {
            self.save_due = Some(Instant::now() + Duration::from_millis(400));
            ctx.request_repaint_after(Duration::from_millis(400));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BACKGROUND).inner_margin(theme::PADDING)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Le tue immagini");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{} file", self.rows.len())).small().color(theme::BLUE));
                });
            });
            ui.label(RichText::new("Trascina file o cartelle. Passa su una miniatura per ingrandirla.").small().color(theme::MUTED));
            ui.add_space(theme::GAP);
            if self.rows.is_empty() {
                let height = ui.available_height();
                egui::Frame::new().fill(theme::PANEL).stroke(egui::Stroke::new(1.0_f32, theme::BORDER))
                    .corner_radius(16).inner_margin(24).show(ui, |ui| {
                    ui.set_min_size(Vec2::new(ui.available_width(), (height - 48.0).max(200.0)));
                    ui.vertical_centered(|ui| {
                        ui.add_space((height * 0.2).max(16.0));
                        ui.horizontal(|ui| {
                            ui.add_space((ui.available_width() - 64.0) / 2.0);
                            theme::logo(ui, &self.logo, 64.0);
                        });
                        ui.add_space(16.0);
                            ui.label(RichText::new("Trascina le tue immagini qui").size(26.0).strong());
                        ui.label(RichText::new("Trascina qui immagini o intere cartelle").color(theme::MUTED));
                        ui.add_space(theme::GAP);
                        if ui.add_enabled(!blocked, theme::primary("Scegli immagini")).clicked() { self.choose(false); }
                        ui.add_space(16.0);
                        ui.label(RichText::new("JPEG · PNG · WebP · BMP · GIF · TIFF").small().color(theme::BLUE));
                        ui.label(RichText::new("Converti anche in AVIF. Gli originali restano al sicuro nelle modalità copia.").small().color(theme::MUTED));
                    });
                });
            } else {
                self.table(ui);
            }
        });
        self.popups(ctx);
        if ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new("drop_overlay"),
            ));
            let rect = ctx.screen_rect();
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(210));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                if blocked {
                    "Attendi il completamento dell'operazione"
                } else {
                    "Rilascia per aggiungere immagini o cartelle"
                },
                egui::FontId::proportional(26.0),
                Color32::WHITE,
            );
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cancel.store(true, Ordering::Relaxed);
        if self.save_due.is_some() {
            let _ = self.settings.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App;

    #[test]
    fn interface_renders_at_minimum_and_default_sizes() {
        for (size, format) in [Vec2::new(1040.0, 680.0), Vec2::new(1280.0, 860.0)]
            .into_iter()
            .flat_map(|size| {
                [
                    None,
                    Some(OutputFormat::Jpeg),
                    Some(OutputFormat::Gif),
                    Some(OutputFormat::Avif),
                ]
                .map(|format| (size, format))
            })
        {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = CompressorApp::new(&cc, Vec::new());
            app.settings = Settings::default();
            if let Some(format) = format {
                app.settings.convert_format = true;
                app.settings.format = format;
            }
            let mut frame = eframe::Frame::_new_kittest();
            for _ in 0..3 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| app.update(ctx, &mut frame),
                );
                assert!(!output.shapes.is_empty());
                // I comandi principali devono rimanere dentro la finestra.
                for shape in output.shapes {
                    if let egui::Shape::Text(text) = shape.shape {
                        if [
                            "+ Immagini",
                            "+ Cartella",
                            "Menu di sistema",
                            "Comprimi immagini",
                            "Comprimi e converti",
                            "Converti immagini",
                            "Ridimensiona immagini",
                            "Comprimi",
                            "Converti formato",
                        ]
                        .contains(&text.galley.job.text.as_str())
                        {
                            assert!(
                                text.pos.x >= 0.0 && text.pos.x + text.galley.size().x <= size.x
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn inactive_operation_controls_are_hidden() {
        for compression in [false, true] {
            for convert_format in [false, true] {
                for resize in [false, true] {
                    let ctx = egui::Context::default();
                    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                    let mut app = CompressorApp::new(&cc, Vec::new());
                    app.settings = Settings {
                        compression,
                        convert_format,
                        resize,
                        format: OutputFormat::Avif,
                        ..Default::default()
                    };
                    for _ in 0..3 {
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    Vec2::new(360.0, 2000.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    app.settings_panel(ui);
                                });
                            },
                        );
                        let labels: Vec<_> = output
                            .shapes
                            .into_iter()
                            .filter_map(|shape| {
                                if let egui::Shape::Text(text) = shape.shape {
                                    Some(text.galley.job.text.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        assert_eq!(labels.iter().any(|label| label == "AVIF"), convert_format);
                        assert_eq!(labels.iter().any(|label| label == "Qualità"), compression);
                        assert_eq!(labels.iter().any(|label| label == "Lato lungo"), resize);
                        assert!(labels.iter().any(|label| label == "Comprimi"));
                        assert!(labels.iter().any(|label| label == "Converti formato"));
                    }
                }
            }
        }
    }

    #[test]
    fn startup_folder_shows_recursion_choice_without_compressing() {
        let dir = tempfile::tempdir().unwrap();
        let cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        let app = CompressorApp::new(&cc, vec![dir.path().to_owned()]);
        assert_eq!(app.pending_folders.len(), 1);
        assert!(app.work_rx.is_none());
        assert!(app.import_rx.is_none());
    }
}
