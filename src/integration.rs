//! Integrazione per utente, senza privilegi amministrativi e senza cambiare
//! associazioni predefinite. Rimuove esclusivamente risorse marcate come nostre.
use anyhow::{bail, Context, Result};
#[cfg(not(target_os = "windows"))]
use directories::BaseDirs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "windows"))]
use std::{fs, io::Write};

const OWNER: &str = "it.marzianetto.context-menu.v1";
const LEGACY_OWNER: &str = "it.imagecompressor.context-menu.v1";
const LABEL: &str = "Invia a marzianetto";

fn launcher_path() -> std::io::Result<PathBuf> {
    // current_exe() dentro un'AppImage è un percorso di mount temporaneo.
    #[cfg(target_os = "linux")]
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe()
}

pub fn description() -> &'static str {
    if std::env::var_os("FLATPAK_ID").is_some() {
        "Flatpak: il menu va installato dall'host Linux con scripts/install-flatpak-context-menu.py. L'app nella sandbox non può registrare correttamente un eseguibile /app nel menu dell'host."
    } else if cfg!(target_os = "macos") {
        "Finder: clic destro → Azioni rapide / Servizi → Invia a marzianetto. Se necessario abilita l'azione in Impostazioni di Sistema → Tastiera → Abbreviazioni → Servizi."
    } else if cfg!(target_os = "windows") {
        "Explorer: clic destro su immagini o cartelle → Invia a marzianetto. Su Windows 11 la voce è nel menu Mostra altre opzioni."
    } else {
        "Dolphin: menu Azioni. Nautilus e Nemo: menu Script (richiede Python 3). Negli altri file manager usa Apri con → marzianetto; non esiste un menu contestuale universale su Linux."
    }
}

/// Apre una cartella nel file manager di sistema (Finder, Esplora file o quello
/// predefinito su Linux). Best-effort: un eventuale errore non tocca l'interfaccia.
pub fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    let _ = command.arg(path).spawn();
}

/// Mostra un file nel file manager, selezionato al suo interno.
pub fn reveal_in_folder(path: &Path) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        // xdg-open non seleziona: apre la cartella che contiene il file.
        let mut command = std::process::Command::new("xdg-open");
        if let Some(parent) = path.parent() {
            command.arg(parent);
        }
        command
    };
    let _ = command.spawn();
}

pub fn install() -> Result<String> {
    if std::env::var_os("FLATPAK_ID").is_some() {
        bail!("{}", description());
    }
    let executable = launcher_path()?;
    #[cfg(target_os = "windows")]
    windows::install(&executable)?;
    #[cfg(not(target_os = "windows"))]
    {
        let files = native_files(&executable)?;
        // Prima verifica tutte le destinazioni: non sovrascrivere risorse utente.
        for file in &files {
            check_owned(&file.path)?;
        }
        for file in &files {
            write(file)?;
        }
        #[cfg(target_os = "macos")]
        remove_legacy_macos()?;
        #[cfg(target_os = "linux")]
        remove_legacy_linux()?;
        refresh();
    }
    Ok(format!(
        "Menu installato. {}\nEseguibile: {}\nSe sposti il programma, reinstalla il menu.",
        description(),
        executable.display()
    ))
}

pub fn uninstall() -> Result<String> {
    if std::env::var_os("FLATPAK_ID").is_some() {
        bail!("Rimuovi il menu dall'host con scripts/install-flatpak-context-menu.py --remove.");
    }
    #[cfg(target_os = "windows")]
    windows::uninstall()?;
    #[cfg(not(target_os = "windows"))]
    {
        let files = native_files(&launcher_path()?)?;
        for file in &files {
            check_owned(&file.path)?;
        }
        for file in &files {
            if file.path.exists() {
                fs::remove_file(&file.path)?;
            }
        }
        #[cfg(target_os = "macos")]
        {
            let root = workflow_root()?;
            // Solo directory vuote: eventuali file aggiunti dall'utente restano intatti.
            let _ = fs::remove_dir(root.join("Contents/Resources"));
            let _ = fs::remove_dir(root.join("Contents"));
            let _ = fs::remove_dir(root);
        }
        refresh();
    }
    Ok("Menu contestuale rimosso per questo utente.".into())
}

