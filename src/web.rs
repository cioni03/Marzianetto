//! Recupero di immagini da pagine web: estrazione degli URL da tutte le
//! posizioni dell'HTML, riconoscimento delle varianti in alta risoluzione
//! dei CDN, verifica con richieste parziali, download concorrente e, in
//! modalità sito, crawler BFS che rispetta robots.txt.

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use regex::Regex;
use scraper::{Html, Selector};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Condvar, LazyLock, Mutex,
    },
    time::{Duration, Instant},
};
use url::Url;

/// User-Agent di browser puro, senza suffissi: alcuni WAF rispondono 403
/// proprio agli identificativi di applicazione.
pub const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36";
const ACCEPT_LANGUAGE: &str = "it-IT,it;q=0.9,en;q=0.8";
const ACCEPT_HTML: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const ACCEPT_IMAGE: &str = "image/avif,image/webp,image/png,image/svg+xml,image/*,*/*;q=0.8";
/// Primi 256 KiB di ogni immagine: bastano per decodificare l'intestazione
/// e spesso contengono l'intera foto.
const PARTIAL_BYTES: u64 = 256 * 1024;
/// Worker paralleli per verifica, download e pagine del crawler.
const WORKERS: usize = 4;
/// Limite di sicurezza per il corpo di una pagina HTML.
const PAGE_LIMIT: u64 = 5 * 1024 * 1024;

/// Immagine individuata in una pagina e verificata con una richiesta parziale.
#[derive(Clone)]
pub struct WebImage {
    /// URL che ha risposto alla verifica: è quello da scaricare.
    pub url: Url,
    /// Variante non usata (miniatura o originale): tentata se il download fallisce.
    pub alternate: Option<Url>,
    /// Pagina di provenienza, usata come Referer.
    pub page: Url,
    pub bytes: u64,
    pub dimensions: [u32; 2],
    pub format: String,
    /// Miniatura decodificata dai primi byte, quando bastano.
    pub preview: Option<RgbaImage>,
}

pub enum ScanEvent {
    Status(String),
    Image(Box<WebImage>),
    Finished,
    Error(String),
}

pub enum DownloadEvent {
    Saved(PathBuf),
    Error(String),
    Finished { cancelled: bool },
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub crawl_site: bool,
    pub max_pages: u32,
    pub min_kb: u64,
}

/// Cartella di destinazione predefinita per i download dal web.
pub fn default_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("marzianetto-web")
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        // I codici 4xx/5xx servono a decidere il ripiego sulla miniatura.
        .http_status_as_error(false)
        .max_redirects(5)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::new_with_config(config)
}

fn request(
    agent: &ureq::Agent,
    url: &Url,
    accept: &str,
    referer: Option<&Url>,
    range: Option<String>,
) -> Result<ureq::http::Response<ureq::Body>> {
    let mut request = agent
        .get(url.as_str())
        .header("Accept", accept)
        .header("Accept-Language", ACCEPT_LANGUAGE);
    if let Some(referer) = referer {
        request = request.header("Referer", referer.as_str());
    }
    if let Some(range) = range {
        request = request.header("Range", range);
    }
    request.call().with_context(|| format!("Richiesta verso {url}"))
}

fn read_body(response: &mut ureq::http::Response<ureq::Body>, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .with_context(|| "Lettura della risposta interrotta")?;
    Ok(bytes)
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
}

/// Scarica una pagina HTML. Il crawler accetta solo risposte text/html.
fn fetch_page(agent: &ureq::Agent, url: &Url) -> Result<String> {
    let mut response = request(agent, url, ACCEPT_HTML, None, None)?;
    let status = response.status().as_u16();
    if status >= 400 {
        bail!("HTTP {status}");
    }
    let content_type = header(&response, "content-type").unwrap_or_default();
    if !content_type.starts_with("text/html") {
        bail!("non è una pagina HTML ({content_type})");
    }
    let bytes = read_body(&mut response, PAGE_LIMIT)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

// ---------------------------------------------------------------------------
// Estrazione
// ---------------------------------------------------------------------------

/// URL immagine trovato in una pagina, con la variante in alta risoluzione
/// da provare per prima e la pagina di provenienza per il Referer.
#[derive(Debug, Clone)]
struct Candidate {
    primary: Url,
    fallback: Option<Url>,
    page: Url,
}

fn push_candidate(
    candidates: &mut Vec<Candidate>,
    seen: &mut HashSet<String>,
    raw: &str,
    page: &Url,
) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    // Risolve i percorsi relativi e scarta data:, blob: e schemi non http.
    let Ok(absolute) = page.join(raw) else {
        return;
    };
    if !matches!(absolute.scheme(), "http" | "https") {
        return;
    }
    let full = full_size(&absolute);
    if !seen.insert(full.to_string()) {
        return;
    }
    if full == absolute {
        candidates.push(Candidate {
            primary: absolute,
            fallback: None,
            page: page.clone(),
        });
    } else {
        candidates.push(Candidate {
            primary: full,
            fallback: Some(absolute),
            page: page.clone(),
        });
    }
}

/// Raccoglie gli URL delle immagini da tutte le posizioni in cui possono
/// nascondersi: <img> con gli attributi di lazy-loading, srcset/data-srcset
/// su <img> e <picture><source>, i <a> che puntano a file immagine, i
/// background-image CSS (attributi style e blocchi <style>) e i meta
/// og:image/twitter:image.
fn collect_from_html(
    html: &str,
    page: &Url,
    candidates: &mut Vec<Candidate>,
    seen: &mut HashSet<String>,
) {
    let document = Html::parse_document(html);
    let img = Selector::parse("img").unwrap();
    let source = Selector::parse("source").unwrap();
    let anchor = Selector::parse("a[href]").unwrap();
    let styled = Selector::parse("[style]").unwrap();
    let style_block = Selector::parse("style").unwrap();
    let meta = Selector::parse("meta").unwrap();

    for element in document.select(&img) {
        for attribute in [
            "src",
            "data-src",
            "data-original",
            "data-lazy-src",
            "data-lazy",
            "data-echo",
        ] {
            if let Some(value) = element.value().attr(attribute) {
                push_candidate(candidates, seen, value, page);
            }
        }
        for attribute in ["srcset", "data-srcset"] {
            if let Some(url) = element.value().attr(attribute).and_then(best_srcset) {
                push_candidate(candidates, seen, &url, page);
            }
        }
    }
    for element in document.select(&source) {
        for attribute in ["srcset", "data-srcset"] {
            if let Some(url) = element.value().attr(attribute).and_then(best_srcset) {
                push_candidate(candidates, seen, &url, page);
            }
        }
    }
    for element in document.select(&anchor) {
        if let Some(href) = element.value().attr("href") {
            if looks_like_image(href) {
                push_candidate(candidates, seen, href, page);
            }
        }
    }
    for element in document.select(&styled) {
        if let Some(style) = element.value().attr("style") {
            for url in css_urls(style) {
                push_candidate(candidates, seen, &url, page);
            }
        }
    }
    for element in document.select(&style_block) {
        let css = element.text().collect::<String>();
        for url in css_urls(&css) {
            push_candidate(candidates, seen, &url, page);
        }
    }
    for element in document.select(&meta) {
        let value = element.value();
        let name = value
            .attr("property")
            .or_else(|| value.attr("name"))
            .unwrap_or_default();
        if matches!(
            name,
            "og:image" | "og:image:secure_url" | "twitter:image" | "twitter:image:src"
        ) {
            if let Some(content) = value.attr("content") {
                push_candidate(candidates, seen, content, page);
            }
        }
    }
}

