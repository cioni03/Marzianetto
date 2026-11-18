use crate::settings::Settings;
use anyhow::{bail, Context, Result};
use eframe::egui;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
};
use tempfile::TempDir;

const RELEASES: &str = "https://github.com/cioni03/Marzianetto/releases";
const LATEST: &str = "https://api.github.com/repos/cioni03/Marzianetto/releases/latest";
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    size: u64,
}

struct Update {
    tag: String,
    version: String,
    url: String,
    digest: String,
    size: u64,
}

fn stable_version(text: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = text.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }
    Some([
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ])
}

impl Release {
    fn newer(self, current: &str, target: &str, skipped: Option<&str>) -> Option<Update> {
        let version = self.tag_name.strip_prefix('v')?;
        if self.draft
            || self.prerelease
            || skipped == Some(self.tag_name.as_str())
            || stable_version(version)? <= stable_version(current)?
        {
            return None;
        }
        let name = format!("marzianetto-{target}.app.zip");
        let asset = self.assets.into_iter().find(|asset| asset.name == name)?;
        let digest = asset.digest?.strip_prefix("sha256:")?.to_ascii_lowercase();
        if digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || asset.size == 0
            || asset.size > MAX_ARCHIVE
        {
            return None;
        }
        Some(Update {
            version: version.to_owned(),
            url: format!("{RELEASES}/download/{}/{name}", self.tag_name),
            tag: self.tag_name,
            digest,
            size: asset.size,
        })
    }
}

fn checked(command: &mut Command) -> Result<Output> {
    let output = command.output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output)
}

fn curl() -> Command {
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "5",
        "--user-agent",
        "Marzianetto-Updater",
    ]);
    command
}

fn installed_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let bundle = executable.ancestors().nth(3)?;
    if bundle.extension()? != "app"
        || executable.strip_prefix(bundle).ok()? != Path::new("Contents/MacOS/marzianetto")
        || !bundle.join("Contents/Info.plist").is_file()
        || bundle.starts_with("/Volumes")
        || bundle
            .components()
            .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return None;
    }
    Some(bundle.to_owned())
}

fn check(skipped: Option<&str>) -> Result<Option<Update>> {
    let data = checked(curl().args(["--max-time", "12", "--max-filesize", "1048576", LATEST]))?;
    let release: Release = serde_json::from_slice(&data.stdout)?;
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else {
        "x86_64-apple-darwin"
    };
    Ok(release.newer(env!("CARGO_PKG_VERSION"), target, skipped))
}

struct Prepared {
    stage: TempDir,
    bundle: PathBuf,
}

fn prepare(update: Update, bundle: PathBuf) -> Result<Prepared> {
    // Stesso filesystem: la sostituzione avviene con rinomine, non copiando sull'app.
    let stage = tempfile::Builder::new()
        .prefix(".marzianetto-update-")
        .tempdir_in(bundle.parent().context("Cartella dell'app non valida")?)
        .context("Non posso scrivere accanto all'app. Usa una cartella scrivibile, ad esempio ~/Applications.")?;
    let zip = stage.path().join("update.zip");
    checked(
        curl()
            .args(["--max-time", "300", "--max-filesize"])
            .arg(update.size.to_string())
            .arg("--output")
            .arg(&zip)
            .arg(update.url),
    )
    .context("Download non riuscito")?;
    if zip.metadata()?.len() != update.size {
        bail!("Dimensione del pacchetto non valida.");
    }
    let hash = checked(
        Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(&zip),
    )?;
    if String::from_utf8_lossy(&hash.stdout)
        .split_whitespace()
        .next()
        != Some(update.digest.as_str())
    {
        bail!("Verifica SHA-256 non riuscita.");
    }
    checked(
        Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(stage.path()),
    )
    .context("Estrazione del pacchetto non riuscita")?;
    let app = stage.path().join("marzianetto.app");
    let plist = app.join("Contents/Info.plist");
    for (key, expected) in [
        ("CFBundleIdentifier", "it.marzianetto.app"),
        ("CFBundleShortVersionString", update.version.as_str()),
        ("CFBundleExecutable", "marzianetto"),
    ] {
        let actual = checked(
            Command::new("/usr/bin/plutil")
                .args(["-extract", key, "raw", "-o", "-"])
                .arg(&plist),
        )?;
        if String::from_utf8_lossy(&actual.stdout).trim() != expected {
            bail!("Il pacchetto scaricato non corrisponde alla release.");
        }
    }
    // Verifica l'integrità della firma ad hoc già prodotta dal packaging.
    checked(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&app),
    )
    .context("Bundle macOS non valido")?;
    Ok(Prepared { stage, bundle })
}

