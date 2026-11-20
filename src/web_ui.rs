//! Finestra «Recupera da pagina web»: URL, analisi, griglia di anteprime
//! ordinata per megapixel con selezione tramite caselle e trascinamento a
//! rettangolo (stile Finder), download con progresso e consegna dei file
//! alla pipeline di importazione esistente.

use crate::{
    model::format_bytes,
    settings::Settings,
    theme,
    web::{self, DownloadEvent, ScanEvent, ScanOptions, WebImage},
};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, TextureHandle, Vec2};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
};

/// Cella della griglia: dati della verifica, anteprima (in attesa di upload
/// sulla GPU o già caricata) e stato di selezione.
struct GridItem {
    image: WebImage,
    texture: Option<TextureHandle>,
    pending: Option<image::RgbaImage>,
    selected: bool,
}

impl GridItem {
    fn pixels(&self) -> f64 {
        self.image.dimensions[0] as f64 * self.image.dimensions[1] as f64
    }

    fn caption(&self) -> String {
        format!(
            "{}×{} · {}",
            self.image.dimensions[0],
            self.image.dimensions[1],
            format_bytes(self.image.bytes)
        )
    }
}

pub struct WebImport {
    pub open: bool,
    url: String,
    crawl_site: bool,
    scan_rx: Option<Receiver<ScanEvent>>,
    cancel_scan: Arc<AtomicBool>,
    items: Vec<GridItem>,
    status: String,
    errors: Vec<String>,
    download_rx: Option<Receiver<DownloadEvent>>,
    cancel_download: Arc<AtomicBool>,
    download_total: usize,
    downloaded_count: usize,
    downloaded: Vec<PathBuf>,
    /// Origine del rettangolo di selezione durante il trascinamento.
    drag_origin: Option<Pos2>,
    drag_remove: bool,
    folder_rx: Option<Receiver<Option<PathBuf>>>,
}

impl Default for WebImport {
    fn default() -> Self {
        Self {
            open: false,
            url: String::new(),
            crawl_site: false,
            scan_rx: None,
            cancel_scan: Arc::new(AtomicBool::new(false)),
            items: Vec::new(),
            status: String::new(),
            errors: Vec::new(),
            download_rx: None,
            cancel_download: Arc::new(AtomicBool::new(false)),
            download_total: 0,
            downloaded_count: 0,
            downloaded: Vec::new(),
            drag_origin: None,
            drag_remove: false,
            folder_rx: None,
        }
    }
}

impl WebImport {
    /// Un'operazione web è in corso: la finestra principale resta bloccata.
    pub fn active(&self) -> bool {
        self.scan_rx.is_some() || self.download_rx.is_some()
    }