/// Sceglie sempre il candidato col descriptor più alto: prima la larghezza
/// in pixel ("foto.jpg 1920w"), poi la densità ("foto.jpg 2x").
pub fn best_srcset(srcset: &str) -> Option<String> {
    let mut best: Option<(f64, bool, &str)> = None;
    for candidate in srcset.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else {
            continue;
        };
        let descriptor = parts.next().unwrap_or("1x");
        let (value, is_width) = if let Some(width) = descriptor.strip_suffix('w') {
            match width.parse::<f64>() {
                Ok(value) => (value, true),
                Err(_) => continue,
            }
        } else if let Some(density) = descriptor.strip_suffix('x') {
            match density.parse::<f64>() {
                Ok(value) => (value, false),
                Err(_) => continue,
            }
        } else {
            continue;
        };
        // Nelle liste miste i descriptor "w" battono gli "x".
        let better = match best {
            None => true,
            Some((_, best_width, _)) if best_width != is_width => is_width,
            Some((best_value, _, _)) => value > best_value,
        };
        if better {
            best = Some((value, is_width, url));
        }
    }
    best.map(|(_, _, url)| url.to_string())
}

/// Estrae gli URL dai riferimenti url(...) di un frammento CSS. Le virgolette
/// sono opzionali: il crate `regex` non supporta i backreference per
/// verificarne la coppia.
fn css_urls(css: &str) -> Vec<String> {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"url\(\s*['"]?([^'")]+)['"]?\s*\)"#).unwrap());
    URL.captures_iter(css)
        .map(|captures| captures[1].to_string())
        .collect()
}

/// Riconosce un collegamento diretto a un file immagine dall'estensione del
/// percorso (ignora query e frammento), con la stessa lista dell'import.
pub fn looks_like_image(href: &str) -> bool {
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let file = path.rsplit('/').next().unwrap_or(path);
    crate::importer::supported(Path::new(file))
}

// ---------------------------------------------------------------------------
// Alta risoluzione
// ---------------------------------------------------------------------------

/// Ricostruisce l'URL dell'originale da una miniatura del CDN: rimuove i
/// suffissi con le dimensioni nel nome file (WordPress "-300x200.jpg" e
/// "-scaled", Shopify "_1024x1024…") e i parametri di trasformazione
/// della query (?w=&h=&fit=&resize=&quality=). Restituisce None quando
/// l'URL è già in forma piena.
pub fn upgraded(url: &Url) -> Option<Url> {
    static SCALED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-scaled$").unwrap());
    static SIZED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[-_]\d+x\d+.*$").unwrap());
    const SIZE_PARAMS: [&str; 9] = [
        "w", "h", "width", "height", "fit", "resize", "quality", "q", "s",
    ];

    let mut result = url.clone();
    let mut changed = false;

    let path = result.path().to_string();
    if let Some((directory, file)) = path.rsplit_once('/') {
        if let Some((stem, extension)) = file.rsplit_once('.') {
            let mut stem = stem.to_string();
            for _ in 0..2 {
                for pattern in [&*SCALED, &*SIZED] {
                    let stripped = pattern.replace_all(&stem, "").to_string();
                    if stripped != stem && !stripped.is_empty() {
                        stem = stripped;
                        changed = true;
                    }
                }
            }
            if changed {
                let file = format!("{stem}.{extension}");
                let joined = if directory.is_empty() {
                    format!("/{file}")
                } else {
                    format!("{directory}/{file}")
                };
                result.set_path(&joined);
            }
        }
    }

    if let Some(query) = url.query() {
        let mut kept: Vec<&str> = Vec::new();
        let mut dropped = false;
        for pair in query.split('&') {
            let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
            if SIZE_PARAMS.contains(&key.as_str()) {
                dropped = true;
            } else {
                kept.push(pair);
            }
        }
        if dropped {
            if kept.is_empty() {
                result.set_query(None);
            } else {
                result.set_query(Some(&kept.join("&")));
            }
            changed = true;
        }
    }
    changed.then_some(result)
}

/// Chiave di deduplicazione: la forma a dimensione piena, così la miniatura
/// e l'originale della stessa foto collassano in una sola voce.
pub fn full_size(url: &Url) -> Url {
    upgraded(url).unwrap_or_else(|| url.clone())
}

// ---------------------------------------------------------------------------
// Verifica con richiesta parziale
// ---------------------------------------------------------------------------

struct Partial {
    status: u16,
    content_type: Option<String>,
    total: Option<u64>,
    bytes: Vec<u8>,
}

/// GET parziale con header Range: alcuni server rifiutano HEAD e i primi
/// byte servono comunque per decodificare l'intestazione dell'immagine.
fn fetch_partial(agent: &ureq::Agent, url: &Url, referer: &Url) -> Result<Partial> {
    let range = format!("bytes=0-{}", PARTIAL_BYTES - 1);
    let mut response = request(agent, url, ACCEPT_IMAGE, Some(referer), Some(range))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Ok(Partial {
            status,
            content_type: None,
            total: None,
            bytes: Vec::new(),
        });
    }
    let content_type = header(&response, "content-type");
    let total = if status == 206 {
        // Content-Range: "bytes 0-N/TOT"
        response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(content_range_total)
    } else {
        response.body().content_length()
    };
    let bytes = read_body(&mut response, PARTIAL_BYTES)?;
    // Senza Content-Length (200 a blocchi): se la lettura è giunta a EOF
    // prima del limite, la dimensione totale è nota comunque.
    let total =
        total.or_else(|| (bytes.len() < PARTIAL_BYTES as usize).then_some(bytes.len() as u64));
    Ok(Partial {
        status,
        content_type,
        total,
        bytes,
    })
}