pub fn installed() -> bool {
    if std::env::var_os("FLATPAK_ID").is_some() {
        return false;
    }
    #[cfg(target_os = "windows")]
    {
        windows::installed()
    }
    #[cfg(not(target_os = "windows"))]
    {
        launcher_path()
            .ok()
            .and_then(|exe| native_files(&exe).ok())
            .is_some_and(|files| {
                files.iter().all(|file| {
                    fs::read_to_string(&file.path).is_ok_and(|content| content.contains(OWNER))
                })
            })
    }
}

#[cfg_attr(target_os = "windows", allow(dead_code))]
struct FileSpec {
    path: PathBuf,
    content: String,
    executable: bool,
}

#[cfg(not(target_os = "windows"))]
fn check_owned(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_symlink() || !fs::read_to_string(path)?.contains(OWNER) =>
        {
            bail!(
                "Risorsa già esistente e non gestita dall'app: {}",
                path.display()
            );
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn write(file: &FileSpec) -> Result<()> {
    let parent = file
        .path
        .parent()
        .context("Percorso integrazione non valido")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(file.content.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(if file.executable {
                0o755
            } else {
                0o644
            }))?;
    }
    temp.as_file().sync_all()?;
    temp.persist(&file.path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn native_files(executable: &Path) -> Result<Vec<FileSpec>> {
    let dirs = BaseDirs::new().context("Directory utente non disponibile")?;
    #[cfg(target_os = "macos")]
    {
        mac_files(&dirs.home_dir().join("Library/Services"), executable)
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux_files(dirs.data_dir(), executable)
    }
}

#[cfg(target_os = "macos")]
fn workflow_root() -> Result<PathBuf> {
    Ok(BaseDirs::new()
        .context("Directory utente non disponibile")?
        .home_dir()
        .join("Library/Services/Invia a marzianetto.workflow"))
}

#[cfg(not(target_os = "windows"))]
fn refresh() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("/System/Library/CoreServices/pbs")
            .arg("-update")
            .output();
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(dirs) = BaseDirs::new() {
            let _ = std::process::Command::new("update-desktop-database")
                .arg(dirs.data_dir().join("applications"))
                .output();
        }
    }
}

#[cfg(target_os = "macos")]
fn remove_legacy_macos() -> Result<()> {
    let root = BaseDirs::new()
        .context("Directory utente non disponibile")?
        .home_dir()
        .join("Library/Services/Invia a Image Compressor.workflow");
    let workflow = root.join("Contents/Resources/document.wflow");
    if fs::read_to_string(workflow).is_ok_and(|content| content.contains(LEGACY_OWNER)) {
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_legacy_linux() -> Result<()> {
    let dirs = BaseDirs::new().context("Directory utente non disponibile")?;
    let data = dirs.data_dir();
    for path in [
        data.join("applications/it.imagecompressor.desktop"),
        data.join("kio/servicemenus/it.imagecompressor.desktop"),
        data.join("kservices5/ServiceMenus/it.imagecompressor.desktop"),
        data.join("nautilus/scripts/Invia a Image Compressor"),
        data.join("nemo/scripts/Invia a Image Compressor"),
    ] {
        if fs::read_to_string(&path).is_ok_and(|content| content.contains(LEGACY_OWNER)) {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "macos", test))]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "macos", test))]