    /// Scarica gli eventi dei worker. Restituisce i percorsi dei file
    /// scaricati quando il download termina (anche annullato o vuoto).
    pub fn poll(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Option<Vec<PathBuf>> {
        loop {
            let event = self.scan_rx.as_ref().map(|rx| rx.try_recv());
            match event {
                Some(Ok(ScanEvent::Status(text))) => self.status = text,
                Some(Ok(ScanEvent::Image(image))) => self.insert_sorted(*image),
                Some(Ok(ScanEvent::Error(text))) => self.push_error(text),
                Some(Ok(ScanEvent::Finished)) => {
                    self.scan_rx = None;
                    self.finish_scan_status();
                    break;
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.scan_rx = None;
                    self.push_error("Analisi interrotta inaspettatamente.".into());
                    break;
                }
            }
        }
        loop {
            let event = self.download_rx.as_ref().map(|rx| rx.try_recv());
            match event {
                Some(Ok(DownloadEvent::Saved(path))) => {
                    self.downloaded.push(path);
                    self.downloaded_count += 1;
                }
                Some(Ok(DownloadEvent::Error(text))) => {
                    self.push_error(text);
                    self.downloaded_count += 1;
                }
                Some(Ok(DownloadEvent::Finished { cancelled })) => {
                    self.download_rx = None;
                    if cancelled {
                        self.push_error(
                            "Download interrotto: importo solo i file completati.".into(),
                        );
                    }
                    let paths = std::mem::take(&mut self.downloaded);
                    self.download_total = 0;
                    self.downloaded_count = 0;
                    self.open = false;
                    return Some(paths);
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.download_rx = None;
                    self.push_error("Download interrotta inaspettatamente.".into());
                    let paths = std::mem::take(&mut self.downloaded);
                    self.download_total = 0;
                    self.downloaded_count = 0;
                    self.open = false;
                    return Some(paths);
                }
            }
        }
        if let Some(rx) = &self.folder_rx {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.folder_rx = None;
                    settings.web_download_dir = Some(path);
                }
                Ok(None) => self.folder_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.folder_rx = None,
            }
        }
        // Un numero limitato di upload per frame evita di bloccare il rendering.
        let mut uploaded = 0;
        for item in &mut self.items {
            if uploaded >= 12 {
                break;
            }
            if let Some(preview) = item.pending.take() {
                let size = [preview.width() as usize, preview.height() as usize];
                item.texture = Some(ctx.load_texture(
                    format!("web-{}", item.image.url),
                    egui::ColorImage::from_rgba_unmultiplied(size, preview.as_raw()),
                    egui::TextureOptions::LINEAR,
                ));
                uploaded += 1;
            }
        }
        None
    }

    fn finish_scan_status(&mut self) {
        if self.items.is_empty() {
            self.status = "Nessuna immagine utilizzabile trovata.".into();
        } else {
            self.status = format!("{} immagini pronte: seleziona e importa.", self.items.len());
        }
    }

    fn insert_sorted(&mut self, mut image: WebImage) {
        let pending = image.preview.take();
        let item = GridItem {
            image,
            texture: None,
            pending,
            selected: false,
        };
        let position = self
            .items
            .partition_point(|existing| existing.pixels() >= item.pixels());
        self.items.insert(position, item);
    }

    fn push_error(&mut self, text: String) {
        if self.errors.len() >= 200 {
            self.errors.remove(0);
        }
        self.errors.push(text);
    }

    fn start_scan(&mut self, settings: &Settings) {
        let trimmed = self.url.trim();
        let parsed = url::Url::parse(trimmed)
            .ok()
            .or_else(|| url::Url::parse(&format!("https://{trimmed}")).ok());
        let Some(start) = parsed else {
            self.status = "Inserisci un URL valido, ad esempio https://esempio.it/galleria".into();
            return;
        };
        if !matches!(start.scheme(), "http" | "https") {
            self.status = "Sono supportati solo gli indirizzi http e https.".into();
            return;
        }
        self.cancel_scan = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(16);
        self.scan_rx = Some(rx);
        self.items.clear();
        self.errors.clear();
        self.status = String::new();
        let options = ScanOptions {
            crawl_site: self.crawl_site,
            max_pages: settings.web_max_pages,
            min_kb: settings.web_min_kb,
        };
        let cancel = Arc::clone(&self.cancel_scan);
        std::thread::spawn(move || web::scan(&start, &options, &cancel, tx));
    }

    fn start_download(&mut self, settings: &Settings) {
        let selected: Vec<WebImage> = self
            .items
            .iter()
            .filter(|item| item.selected)
            .map(|item| item.image.clone())
            .collect();
        if selected.is_empty() {
            return;
        }
        let dir = download_dir(settings);
        self.download_total = selected.len();
        self.downloaded_count = 0;
        self.downloaded.clear();
        self.cancel_download = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(64);
        self.download_rx = Some(rx);
        let cancel = Arc::clone(&self.cancel_download);
        std::thread::spawn(move || web::download(selected, &dir, &cancel, tx));
    }

    fn choose_folder(&mut self) {
        let (tx, rx) = mpsc::channel();
        self.folder_rx = Some(rx);
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Cartella di destinazione")
                .pick_folder();
            let _ = tx.send(picked);
        });
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) {
        if !self.open {
            return;
        }
        let downloading = self.download_rx.is_some();
        egui::Modal::new(egui::Id::new("web_import")).show(ctx, |ui| {
            ui.set_min_width(780.0);
            ui.heading(if downloading {
                "Download in corso"
            } else {
                "Recupera da pagina web"
            });
            ui.label(
                egui::RichText::new(
                    "Analizza una pagina (o un intero sito), scegli le foto e importale.",
                )
                .small()
                .color(theme::MUTED),
            );
            ui.add_space(theme::GAP);
            if downloading {
                self.download_body(ui);
            } else {
                self.search_body(ui, settings);
            }
        });
    }

    fn search_body(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        ui.horizontal(|ui| {
            ui.label("Pagina");
            let mut field = egui::TextEdit::singleline(&mut self.url)
                .hint_text("https://esempio.it/galleria")
                .desired_width(ui.available_width() - 130.0);
            if self.scan_rx.is_some() {
                field = field.interactive(false);
            }
            ui.add(field);
            let scanning = self.scan_rx.is_some();
            if ui
                .add_enabled(
                    !scanning,
                    egui::Button::new("Analizza").min_size(Vec2::new(110.0, 34.0)),
                )
                .clicked()
            {
                self.start_scan(settings);
            }
        });
        ui.horizontal(|ui| {
            for (mode, label) in [(false, "Solo questa pagina"), (true, "Intero sito")] {
                if ui
                    .add(egui::Button::new(label).selected(self.crawl_site == mode))
                    .clicked()
                {
                    self.crawl_site = mode;
                }
            }
            if self.crawl_site {
                ui.label("Pagine max");
                ui.add(
                    egui::DragValue::new(&mut settings.web_max_pages)
                        .range(1..=100)
                        .speed(1),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.web_min_kb)
                        .range(0..=100_000)
                        .speed(1)
                        .suffix(" KiB"),
                );
                ui.label("Salta immagini sotto");
            });
        });
        ui.horizontal(|ui| {
            ui.label("Salva in");
            let dir = download_dir(settings);
            ui.monospace(
                egui::RichText::new(
                    dir.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| dir.display().to_string()),
                )
                .small()
                .color(theme::BLUE),
            )
            .on_hover_text(dir.display().to_string());
            if self.folder_rx.is_some() {
                ui.spinner();
            } else if ui.button("Sfoglia…").clicked() {
                self.choose_folder();
            }
        });
        ui.add_space(theme::GAP);

        if self.scan_rx.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&self.status);
                if ui.button("Annulla").clicked() {
                    self.cancel_scan.store(true, Ordering::Relaxed);
                }
            });
        } else if !self.status.is_empty() {
            ui.label(
                egui::RichText::new(&self.status)
                    .small()
                    .color(theme::MUTED),
            );
        }
        if let Some(last) = self.errors.last() {
            ui.colored_label(
                theme::DANGER,
                egui::RichText::new(format!("Ultimo problema: {last}")).small(),
            )
            .on_hover_text(self.errors.join("\n"));
        }

        if !self.items.is_empty() {
            ui.add_space(theme::GAP);
            ui.horizontal(|ui| {
                if ui.button("Seleziona tutte").clicked() {
                    for item in &mut self.items {
                        item.selected = true;
                    }
                }
                if ui.button("Deseleziona tutte").clicked() {
                    for item in &mut self.items {
                        item.selected = false;
                    }
                }
                let selected: Vec<&GridItem> =
                    self.items.iter().filter(|item| item.selected).collect();
                ui.label(
                    egui::RichText::new(format!(
                        "{} di {} selezionate · {}",
                        selected.len(),
                        self.items.len(),
                        format_bytes(selected.iter().map(|item| item.image.bytes).sum())
                    ))
                    .small()
                    .color(theme::BLUE),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Trascina sulle miniature per selezionare (con Alt per deselezionare).",
                )
                .small()
                .color(theme::MUTED),
            );
            self.grid(ui);
        }

        ui.add_space(theme::GAP);
        ui.horizontal(|ui| {
            let selected = self.items.iter().filter(|item| item.selected).count();
            let label = if selected == 1 {
                "Importa 1 immagine".to_string()
            } else {
                format!("Importa {selected} immagini")
            };
            let ready = selected > 0 && self.scan_rx.is_none();
            if ui.add_enabled(ready, theme::primary(&label)).clicked() {
                self.start_download(settings);
            }
            if ui.button("Chiudi").clicked() {
                // Chiudere durante l'analisi la interrompe, altrimenti la
                // finestra principale resterebbe bloccata senza motivo.
                if self.scan_rx.is_some() {
                    self.cancel_scan.store(true, Ordering::Relaxed);
                }
                self.open = false;
            }
        });
    }

    /// Griglia di anteprime: celle a dimensione fissa disposte per righe
    /// dentro un'area scorrevole, con rettangolo di selezione durante
    /// il trascinamento.
    fn grid(&mut self, ui: &mut egui::Ui) {
        const CELL: Vec2 = Vec2::new(168.0, 152.0);
        const PREVIEW: Vec2 = Vec2::new(150.0, 96.0);
        let mut rects: Vec<(usize, Rect)> = Vec::new();
        let mut toggled: Option<usize> = None;
        let mut drag_start: Option<Pos2> = None;

        egui::ScrollArea::vertical()
            .max_height(360.0)
            .drag_to_scroll(false)
            .show(ui, |ui| {
                let columns =
                    (((ui.available_width() + theme::GAP) / (CELL.x + theme::GAP)).floor()
                        as usize)
                        .max(1);
                for (row, chunk) in self.items.chunks_mut(columns).enumerate() {
                    ui.horizontal(|ui| {
                        for (column, item) in chunk.iter_mut().enumerate() {
                            let index = row * columns + column;
                            let (rect, response) =
                                ui.allocate_exact_size(CELL, Sense::click_and_drag());
                            let fill = if item.selected {
                                Color32::from_rgb(37, 79, 79)
                            } else {
                                theme::SURFACE
                            };
                            let stroke = if item.selected {
                                egui::Stroke::new(2.0_f32, theme::ACCENT)
                            } else {
                                egui::Stroke::new(1.0_f32, theme::BORDER)
                            };
                            ui.painter().rect_filled(rect, 10.0, fill);
                            ui.painter()
                                .rect_stroke(rect, 10.0, stroke, egui::StrokeKind::Inside);
                            ui.allocate_new_ui(
                                egui::UiBuilder::new().max_rect(rect.shrink(4.0)),
                                |ui| {
                                    ui.set_min_size(rect.size() - Vec2::splat(8.0));
                                    ui.vertical_centered(|ui| {
                                        let [width, height] = item.image.dimensions;
                                        if let Some(texture) = &item.texture {
                                            let (preview_rect, _) =
                                                ui.allocate_exact_size(PREVIEW, Sense::hover());
                                            ui.put(
                                                preview_rect,
                                                egui::Image::new(texture).max_size(PREVIEW),
                                            )
                                            .on_hover_ui(|ui| {
                                                ui.set_min_width(360.0);
                                                ui.add(
                                                    egui::Image::new(texture)
                                                        .max_size(Vec2::new(480.0, 360.0)),
                                                );
                                                ui.label(item.image.url.as_str());
                                            });
                                        } else if item.pending.is_some() {
                                            let (busy, _) =
                                                ui.allocate_exact_size(PREVIEW, Sense::hover());
                                            ui.put(busy, egui::Spinner::new());
                                        } else {
                                            let (empty, _) =
                                                ui.allocate_exact_size(PREVIEW, Sense::hover());
                                            ui.painter().rect_filled(empty, 8.0, theme::PANEL);
                                            ui.painter().text(
                                                empty.center(),
                                                egui::Align2::CENTER_CENTER,
                                                format!("{width}×{height}"),
                                                egui::FontId::proportional(15.0),
                                                theme::MUTED,
                                            );
                                        }
                                        ui.horizontal(|ui| {
                                            ui.checkbox(&mut item.selected, "");
                                            ui.label(
                                                egui::RichText::new(item.caption())
                                                    .small()
                                                    .color(theme::MUTED),
                                            );
                                        });
                                    });
                                },
                            );
                            if response.clicked() {
                                toggled = Some(index);
                            }
                            if response.drag_started() {
                                drag_start = response.interact_pointer_pos();
                            }
                            rects.push((index, rect));
                        }
                    });
                }
            });

        if let Some(origin) = drag_start {
            self.drag_origin = Some(origin);
            self.drag_remove = ui.input(|input| input.modifiers.alt);
        }
        if let Some(origin) = self.drag_origin {
            let current = ui.input(|input| input.pointer.interact_pos());
            if let Some(current) = current {
                let band = Rect::from_two_pos(origin, current);
                ui.painter()
                    .rect_filled(band, 3.0, theme::ACCENT.gamma_multiply(0.15));
                ui.painter().rect_stroke(
                    band,
                    3.0,
                    egui::Stroke::new(1.5_f32, theme::ACCENT),
                    egui::StrokeKind::Inside,
                );
                for (index, rect) in &rects {
                    if rect.intersects(band) {
                        if let Some(item) = self.items.get_mut(*index) {
                            item.selected = !self.drag_remove;
                        }
                    }
                }
            }
            if !ui.input(|input| input.pointer.primary_down()) {
                self.drag_origin = None;
            }
        }
        if let Some(index) = toggled {
            if let Some(item) = self.items.get_mut(index) {
                item.selected = !item.selected;
            }
        }
    }

    fn download_body(&mut self, ui: &mut egui::Ui) {
        ui.add_space(theme::GAP);
        let progress = self.downloaded_count as f32 / self.download_total.max(1) as f32;
        ui.add(
            egui::ProgressBar::new(progress)
                .fill(theme::ACCENT)
                .text(format!(
                    "{} / {} file",
                    self.downloaded_count, self.download_total
                ))
                .desired_width(ui.available_width()),
        );
        if let Some(last) = self.errors.last() {
            ui.colored_label(
                theme::DANGER,
                egui::RichText::new(format!("Ultimo problema: {last}")).small(),
            );
        }
        ui.label(
            egui::RichText::new(format!(
                "I file vengono salvati in {} e poi importati nella lista.",
                self.downloaded
                    .last()
                    .and_then(|path| path.parent())
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_default()
            ))
            .small()
            .color(theme::MUTED),
        );
        ui.add_space(theme::GAP);
        if ui
            .add_enabled(
                !self.cancel_download.load(Ordering::Relaxed),
                egui::Button::new("Annulla e importa i completati"),
            )
            .clicked()
        {
            self.cancel_download.store(true, Ordering::Relaxed);
        }
    }

    /// Inietta un risultato di verifica, solo per i test della UI.
    #[cfg(test)]
    pub fn push_item_for_tests(&mut self, image: WebImage) {
        self.insert_sorted(image);
    }
}

fn download_dir(settings: &Settings) -> PathBuf {
    settings
        .web_download_dir
        .clone()
        .unwrap_or_else(web::default_download_dir)
}