fn content_range_total(value: &str) -> Option<u64> {
    value.split('/').nth(1)?.trim().parse().ok()
}

/// Decodifica solo l'intestazione dell'immagine per ricavarne formato e
/// risoluzione, con gli stessi limiti dell'import dal disco. Scartati i
/// formati che la pipeline non sa trattare (SVG, AVIF, …).
fn image_header(bytes: &[u8]) -> Option<(String, [u32; 2])> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    if !matches!(
        format,
        image::ImageFormat::Jpeg
            | image::ImageFormat::Png
            | image::ImageFormat::WebP
            | image::ImageFormat::Gif
            | image::ImageFormat::Bmp
            | image::ImageFormat::Tiff
    ) {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(32_768);
    limits.max_image_height = Some(32_768);
    reader.limits(limits);
    let (width, height) = reader.into_dimensions().ok()?;
    Some((format!("{format:?}"), [width, height]))
}

/// Miniatura "best effort" dai primi byte: decodifica solo quando il
/// contenuto è completo (le immagini maggiori di 256 KiB restano senza
/// anteprima e la griglia mostra le dimensioni).
fn decode_preview(bytes: &[u8]) -> Option<RgbaImage> {
    image::load_from_memory(bytes)
        .ok()
        .map(|image| image.thumbnail(480, 360).to_rgba8())
}

/// Impronta digitale per la deduplicazione: due voci con identici
/// larghezza × altezza × byte × formato sono la stessa foto servita da
/// URL diversi.
type Fingerprint = (u32, u32, u64, String);

fn probe_candidate(
    agent: &ureq::Agent,
    candidate: &Candidate,
    min_kb: u64,
    fingerprints: &Mutex<HashSet<Fingerprint>>,
) -> Result<Option<WebImage>> {
    let mut partial = fetch_partial(agent, &candidate.primary, &candidate.page)?;
    let mut used = candidate.primary.clone();
    if partial.status >= 400 {
        let Some(fallback) = &candidate.fallback else {
            return Ok(None);
        };
        partial = fetch_partial(agent, fallback, &candidate.page)?;
        if partial.status >= 400 {
            return Ok(None);
        }
        used = fallback.clone();
    }
    let Some(content_type) = &partial.content_type else {
        return Ok(None);
    };
    if !content_type.starts_with("image/") || content_type.contains("svg") {
        return Ok(None);
    }
    let Some(total) = partial.total else {
        return Ok(None);
    };
    let Some((format, dimensions)) = image_header(&partial.bytes) else {
        return Ok(None);
    };
    if total < min_kb.saturating_mul(1024) {
        return Ok(None);
    }
    {
        let mut seen = fingerprints.lock().unwrap();
        if !seen.insert((dimensions[0], dimensions[1], total, format.clone())) {
            return Ok(None);
        }
    }
    let alternate = if used == candidate.primary {
        candidate.fallback.clone()
    } else {
        Some(candidate.primary.clone())
    };
    Ok(Some(WebImage {
        url: used,
        alternate,
        page: candidate.page.clone(),
        bytes: total,
        dimensions,
        format,
        preview: decode_preview(&partial.bytes),
    }))
}

// ---------------------------------------------------------------------------
// Concorrenza
// ---------------------------------------------------------------------------

/// Piccolo pool di worker: estrae i lavori da una coda protetta da mutex,
/// con una condvar per il termine coordinato e l'annullamento immediato.
fn pool<T, F>(jobs: Vec<T>, cancel: &Arc<AtomicBool>, task: F)
where
    T: Send + 'static,
    F: Fn(T) + Send + Sync + 'static,
{
    let shared = Arc::new((Mutex::new((VecDeque::from(jobs), 0usize)), Condvar::new()));
    let task = Arc::new(task);
    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let shared = Arc::clone(&shared);
        let cancel = Arc::clone(cancel);
        let task = Arc::clone(&task);
        handles.push(std::thread::spawn(move || loop {
            let job = {
                let mut guard = shared.0.lock().unwrap();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        guard.0.clear();
                        drop(guard);
                        shared.1.notify_all();
                        return;
                    }
                    if let Some(job) = guard.0.pop_front() {
                        guard.1 += 1;
                        break job;
                    }
                    if guard.1 == 0 {
                        drop(guard);
                        shared.1.notify_all();
                        return;
                    }
                    guard = shared.1.wait(guard).unwrap();
                }
            };
            task(job);
            shared.0.lock().unwrap().1 -= 1;
            shared.1.notify_all();
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Scarica le immagini selezionate in `dir` con quattro worker paralleli.
/// I nomi file arrivano dall'URL e vengono deduplicati come "nome (2).jpg".
pub fn download(
    images: Vec<WebImage>,
    dir: &Path,
    cancel: &Arc<AtomicBool>,
    tx: SyncSender<DownloadEvent>,
) {
    if let Err(err) = fs::create_dir_all(dir) {
        let _ = tx.send(DownloadEvent::Error(format!(
            "Creazione della cartella {}: {err:#}",
            dir.display()
        )));
        let _ = tx.send(DownloadEvent::Finished { cancelled: false });
        return;
    }
    let agent = Arc::new(agent());
    let dir = dir.to_path_buf();
    let taken = Arc::new(Mutex::new(existing_names(&dir)));
    let worker_tx = tx.clone();
    pool(images, cancel, move |image| {
        match download_one(&agent, &image, &dir, &taken) {
            Ok(path) => {
                let _ = worker_tx.send(DownloadEvent::Saved(path));
            }
            Err(err) => {
                let _ = worker_tx.send(DownloadEvent::Error(format!("{}: {err:#}", image.url)));
            }
        }
    });
    let _ = tx.send(DownloadEvent::Finished {
        cancelled: cancel.load(Ordering::Relaxed),
    });
}

fn download_one(
    agent: &ureq::Agent,
    image: &WebImage,
    dir: &Path,
    taken: &Mutex<HashSet<String>>,
) -> Result<PathBuf> {
    // Prova prima l'URL "upgradato" e, su 4xx/5xx, ricade sull'altro.
    let mut response = request(agent, &image.url, ACCEPT_IMAGE, Some(&image.page), None)?;
    if response.status().as_u16() >= 400 {
        if let Some(alternate) = &image.alternate {
            response = request(agent, alternate, ACCEPT_IMAGE, Some(&image.page), None)?;
        }
    }
    if response.status().as_u16() >= 400 {
        bail!("HTTP {}", response.status().as_u16());
    }
    let content_type = header(&response, "content-type");
    let (stem, extension) = file_parts(&image.url, &image.format, content_type.as_deref());
    // Il nome viene riservato prima del download: i worker paralleli non
    // possono scegliere lo stesso file.
    let _file_name = {
        let mut names = taken.lock().unwrap();
        unique_file_name(&stem, &extension, &mut names)
    };
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("Cartella {} non scrivibile", dir.display()))?;
    std::io::copy(
        &mut response.body_mut().as_reader(),
        temp.as_file_mut(),
    )
    .with_context(|| format!("Download di {} interrotto", image.url))?;
    temp.as_file().sync_all()?;
    persist_with_numbering(temp, dir, &stem, &extension)
}

/// Persistenza senza sovrascrittura, con rinomina "nome (2).jpg".
fn persist_with_numbering(
    mut temp: tempfile::NamedTempFile,
    dir: &Path,
    stem: &str,
    extension: &str,
) -> Result<PathBuf> {
    for index in 0..10_000 {
        let candidate = if index == 0 {
            dir.join(format!("{stem}.{extension}"))
        } else {
            // La numerazione riprende quella delle prenotazioni: "nome (2).jpg".
            dir.join(format!("{stem} ({}).{extension}", index + 1))
        };
        match temp.persist_noclobber(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temp = err.file;
            }
            Err(err) => return Err(err.error.into()),
        }
    }
    bail!("Troppe collisioni di nomi nella cartella di download")
}