fn mac_files(services: &Path, executable: &Path) -> Result<Vec<FileSpec>> {
    let exe = executable
        .to_str()
        .context("Il percorso dell'eseguibile deve essere Unicode")?;
    // L'azione non deve attendere la chiusura della GUI. I parametri sono passati
    // come argomenti, non interpolati nel codice shell, anche con spazi/apostrofi.
    let script = xml(&format!(
        "{} -- \"$@\" </dev/null >/dev/null 2>&1 &\n",
        shell_quote(exe)
    ));
    let root = services.join("Invia a marzianetto.workflow/Contents");
    let info = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>{LABEL}</string>
<key>CFBundleIdentifier</key><string>{OWNER}</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>NSServices</key><array><dict>
<key>NSMenuItem</key><dict><key>default</key><string>{LABEL}</string></dict>
<key>NSMessage</key><string>runWorkflowAsService</string>
<key>NSSendFileTypes</key><array><string>public.image</string><string>public.folder</string></array>
<key>NSRequiredContext</key><dict><key>NSApplicationIdentifier</key><string>com.apple.finder</string></dict>
</dict></array></dict></plist>
"#
    );
    let workflow = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>MarzianettoOwner</key><string>{OWNER}</string>
<key>AMDocumentVersion</key><string>2</string>
<key>actions</key><array><dict><key>action</key><dict>
<key>AMAccepts</key><dict><key>Container</key><string>List</string><key>Optional</key><true/><key>Types</key><array><string>com.apple.cocoa.string</string></array></dict>
<key>AMProvides</key><dict><key>Container</key><string>List</string><key>Types</key><array><string>com.apple.cocoa.string</string></array></dict>
<key>AMActionVersion</key><string>2.0.3</string>
<key>AMParameterProperties</key><dict/>
<key>ActionBundlePath</key><string>/System/Library/Automator/Run Shell Script.action</string>
<key>ActionName</key><string>Run Shell Script</string>
<key>ActionParameters</key><dict>
<key>COMMAND_STRING</key><string>{script}</string>
<key>CheckedForUserDefaultShell</key><true/>
<key>inputMethod</key><integer>1</integer>
<key>shell</key><string>/bin/zsh</string>
<key>source</key><string></string>
</dict>
<key>BundleIdentifier</key><string>com.apple.RunShellScript</string>
<key>Class Name</key><string>RunShellScriptAction</string>
<key>UUID</key><string>E8A654B7-310B-43D7-A52D-04E19B681410</string>
<key>InputUUID</key><string>157D9D60-BB04-46EB-B6FC-2A53ACFBFE92</string>
<key>OutputUUID</key><string>C5EE087C-4283-4B22-8CB2-801B824814A4</string>
<key>isViewVisible</key><true/>
</dict></dict></array>
<key>connectors</key><dict/>
<key>workflowMetaData</key><dict>
<key>serviceApplicationBundleID</key><string>com.apple.finder</string>
<key>serviceApplicationPath</key><string>/System/Library/CoreServices/Finder.app</string>
<key>serviceInputTypeIdentifier</key><string>com.apple.Automator.fileSystemObject</string>
<key>serviceOutputTypeIdentifier</key><string>com.apple.Automator.nothing</string>
<key>serviceProcessesInput</key><integer>0</integer>
<key>workflowTypeIdentifier</key><string>com.apple.Automator.servicesMenu</string>
</dict></dict></plist>
"#
    );
    Ok(vec![
        FileSpec {
            path: root.join("Info.plist"),
            content: info,
            executable: false,
        },
        FileSpec {
            path: root.join("Resources/document.wflow"),
            content: workflow,
            executable: false,
        },
    ])
}

#[cfg(any(target_os = "linux", test))]
fn desktop_exec(executable: &str) -> String {
    // Due livelli di escaping richiesti dalla Desktop Entry Specification:
    // quoting Exec, poi escaping del valore. Percentuali non sono field code.
    let quoted = executable
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    format!("\"{}\" -- %F", quoted.replace('\\', "\\\\"))
}