// Lo script è costante: i percorsi sono passati come argomenti, non interpolati.
const REPLACE: &str = r#"
pid=$1
stage=$2
app=$3
new="$stage/marzianetto.app"
old="$stage/precedente.app"
notify() {
    /usr/bin/osascript -e 'display alert "Aggiornamento di Marzianetto non riuscito" message "Controlla la cartella dell’app o scarica la release da GitHub."'
}
for ((i=0; i<150; i++)); do
    kill -0 "$pid" 2>/dev/null || break
    /bin/sleep 0.2
done
if kill -0 "$pid" 2>/dev/null; then
    /bin/rm -rf "$stage"
    notify
    exit 1
fi
if ! /bin/mv "$app" "$old"; then
    /bin/rm -rf "$stage"
    /usr/bin/open -n "$app"
    notify
    exit 1
fi
if /bin/mv "$new" "$app"; then
    if /usr/bin/open -n "$app"; then
        /bin/rm -rf "$stage"
        exit 0
    fi
    /bin/mv "$app" "$new" || { notify; exit 1; }
fi
if /bin/mv "$old" "$app"; then
    /bin/rm -rf "$stage"
    /usr/bin/open -n "$app"
fi
notify
"#;

impl Prepared {
    fn restart(self) -> Result<()> {
        Command::new("/bin/bash")
            .args([
                "-c",
                REPLACE,
                "marzianetto-updater",
                &std::process::id().to_string(),
            ])
            .arg(self.stage.path())
            .arg(&self.bundle)
            .env_remove("BASH_ENV")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Impossibile avviare l'installazione")?;
        let _ = self.stage.keep();
        Ok(())
    }
}

#[derive(Default)]
pub struct Updater {
    check: Option<Receiver<Result<Option<Update>>>>,
    offered: Option<Update>,
    download: Option<Receiver<Result<Prepared>>>,
    error: Option<String>,
}

fn worker<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let ctx = ctx.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}