/// Nome file unico nella sessione di download: "foto.jpg", "foto (2).jpg", …
fn unique_file_name(stem: &str, extension: &str, taken: &mut HashSet<String>) -> String {
    let base = format!("{stem}.{extension}");
    if taken.insert(base.clone()) {
        return base;
    }
    for index in 2..10_000 {
        let candidate = format!("{stem} ({index}).{extension}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
    "immagine.bin".into()
}

/// Nome file dall'ultimo segmento dell'URL, ripulito dai caratteri
/// problematici; l'estensione arriva dall'URL quando è credibile, altrimenti
/// dal formato decodificato durante la verifica.
fn file_parts(url: &Url, format: &str, content_type: Option<&str>) -> (String, String) {
    let segment = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(segment).decode_utf8_lossy();
    let (stem, extension) = match decoded.rsplit_once('.') {
        Some((stem, extension)) if looks_like_image(&decoded) => {
            (stem.to_string(), extension.to_ascii_lowercase())
        }
        _ => (
            decoded.to_string(),
            extension_for(format, content_type).to_string(),
        ),
    };
    let mut stem: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    stem.truncate(120);
    let mut stem = stem.trim_matches(['.', '-']).to_string();
    if stem.is_empty() {
        stem = format!("immagine-{:x}", url_hash(url));
    }
    (stem, extension)
}

fn extension_for(format: &str, content_type: Option<&str>) -> &'static str {
    match format {
        "Jpeg" => "jpg",
        "Png" => "png",
        "WebP" => "webp",
        "Bmp" => "bmp",
        "Gif" => "gif",
        "Tiff" => "tiff",
        _ => match content_type.unwrap_or_default() {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/webp" => "webp",
            "image/gif" => "gif",
            "image/bmp" => "bmp",
            "image/tiff" => "tiff",
            _ => "img",
        },
    }
}

fn url_hash(url: &Url) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.as_str().hash(&mut hasher);
    hasher.finish()
}

fn existing_names(dir: &Path) -> HashSet<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Crawler e robots.txt
// ---------------------------------------------------------------------------

/// Regole di robots.txt per il nostro User-Agent (o il gruppo "*").
pub struct Robots {
    rules: Vec<Rule>,
    disallow_all: bool,
    crawl_delay: Option<Duration>,
}

struct Rule {
    allow: bool,
    pattern: String,
    regex: Regex,
}

impl Rule {
    fn new(allow: bool, pattern: &str) -> Self {
        // '*' è un jolly, '$' ancora la fine del percorso.
        let end_anchored = pattern.strip_suffix('$');
        let body = end_anchored.unwrap_or(pattern);
        let mut expression = String::from("^");
        for character in body.chars() {
            if character == '*' {
                expression.push_str(".*");
            } else {
                expression.push_str(&regex::escape(&character.to_string()));
            }
        }
        if end_anchored.is_some() {
            expression.push('$');
        }
        Self {
            allow,
            pattern: pattern.to_string(),
            regex: Regex::new(&expression).unwrap(),
        }
    }
}

impl Robots {
    fn allow_all() -> Self {
        Self {
            rules: Vec::new(),
            disallow_all: false,
            crawl_delay: None,
        }
    }

    /// Analizza robots.txt: gruppi individuati da righe User-agent consecutive,
    /// regole Allow/Disallow con jolly "*" e ancoraggio finale "$".
    pub fn parse(text: &str, user_agent: &str) -> Self {
        let agent = user_agent.to_ascii_lowercase();
        let mut rules = Vec::new();
        let mut crawl_delay = None;
        let mut agents: Vec<String> = Vec::new();
        let mut collecting_agents = true;
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" if collecting_agents => agents.push(value.to_ascii_lowercase()),
                "user-agent" => {
                    agents = vec![value.to_ascii_lowercase()];
                    collecting_agents = true;
                }
                _ if agents.is_empty() => {}
                "disallow" => {
                    collecting_agents = false;
                    if group_applies(&agents, &agent) && !value.is_empty() {
                        rules.push(Rule::new(false, value));
                    }
                }
                "allow" => {
                    collecting_agents = false;
                    if group_applies(&agents, &agent) && !value.is_empty() {
                        rules.push(Rule::new(true, value));
                    }
                }
                "crawl-delay" => {
                    collecting_agents = false;
                    if group_applies(&agents, &agent) {
                        crawl_delay = value
                            .parse::<f64>()
                            .ok()
                            .filter(|seconds| *seconds >= 0.0)
                            .map(Duration::from_secs_f64);
                    }
                }
                _ => {}
            }
        }
        Self {
            rules,
            disallow_all: false,
            crawl_delay,
        }
    }

    /// Legge robots.txt dalla radice dell'host: assente (404) permette tutto,
    /// 401/403 vieta tutto, gli altri errori permettono (RFC 9309).
    fn load(agent: &ureq::Agent, site: &Url) -> Self {
        let mut url = site.clone();
        url.set_path("/robots.txt");
        url.set_query(None);
        url.set_fragment(None);
        let Ok(mut response) = request(agent, &url, ACCEPT_HTML, None, None) else {
            return Self::allow_all();
        };
        match response.status().as_u16() {
            401 | 403 => Self {
                rules: Vec::new(),
                disallow_all: true,
                crawl_delay: None,
            },
            status if status >= 400 => Self::allow_all(),
            _ => {
                let bytes = read_body(&mut response, PAGE_LIMIT).unwrap_or_default();
                Self::parse(&String::from_utf8_lossy(&bytes), USER_AGENT)
            }
        }
    }

    fn allowed(&self, url: &Url) -> bool {
        if self.disallow_all {
            return false;
        }
        let mut target = url.path().to_string();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        // Vince la regola col pattern originale più lungo.
        let mut best: Option<&Rule> = None;
        for rule in &self.rules {
            if rule.regex.is_match(&target)
                && best.is_none_or(|current| rule.pattern.len() > current.pattern.len())
            {
                best = Some(rule);
            }
        }
        best.is_none_or(|rule| rule.allow)
    }
}