#[cfg(any(target_os = "linux", test))]
fn linux_files(data: &Path, executable: &Path) -> Result<Vec<FileSpec>> {
    let exe = executable
        .to_str()
        .context("Il percorso dell'eseguibile deve essere Unicode")?;
    if exe.contains(['\n', '\r']) {
        bail!("Percorso eseguibile non supportato per l'integrazione Linux");
    }
    let exec = desktop_exec(exe);
    let application = format!("# {OWNER}\n[Desktop Entry]\nType=Application\nName=marzianetto\nComment=Comprimi immagini e cartelle\nExec={exec}\nIcon=image-x-generic\nTerminal=false\nCategories=Graphics;\nMimeType=image/jpeg;image/png;image/webp;image/bmp;image/gif;image/tiff;inode/directory;\n");
    let dolphin = format!("# {OWNER}\n[Desktop Entry]\nType=Service\nMimeType=image/jpeg;image/png;image/webp;image/bmp;image/gif;image/tiff;inode/directory;\nX-KDE-ServiceTypes=KonqPopupMenu/Plugin\nActions=compress;\n\n[Desktop Action compress]\nName={LABEL}\nIcon=image-x-generic\nExec={exec}\n");
    // URIs invece di nomi separati da newline: funzionano anche percorsi con
    // newline, Unicode e metacaratteri. Nessun interprete shell per i parametri.
    let script = format!(
        r#"#!/usr/bin/env python3
# {OWNER}
import os
import subprocess
import sys
from urllib.parse import urlsplit, unquote

uris = os.environ.get('NAUTILUS_SCRIPT_SELECTED_URIS') or os.environ.get('NEMO_SCRIPT_SELECTED_URIS', '')
paths = []
for uri in uris.splitlines():
    parsed = urlsplit(uri)
    if parsed.scheme == 'file' and parsed.netloc in ('', 'localhost'):
        paths.append(unquote(parsed.path))
if not paths:
    paths = sys.argv[1:]
if paths:
    subprocess.Popen([{exe}, '--', *paths], start_new_session=True)
"#,
        exe = serde_json::to_string(exe)?
    );
    Ok(vec![
        FileSpec {
            path: data.join("applications/it.marzianetto.app.desktop"),
            content: application,
            executable: false,
        },
        FileSpec {
            path: data.join("kio/servicemenus/it.marzianetto.app.desktop"),
            content: dolphin.clone(),
            executable: true,
        },
        FileSpec {
            path: data.join("kservices5/ServiceMenus/it.marzianetto.app.desktop"),
            content: dolphin,
            executable: true,
        },
        FileSpec {
            path: data.join("nautilus/scripts/Invia a marzianetto"),
            content: script.clone(),
            executable: true,
        },
        FileSpec {
            path: data.join("nemo/scripts/Invia a marzianetto"),
            content: script,
            executable: true,
        },
    ])
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    fn command() -> Command {
        let mut command = Command::new("reg.exe");
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
    }

    fn keys() -> Vec<String> {
        let mut keys = vec![r"HKCU\Software\Classes\Directory\shell\Marzianetto".to_owned()];
        for extension in ["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"] {
            keys.push(format!(
                r"HKCU\Software\Classes\SystemFileAssociations\.{extension}\shell\Marzianetto"
            ));
        }
        keys
    }

    fn reg(args: &[&str]) -> Result<()> {
        let output = command().args(args).output()?;
        if !output.status.success() {
            bail!(
                "Registro di Windows: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    fn owned(key: &str) -> bool {
        ["MarzianettoOwner", "ImageCompressorOwner"]
            .iter()
            .any(|value| {
                command()
                    .args(["query", key, "/v", value])
                    .output()
                    .is_ok_and(|output| {
                        if !output.status.success() {
                            return false;
                        }
                        let contents = String::from_utf8_lossy(&output.stdout);
                        contents.contains(OWNER) || contents.contains(LEGACY_OWNER)
                    })
            })
    }

    fn check(key: &str) -> Result<()> {
        let exists = command().args(["query", key]).output()?.status.success();
        if exists && !owned(key) {
            bail!("Chiave già esistente e non gestita dall'app: {key}");
        }
        Ok(())
    }

    pub fn installed() -> bool {
        keys().iter().all(|key| owned(key))
    }

    pub fn install(executable: &Path) -> Result<()> {
        let exe = executable
            .to_str()
            .context("Percorso eseguibile non valido")?;
        let command = format!("\"{exe}\" -- \"%1\"");
        let keys = keys();
        for key in &keys {
            check(key)?;
        }
        for key in &keys {
            reg(&[
                "add",
                key,
                "/v",
                "MarzianettoOwner",
                "/t",
                "REG_SZ",
                "/d",
                OWNER,
                "/f",
            ])?;
            reg(&["add", key, "/ve", "/t", "REG_SZ", "/d", LABEL, "/f"])?;
            reg(&[
                "add",
                key,
                "/v",
                "Icon",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{exe}\",0"),
                "/f",
            ])?;
            reg(&[
                "add",
                key,
                "/v",
                "MultiSelectModel",
                "/t",
                "REG_SZ",
                "/d",
                "Single",
                "/f",
            ])?;
            reg(&[
                "add",
                &format!(r"{key}\command"),
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                &command,
                "/f",
            ])?;
        }
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let keys = keys();
        for key in &keys {
            check(key)?;
        }
        for key in &keys {
            if owned(key) {
                reg(&["delete", key, "/f"])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_script_quotes_executable_and_passes_arguments() {
        let files = mac_files(
            Path::new("/services"),
            Path::new("/Applications/it's & nice/app"),
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[1].content.contains("inputMethod</key><integer>1"));
        assert!(files[1].content.contains("&quot;$@&quot;"));
        assert!(files.iter().all(|file| file.content.contains(OWNER)));
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn linux_generates_file_manager_entries_without_changing_defaults() {
        let files = linux_files(Path::new("/data"), Path::new("/opt/My App/compress")).unwrap();
        assert_eq!(files.len(), 5);
        assert!(files.iter().all(|file| file.content.contains(OWNER)));
        assert!(files[0]
            .content
            .contains("Exec=\"/opt/My App/compress\" -- %F"));
        assert!(files[3].content.contains("subprocess.Popen"));
        assert!(!files
            .iter()
            .any(|file| file.path.ends_with("mimeapps.list")));
    }

    #[test]
    fn desktop_percent_is_escaped() {
        assert_eq!(desktop_exec("/opt/100%/app"), "\"/opt/100%%/app\" -- %F");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unrelated_integration_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing");
        fs::write(&path, "user content").unwrap();
        assert!(check_owned(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "user content");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_workflow_is_valid_plist() {
        let dir = tempfile::tempdir().unwrap();
        let files = mac_files(
            dir.path(),
            Path::new("/Applications/Example & Test/compressor"),
        )
        .unwrap();
        for file in files {
            write(&file).unwrap();
            let output = std::process::Command::new("/usr/bin/plutil")
                .args(["-lint", "--"])
                .arg(file.path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn automator_passes_selected_path_as_argument() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("test app's launcher");
        let log = dir.path().join("arguments.txt");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n",
                shell_quote(log.to_str().unwrap())
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let selected = dir.path().join("foto con spazi & apostrofo'.png");
        fs::write(&selected, "test").unwrap();
        let mut files = mac_files(dir.path(), &executable).unwrap();
        // Solo per il test, attendiamo il launcher di prova anziché lasciarlo
        // in background, così il controllo degli argomenti è deterministico.
        files[1].content = files[1].content.replace(" &amp;\n", "\n");
        for file in &files {
            write(file).unwrap();
        }
        let workflow = dir.path().join("Invia a marzianetto.workflow");
        let output = std::process::Command::new("/usr/bin/automator")
            .arg("-i")
            .arg(&selected)
            .arg(workflow)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(log).unwrap(),
            format!("--\n{}\n", selected.display())
        );
    }
}