impl Updater {
    pub fn start(ctx: &egui::Context, skipped: Option<String>) -> Self {
        Self {
            check: installed_bundle().map(|_| worker(ctx, move || check(skipped.as_deref()))),
            ..Default::default()
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) -> bool {
        if let Some(result) = self.check.as_ref().map(Receiver::try_recv) {
            match result {
                Ok(result) => {
                    self.check = None;
                    self.offered = result.ok().flatten();
                }
                Err(mpsc::TryRecvError::Disconnected) => self.check = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(result) = self.download.as_ref().map(Receiver::try_recv) {
            match result {
                Ok(result) => {
                    self.download = None;
                    match result.and_then(Prepared::restart) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(err) => self.error = Some(format!("{err:#}")),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.download = None;
                    self.error = Some("Download interrotto.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        let mut changed = false;
        if self.offered.is_some() || self.download.is_some() || self.error.is_some() {
            egui::Modal::new(egui::Id::new("app_update")).show(ctx, |ui| {
                if let Some(error) = &self.error {
                    ui.heading("Aggiornamento non riuscito");
                    ui.label(error);
                    ui.hyperlink_to("Apri GitHub Releases", RELEASES);
                    if ui.button("Chiudi").clicked() {
                        self.error = None;
                    }
                } else if self.download.is_some() {
                    ui.heading("Aggiornamento in preparazione");
                    ui.spinner();
                    ui.label("Marzianetto si riavvierà quando il pacchetto sarà pronto.");
                } else if let Some(update) = &self.offered {
                    ui.heading(format!("Marzianetto {} disponibile", update.tag));
                    ui.label("L'app si chiuderà e si riaprirà al termine dell'aggiornamento.");
                    ui.horizontal(|ui| {
                        if ui.button("Aggiorna").clicked() {
                            if let (Some(update), Some(bundle)) =
                                (self.offered.take(), installed_bundle())
                            {
                                self.download = Some(worker(ctx, move || prepare(update, bundle)));
                            } else {
                                self.error = Some("Cartella dell'app non disponibile.".into());
                            }
                        } else if ui.button("Salta questa versione").clicked() {
                            settings.skipped_update = self.offered.take().map(|update| update.tag);
                            changed = true;
                        }
                    });
                }
            });
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TARGET: &str = "aarch64-apple-darwin";

    fn release(tag: &str) -> Release {
        serde_json::from_value(serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [{
                "name": format!("marzianetto-{TARGET}.app.zip"),
                "size": 1024, "digest": format!("sha256:{}", "AB".repeat(32)),
            }]
        }))
        .unwrap()
    }

    #[test]
    fn picks_new_stable_release_and_matching_asset() {
        let update = release("v0.10.0")
            .newer("0.9.0", TARGET, Some("v0.9.5"))
            .unwrap();
        assert_eq!(update.version, "0.10.0");
        assert_eq!(update.tag, "v0.10.0");
        assert_eq!(update.digest, "ab".repeat(32));
        assert_eq!(
            update.url,
            format!("{RELEASES}/download/v0.10.0/marzianetto-{TARGET}.app.zip")
        );
        assert!(release("v1.0.0")
            .newer("0.9.0", "x86_64-apple-darwin", None)
            .is_none());
    }

    #[test]
    fn ignores_skipped_older_and_unstable_versions() {
        for tag in ["v0.9.0", "v0.8.9", "v1.0.0-rc.1", "v1.0.0/../x", "v01.0.0"] {
            assert!(release(tag).newer("0.9.0", TARGET, None).is_none());
        }
        assert!(release("v1.0.0")
            .newer("0.9.0", TARGET, Some("v1.0.0"))
            .is_none());
        assert!(release("v1.0.1")
            .newer("0.9.0", TARGET, Some("v1.0.0"))
            .is_some());
        let mut candidate = release("v2.0.0");
        candidate.prerelease = true;
        assert!(candidate.newer("1.0.0", TARGET, None).is_none());
    }

    #[test]
    fn requires_a_digest_and_bounded_package_size() {
        for (size, digest) in [
            (0, Some("f".repeat(64))),
            (MAX_ARCHIVE + 1, Some("f".repeat(64))),
            (1024, None),
            (1024, Some("z".repeat(64))),
        ] {
            let mut candidate = release("v1.0.0");
            candidate.assets[0].size = size;
            candidate.assets[0].digest = digest.map(|digest| format!("sha256:{digest}"));
            assert!(candidate.newer("0.9.0", TARGET, None).is_none());
        }
    }

    #[test]
    fn install_helper_replaces_bundle_or_restores_it_if_launch_fails() {
        for launch_ok in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let app = root.path().join("Marzianetto con spazi.app");
            let stage = root.path().join("staging");
            let new = stage.join("marzianetto.app");
            fs::create_dir(&app).unwrap();
            fs::create_dir_all(&new).unwrap();
            fs::write(app.join("version"), "old").unwrap();
            fs::write(new.join("version"), "new").unwrap();
            // Non avvia interfacce grafiche: simula solo l'esito di `open`.
            let script = REPLACE
                .replace(
                    "/usr/bin/open -n",
                    if launch_ok {
                        "/usr/bin/true"
                    } else {
                        "/usr/bin/false"
                    },
                )
                .replace("/usr/bin/osascript", "/usr/bin/false");
            let mut exited = Command::new("/usr/bin/true").spawn().unwrap();
            let pid = exited.id().to_string();
            assert!(exited.wait().unwrap().success());
            let result = Command::new("/bin/bash")
                .args(["-c", &script, "test-updater", &pid])
                .arg(&stage)
                .arg(&app)
                .status()
                .unwrap();
            assert_eq!(result.success(), launch_ok);
            assert_eq!(
                fs::read_to_string(app.join("version")).unwrap(),
                if launch_ok { "new" } else { "old" }
            );
            assert!(!stage.exists());
        }
    }
}