fn group_applies(agents: &[String], user_agent: &str) -> bool {
    agents
        .iter()
        .any(|candidate| candidate == "*" || user_agent.contains(candidate.as_str()))
}

/// Stesso host e porta: il crawler non lascia il sito di partenza.
fn same_origin(one: &Url, other: &Url) -> bool {
    one.host_str() == other.host_str()
        && one.port_or_known_default() == other.port_or_known_default()
}

/// Collegamenti <a href> dello stesso host, senza frammenti, verso pagine
/// plausibili (non file immagine né scaricabili).
fn page_links(html: &str, page: &Url) -> Vec<Url> {
    let document = Html::parse_document(html);
    let anchor = Selector::parse("a[href]").unwrap();
    let mut links = Vec::new();
    for element in document.select(&anchor) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(mut absolute) = page.join(href) else {
            continue;
        };
        if !matches!(absolute.scheme(), "http" | "https") {
            continue;
        }
        if !same_origin(page, &absolute) || looks_like_image(href) || downloadable(&absolute) {
            continue;
        }
        absolute.set_fragment(None);
        links.push(absolute);
    }
    links
}

fn downloadable(url: &Url) -> bool {
    let file = url.path().rsplit('/').next().unwrap_or_default();
    let Some(extension) = file.rsplit_once('.').map(|(_, extension)| extension) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "pdf" | "zip" | "rar" | "7z" | "gz" | "tar" | "mp3" | "mp4" | "avi" | "mov" | "m4a"
            | "wav" | "woff" | "woff2" | "ttf" | "otf" | "css" | "js" | "json" | "ico"
    )
}

/// Visita il sito in ampiezza (BFS) partendo dall'URL dato: stesso host,
/// solo risposte text/html, al massimo `max_pages` pagine, quattro fetch
/// paralleli che rispettano robots.txt e l'eventuale Crawl-delay.
fn crawl(
    agent: &ureq::Agent,
    start: &Url,
    max_pages: u32,
    robots: &Arc<Robots>,
    cancel: &Arc<AtomicBool>,
) -> Vec<(Url, String)> {
    struct Shared {
        queue: VecDeque<Url>,
        queued: HashSet<String>,
        started: usize,
        inflight: usize,
    }
    let shared = Arc::new((
        Mutex::new(Shared {
            queue: VecDeque::from([start.clone()]),
            queued: HashSet::from([start.to_string()]),
            started: 0,
            inflight: 0,
        }),
        Condvar::new(),
    ));
    let pages = Arc::new(Mutex::new(Vec::<(Url, String)>::new()));
    let last_request = Arc::new(Mutex::new(Option::<Instant>::None));

    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let shared = Arc::clone(&shared);
        let pages = Arc::clone(&pages);
        let robots = Arc::clone(robots);
        let last_request = Arc::clone(&last_request);
        let cancel = Arc::clone(cancel);
        let agent = agent.clone();
        handles.push(std::thread::spawn(move || loop {
            let url = {
                let mut guard = shared.0.lock().unwrap();
                loop {
                    if cancel.load(Ordering::Relaxed) || guard.started >= max_pages as usize {
                        guard.queue.clear();
                        drop(guard);
                        shared.1.notify_all();
                        return;
                    }
                    if let Some(url) = guard.queue.pop_front() {
                        guard.started += 1;
                        guard.inflight += 1;
                        break url;
                    }
                    if guard.inflight == 0 {
                        drop(guard);
                        shared.1.notify_all();
                        return;
                    }
                    guard = shared.1.wait(guard).unwrap();
                }
            };
            if let Some(delay) = robots.crawl_delay {
                // Il lock trattenuto durante l'attesa distanzia tutte le
                // richieste del sito, come chiede Crawl-delay.
                let mut last = last_request.lock().unwrap();
                if let Some(previous) = *last {
                    let elapsed = previous.elapsed();
                    if elapsed < delay {
                        std::thread::sleep(delay - elapsed);
                    }
                }
                *last = Some(Instant::now());
            }
            if !robots.allowed(&url) {
                shared.0.lock().unwrap().inflight -= 1;
                shared.1.notify_all();
                continue;
            }
            if let Ok(html) = fetch_page(&agent, &url) {
                for link in page_links(&html, &url) {
                    let mut guard = shared.0.lock().unwrap();
                    if guard.queued.insert(link.to_string()) {
                        guard.queue.push_back(link);
                    }
                }
                pages.lock().unwrap().push((url, html));
            }
            shared.0.lock().unwrap().inflight -= 1;
            shared.1.notify_all();
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
    Arc::try_unwrap(pages)
        .map(|pages| pages.into_inner().unwrap_or_default())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Orchestrazione
// ---------------------------------------------------------------------------

/// Analizza una pagina (o un intero sito), verifica con richieste parziali
/// le immagini trovate e invia gli eventi alla UI.
pub fn scan(start: &Url, options: &ScanOptions, cancel: &Arc<AtomicBool>, tx: SyncSender<ScanEvent>) {
    let agent = agent();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    if options.crawl_site {
        let _ = tx.send(ScanEvent::Status("Lettura di robots.txt…".into()));
        let robots = Arc::new(Robots::load(&agent, start));
        let _ = tx.send(ScanEvent::Status(format!(
            "Analisi del sito in corso (al massimo {} pagine)…",
            options.max_pages
        )));
        for (page, html) in crawl(&agent, start, options.max_pages, &robots, cancel) {
            collect_from_html(&html, &page, &mut candidates, &mut seen);
        }
    } else {
        let _ = tx.send(ScanEvent::Status(format!("Analisi di {start}…")));
        match fetch_page(&agent, start) {
            Ok(html) => collect_from_html(&html, start, &mut candidates, &mut seen),
            Err(err) => {
                let _ = tx.send(ScanEvent::Error(format!("{start}: {err:#}")));
            }
        }
    }

    if cancel.load(Ordering::Relaxed) || candidates.is_empty() {
        if candidates.is_empty() {
            let _ = tx.send(ScanEvent::Status("Nessuna immagine trovata.".into()));
        }
        let _ = tx.send(ScanEvent::Finished);
        return;
    }
    let _ = tx.send(ScanEvent::Status(format!(
        "Trovate {} immagini uniche, verifica in corso…",
        candidates.len()
    )));

    let agent = Arc::new(agent);
    let fingerprints = Arc::new(Mutex::new(HashSet::<Fingerprint>::new()));
    let min_kb = options.min_kb;
    let worker_tx = tx.clone();
    pool(candidates, cancel, move |candidate| {
        match probe_candidate(&agent, &candidate, min_kb, &fingerprints) {
            Ok(Some(image)) => {
                let _ = worker_tx.send(ScanEvent::Image(Box::new(image)));
            }
            Ok(None) => {}
            Err(err) => {
                let _ = worker_tx.send(ScanEvent::Error(format!("{}: {err:#}", candidate.primary)));
            }
        }
    });
    let _ = tx.send(ScanEvent::Finished);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write as _,
        net::{TcpListener, TcpStream},
        sync::mpsc,
    };

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn srcset_picks_highest_descriptor() {
        assert_eq!(
            best_srcset("a.jpg 480w, b.jpg 1920w, c.jpg 800w").as_deref(),
            Some("b.jpg")
        );
        assert_eq!(best_srcset("a.jpg 1x, b.jpg 2x").as_deref(), Some("b.jpg"));
        // Nelle liste miste i descriptor "w" battono gli "x".
        assert_eq!(best_srcset("a.jpg 2x, b.jpg 800w").as_deref(), Some("b.jpg"));
        assert_eq!(best_srcset("solo.jpg").as_deref(), Some("solo.jpg"));
        assert_eq!(best_srcset(" , ,"), None);
    }

    #[test]
    fn extraction_covers_every_source_and_resolves_relative() {
        let html = r#"<html><head>
            <meta property="og:image" content="/social/card.jpg">
            <meta name="twitter:image" content="https://cdn.esempio.it/tw.png">
            <style>.hero { background-image: url('../img/hero.jpg') }</style>
        </head><body>
            <img src="foto-300x200.jpg">
            <img data-src="lazy.webp" data-original="original-full.png">
            <img src="piccola.png" srcset="s.jpg 480w, l.jpg 1920w">
            <img data-srcset="a.jpg 1x, b.jpg 2x">
            <picture><source srcset="x1.avif 800w, x2.jpg 1600w"><img src="default.jpg"></picture>
            <a href="/downloads/bigmap.png">scarica</a>
            <a href="/pagine/altra">link</a>
            <div style="background-image: url('bg-300x200.jpg')">ciao</div>
            <img src="data:image/gif;base64,R0lGOD">
            <img src="blob:https://esempio.it/xyz">
        </body></html>"#;
        let page = url("https://esempio.it/gallery/pagina/");
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        collect_from_html(html, &page, &mut candidates, &mut seen);

        let primaries: Vec<String> = candidates.iter().map(|c| c.primary.to_string()).collect();
        for expected in [
            "https://esempio.it/social/card.jpg",
            "https://cdn.esempio.it/tw.png",
            "https://esempio.it/gallery/img/hero.jpg",
            "https://esempio.it/gallery/pagina/lazy.webp",
            "https://esempio.it/gallery/pagina/original-full.png",
            "https://esempio.it/gallery/pagina/l.jpg",
            "https://esempio.it/gallery/pagina/b.jpg",
            "https://esempio.it/gallery/pagina/x2.jpg",
            "https://esempio.it/gallery/pagina/default.jpg",
            "https://esempio.it/downloads/bigmap.png",
        ] {
            assert!(primaries.contains(&expected.to_string()), "manca {expected}");
        }
        assert_eq!(primaries.len(), 13, "primitive: {primaries:?}");
        // data: e blob: non diventano mai candidati.
        assert!(primaries.iter().all(|u| u.starts_with("https://")));
        // Le miniature con suffisso dimensione provano prima l'originale.
        let bg = candidates
            .iter()
            .find(|c| c.primary.path().ends_with("/bg.jpg"))
            .expect("bg.jpg upgradato");
        assert_eq!(
            bg.fallback.as_ref().map(Url::as_str),
            Some("https://esempio.it/gallery/pagina/bg-300x200.jpg")
        );
    }

    #[test]
    fn full_size_collapses_thumbnail_and_original() {
        let html = r#"<img src="a-150x150.jpg"><img src="a.jpg">"#;
        let page = url("https://sito.it/");
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        collect_from_html(html, &page, &mut candidates, &mut seen);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].primary.path(), "/a.jpg");
        assert!(candidates[0].fallback.is_some());
    }

    #[test]
    fn upgraded_strips_wordpress_shopify_and_cdn_params() {
        // WordPress "-300x200".
        assert_eq!(
            upgraded(&url("https://sito.it/wp-content/uploads/foto-300x200.jpg"))
                .as_ref()
                .map(Url::path),
            Some("/wp-content/uploads/foto.jpg")
        );
        // WordPress "-scaled" e combinazioni.
        assert_eq!(
            upgraded(&url("https://sito.it/foto-scaled.jpg")).as_ref().map(Url::path),
            Some("/foto.jpg")
        );
        assert_eq!(
            upgraded(&url("https://sito.it/foto-1024x768-scaled.jpg"))
                .as_ref()
                .map(Url::path),
            Some("/foto.jpg")
        );
        // Shopify "_1024x1024_crop_center".
        assert_eq!(
            upgraded(&url("https://cdn.shopify.net/s/files/1/0001/prod_1024x1024_crop_center.jpg"))
                .as_ref()
                .map(Url::path),
            Some("/s/files/1/0001/prod.jpg")
        );
        // Parametri CDN di trasformazione.
        let clean = upgraded(&url("https://pics.example.net/800/600.jpg?w=300&h=200&fit=crop&quality=80"));
        assert_eq!(clean.as_ref().map(Url::as_str), Some("https://pics.example.net/800/600.jpg"));
        // I parametri innocui restano.
        let versioned = upgraded(&url("https://sito.it/img.jpg?v=2&w=300"));
        assert_eq!(
            versioned.as_ref().map(Url::as_str),
            Some("https://sito.it/img.jpg?v=2")
        );
        // Gli URL già pieni non cambiano.
        assert_eq!(upgraded(&url("https://sito.it/foto.jpg")), None);
    }

    #[test]
    fn image_header_reads_dimensions_and_rejects_unsupported() {
        let (format, dimensions) = image_header(&png_bytes(320, 200)).unwrap();
        assert_eq!(format, "Png");
        assert_eq!(dimensions, [320, 200]);
        assert_eq!(image_header(b"<svg xmlns='http://www.w3.org/2000/svg'></svg>"), None);
        assert_eq!(image_header(b"\x00\x00\x00\x18ftypavif\x00\x00\x00\x00avif"), None);
    }

    #[test]
    fn robots_longest_rule_wins_with_wildcards() {
        let text = "User-agent: *\nDisallow: /admin\nAllow: /admin/public\nDisallow: /*.pdf$\nCrawl-delay: 3\n\nUser-agent: AltroBot\nDisallow: /\n";
        let robots = Robots::parse(text, USER_AGENT);
        assert!(robots.allowed(&url("https://sito.it/pagine/ok")));
        assert!(!robots.allowed(&url("https://sito.it/admin/privato")));
        assert!(robots.allowed(&url("https://sito.it/admin/public")));
        assert!(!robots.allowed(&url("https://sito.it/docs/a.pdf")));
        assert!(robots.allowed(&url("https://sito.it/docs/a.pdfx")));
        assert_eq!(robots.crawl_delay, Some(Duration::from_secs(3)));
    }

    #[test]
    fn unique_file_names_number_collisions() {
        let mut taken = HashSet::new();
        assert_eq!(unique_file_name("foto", "jpg", &mut taken), "foto.jpg");
        assert_eq!(unique_file_name("foto", "jpg", &mut taken), "foto (2).jpg");
        assert_eq!(unique_file_name("foto", "jpg", &mut taken), "foto (3).jpg");
    }

    #[test]
    fn file_parts_sanitizes_and_completes_extension() {
        let (stem, extension) = file_parts(&url("https://x.it/p/Caffè premium.png"), "Png", None);
        assert_eq!(extension, "png");
        assert_eq!(stem, "Caff__premium");
        let (stem, extension) = file_parts(&url("https://x.it/render?id=9"), "WebP", None);
        assert_eq!((stem.as_str(), extension.as_str()), ("render", "webp"));
        let (stem, _) = file_parts(&url("https://x.it/"), "Jpeg", Some("image/jpeg"));
        assert!(stem.starts_with("immagine-"), "stem: {stem}");
    }

    #[test]
    fn content_range_total_parses() {
        assert_eq!(content_range_total("bytes 0-262143/1834572"), Some(1834572));
        assert_eq!(content_range_total("bytes 0-9/100"), Some(100));
        assert_eq!(content_range_total("bytes */0"), Some(0));
        assert_eq!(content_range_total("bytes 0-9"), None);
    }

    // -----------------------------------------------------------------------
    // Server HTTP locale: nessuna rete esterna nei test.
    // -----------------------------------------------------------------------

    /// (percorso richiesto, User-Agent, presenza dell'header Range)
    type RequestLog = Arc<Mutex<Vec<(String, String, bool)>>>;

    struct TestServer {
        base: Url,
        addr: std::net::SocketAddr,
        log: RequestLog,
    }

    impl TestServer {
        fn route(&self, path: &str) -> bool {
            self.log.lock().unwrap().iter().any(|(served, _, _)| served == path)
        }
    }

    fn status_text(code: u16) -> &'static str {
        match code {
            200 => "200 OK",
            206 => "206 Partial Content",
            404 => "404 Not Found",
            _ => "500 Internal Server Error",
        }
    }

    fn spawn_server(routes: Vec<(&str, u16, &str, Vec<u8>)>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: std::collections::HashMap<String, (u16, String, Vec<u8>)> = routes
            .into_iter()
            .map(|(path, status, content_type, body)| {
                (path.to_string(), (status, content_type.to_string(), body))
            })
            .collect();
        let log: RequestLog = Arc::new(Mutex::new(Vec::new()));
        let server = TestServer {
            base: url(&format!("http://{addr}")),
            addr,
            log: Arc::clone(&log),
        };
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = String::new();
                let mut buffer = [0u8; 8192];
                while !request.contains("\r\n\r\n") {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    request.push_str(&String::from_utf8_lossy(&buffer[..read]));
                }
                let mut lines = request.lines();
                let path = lines
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                let mut user_agent = String::new();
                let mut ranged = false;
                for line in request.lines() {
                    if let Some((name, value)) = line.split_once(':') {
                        match name.trim().to_ascii_lowercase().as_str() {
                            "user-agent" => user_agent = value.trim().to_string(),
                            "range" => ranged = true,
                            _ => {}
                        }
                    }
                }
                log.lock().unwrap().push((path.clone(), user_agent, ranged));
                if path == "/shutdown" {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                }
                let (status, content_type, body) = routes
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, "text/plain".into(), b"non trovato".to_vec()));
                // Il server onora Range sui file immagine.
                let (status, extra) =
                    if ranged && status == 200 && content_type.starts_with("image/") {
                        (
                            206u16,
                            format!("Content-Range: bytes 0-{}/{}\r\n", body.len() - 1, body.len()),
                        )
                    } else {
                        (status, String::new())
                    };
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Accept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    status_text(status),
                    content_type,
                    body.len(),
                    extra
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        server
    }

    fn shutdown(server: &TestServer) {
        if let Ok(mut stream) = TcpStream::connect(server.addr) {
            let _ = stream.write_all(b"GET /shutdown HTTP/1.1\r\nHost: test\r\n\r\n");
            let _ = stream.read(&mut [0u8; 128]);
        }
    }

    fn run_scan(start: &Url, options: &ScanOptions) -> (Vec<WebImage>, Vec<String>) {
        let (tx, rx) = mpsc::sync_channel(64);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let start = start.clone();
            let options = options.clone();
            std::thread::spawn(move || scan(&start, &options, &cancel, tx))
        };
        let mut images = Vec::new();
        let mut errors = Vec::new();
        for event in rx {
            match event {
                ScanEvent::Image(image) => images.push(*image),
                ScanEvent::Error(error) => errors.push(error),
                ScanEvent::Status(_) => {}
                ScanEvent::Finished => break,
            }
        }
        worker.join().unwrap();
        (images, errors)
    }

    #[test]
    fn probe_uses_range_upgrades_and_fingerprints() {
        let png_a = png_bytes(120, 80);
        let png_b = png_bytes(90, 60);
        let png_c = png_bytes(200, 100);
        let gallery = r#"<img src="/foto-300x200.jpg"><img src="/other/foto-300x200.jpg"><a href="/big.png">g</a><img src="/ripetuta.png">"#;
        let server = spawn_server(vec![
            ("/gallery", 200, "text/html; charset=utf-8", gallery.as_bytes().to_vec()),
            // L'originale ricostruito non esiste: si ricade sulla miniatura.
            ("/foto.jpg", 404, "text/plain", b"no".to_vec()),
            ("/foto-300x200.jpg", 200, "image/png", png_a.clone()),
            // Stesso nome file, contenuto diverso: nessuna collisione di impronta.
            ("/other/foto-300x200.jpg", 200, "image/png", png_b.clone()),
            // big.png e ripetuta.png sono la stessa foto: collassano per impronta.
            ("/big.png", 200, "image/png", png_c.clone()),
            ("/ripetuta.png", 200, "image/png", png_c.clone()),
        ]);

        let (images, errors) = run_scan(
            &server.base.join("/gallery").unwrap(),
            &ScanOptions { crawl_site: false, max_pages: 1, min_kb: 0 },
        );
        assert!(errors.is_empty(), "errori: {errors:?}");

        // 3 voci su 4 candidate: la duplicata per impronta digitale è caduta.
        assert_eq!(images.len(), 3);
        let urls: Vec<String> = images.iter().map(|image| image.url.to_string()).collect();
        let fallback = images
            .iter()
            .find(|image| image.url.path() == "/foto-300x200.jpg")
            .expect("ricaduta sulla miniatura dopo il 404 dell'originale");
        assert_eq!(
            fallback.alternate.as_ref().map(Url::path),
            Some("/foto.jpg")
        );
        assert_eq!(fallback.dimensions, [120, 80]);
        assert_eq!(fallback.format, "Png");
        assert!(fallback.preview.is_some());
        let other = images
            .iter()
            .find(|image| image.url.path() == "/other/foto-300x200.jpg")
            .expect("seconda foto con stesso nome file");
        assert_eq!(other.dimensions, [90, 60]);
        // Della coppia identica big.png/ripetuta.png ne sopravvive una sola.
        assert_eq!(
            urls.iter().filter(|u| u.ends_with(".png")).count(),
            1,
            "url: {urls:?}"
        );

        // Tutte le richieste usano lo User-Agent di browser, senza suffissi,
        // e le immagini vengono verificate con GET parziale, non con HEAD.
        let log = server.log.lock().unwrap();
        for (path, agent, ranged) in log.iter() {
            assert_eq!(agent, USER_AGENT, "user-agent per {path}: {agent}");
            if path.ends_with(".jpg") || path.ends_with(".png") {
                assert!(ranged, "manca l'header Range per {path}");
            }
        }
        drop(log);
        shutdown(&server);
    }

    #[test]
    fn min_kb_filter_drops_tiny_images() {
        let png = png_bytes(64, 64);
        let server = spawn_server(vec![
            ("/gallery", 200, "text/html", b"<img src=\"icona.png\">".to_vec()),
            ("/icona.png", 200, "image/png", png),
        ]);
        let start = server.base.join("/gallery").unwrap();
        let (images, _) = run_scan(
            &start,
            &ScanOptions { crawl_site: false, max_pages: 1, min_kb: 50 },
        );
        assert!(images.is_empty());
        shutdown(&server);
    }

    #[test]
    fn download_names_and_dedup_files() {
        let png_a = png_bytes(120, 80);
        let png_b = png_bytes(90, 60);
        let server = spawn_server(vec![
            (
                "/gallery",
                200,
                "text/html",
                b"<img src=\"/foto.jpg\"><img src=\"/altro/foto.jpg\">".to_vec(),
            ),
            ("/foto.jpg", 200, "image/png", png_a.clone()),
            ("/altro/foto.jpg", 200, "image/png", png_b.clone()),
        ]);
        let start = server.base.join("/gallery").unwrap();
        let (images, _) = run_scan(
            &start,
            &ScanOptions { crawl_site: false, max_pages: 1, min_kb: 0 },
        );
        assert_eq!(images.len(), 2);

        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::sync_channel(64);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let dir = dir.path().to_path_buf();
            let images = images.clone();
            std::thread::spawn(move || download(images, &dir, &cancel, tx));
        }
        let mut saved = Vec::new();
        for event in rx {
            match event {
                DownloadEvent::Saved(path) => saved.push(path),
                DownloadEvent::Error(error) => panic!("download: {error}"),
                DownloadEvent::Finished { cancelled } => {
                    assert!(!cancelled);
                    break;
                }
            }
        }
        let names: Vec<String> = saved
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Stesso nome nell'URL: la seconda diventa "foto (2).jpg".
        assert!(names.contains(&"foto.jpg".to_string()), "nomi: {names:?}");
        assert!(names.contains(&"foto (2).jpg".to_string()), "nomi: {names:?}");
        // Il contenuto è integro: uno dei due PNG originali per file.
        for path in &saved {
            let content = fs::read(path).unwrap();
            assert!(
                content == png_a || content == png_b,
                "contenuto inatteso in {}",
                path.display()
            );
        }
        shutdown(&server);
    }

    #[test]
    fn crawl_respects_robots_and_page_limit() {
        let png_a = png_bytes(100, 50);
        let png_b = png_bytes(130, 70);
        let page_a = r#"<a href="/b.html">b</a><a href="/private/x.html">p</a><img src="/a.png">"#;
        let page_b = r#"<a href="/c.html">c</a><img src="/b.png">"#;
        let robots = "User-agent: *\nDisallow: /private\n";
        let server = spawn_server(vec![
            ("/robots.txt", 200, "text/plain", robots.as_bytes().to_vec()),
            ("/a.html", 200, "text/html", page_a.as_bytes().to_vec()),
            ("/b.html", 200, "text/html", page_b.as_bytes().to_vec()),
            ("/a.png", 200, "image/png", png_a.clone()),
            ("/b.png", 200, "image/png", png_b.clone()),
        ]);

        let start = server.base.join("/a.html").unwrap();
        let (images, _) = run_scan(
            &start,
            &ScanOptions { crawl_site: true, max_pages: 10, min_kb: 0 },
        );
        // Le immagini arrivano da entrambe le pagine visitate.
        let paths: Vec<String> = images.iter().map(|image| image.url.path().to_string()).collect();
        assert!(paths.contains(&"/a.png".to_string()), "percorsi: {paths:?}");
        assert!(paths.contains(&"/b.png".to_string()), "percorsi: {paths:?}");
        // La pagina vietata da robots.txt non è mai stata richiesta.
        assert!(!server.route("/private/x.html"));
        shutdown(&server);
    }
}
