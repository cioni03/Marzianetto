# Build e distribuzione

Il percorso più semplice è **compilare su ciascun sistema** (tre macchine, VM o runner CI). Da macOS non basta `cargo build --target ...` per produrre automaticamente una build Windows o Linux: servono linker, SDK e librerie della piattaforma di destinazione. **Flatpak si costruisce su Linux**, anche in una VM Linux su Mac.

Tutti i comandi partono dalla radice del progetto. Conserva `Cargo.lock`; gli script usano `--locked`. Dopo modifiche al codice o alle dipendenze, rigenera i sorgenti vendorizzati per Flatpak.

## macOS: `.app` e `.dmg`

Prerequisiti: Rust stabile, Python 3 (3.8+), Xcode Command Line Tools.

```sh
xcode-select --install
cargo test
bash scripts/package-macos.sh
```

Lo script produce una app e un DMG in `dist/macos/<target>/` per l'architettura del Mac:

- Apple Silicon: `aarch64-apple-darwin`.
- Intel: `x86_64-apple-darwin`.

Per scegliere il target esplicitamente:

```sh
bash scripts/package-macos.sh aarch64-apple-darwin
bash scripts/package-macos.sh x86_64-apple-darwin
```

Per Intel su Apple Silicon servono le librerie/SDK x86_64 di Xcode. Meglio testare ciascun artefatto sul relativo hardware o con Rosetta. Questi comandi producono **due app separate**, non una universal binary; una universal richiede compilare entrambi i target e unirli con `lipo`, poi rifirmare il bundle.

Apri il DMG e copia `marzianetto.app` in **Applicazioni**. Avvia la copia installata e reinstalla il menu contestuale dalla sua UI, così non rimarrà collegato all'eseguibile di `target/` o al DMG.

### Distribuzione pubblica e Gatekeeper

Lo script applica una **firma ad hoc**, adatta al collaudo locale ma non sufficiente per distribuire un'app riconosciuta da Gatekeeper. Per una distribuzione pubblica servono Apple Developer Program, certificato **Developer ID Application**, firma con hardened runtime e notarizzazione. Dopo avere configurato un profilo `notarytool` nel Portachiavi:

```sh
APP="dist/macos/aarch64-apple-darwin/marzianetto.app"
DMG="dist/macos/aarch64-apple-darwin/marzianetto-aarch64-apple-darwin.dmg"
codesign --force --deep --options runtime --timestamp --sign "Developer ID Application: NOME (TEAMID)" "$APP"
ditto "$APP" "dist/macos/aarch64-apple-darwin/dmg-root/marzianetto.app"
hdiutil create -volname "marzianetto" -srcfolder "dist/macos/aarch64-apple-darwin/dmg-root" -ov -format UDZO "$DMG"
xcrun notarytool submit "$DMG" --keychain-profile "PROFILO_NOTARIZZAZIONE" --wait
xcrun stapler staple "$DMG"
```

Sostituisci architettura, nome del certificato e profilo con i tuoi valori reali. Non inserire credenziali o certificati privati nel repository.

## Windows: `.exe` e ZIP portabile

Prerequisiti: Rust stabile **MSVC**, Visual Studio Build Tools con **Desktop development with C++** e Windows SDK. Per ARM64 installa anche gli strumenti C++ ARM64.

Da PowerShell:

```powershell
cargo test
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
```

Output: `dist/windows/x86_64-pc-windows-msvc/marzianetto.exe` e ZIP con l'eseguibile e la documentazione. Per ARM64:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1 -Target aarch64-pc-windows-msvc
```

L'EXE è portabile; spostalo nella cartella definitiva prima di installare il menu contestuale dalla UI. Su Windows 11 la voce è in **Mostra altre opzioni**. Lo script non crea un installer MSI/Setup. Per un installer puoi aggiungere Inno Setup o WiX; per ridurre gli avvisi SmartScreen serve la firma Authenticode con un certificato appropriato, non basta compilare in release. Verifica la disponibilità del runtime Microsoft Visual C++ sui PC di destinazione.

## Linux: build nativa

Su Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libx11-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev xdg-desktop-portal xdg-desktop-portal-gtk
cargo build --locked --release
```

L'eseguibile nativo è `target/release/marzianetto`. Per distribuire un binario nativo, compila su una distribuzione con glibc non più recente di quelle che vuoi supportare, oppure usa Flatpak.

## Linux: bundle `.flatpak`

Prerequisiti su Linux: Flatpak, flatpak-builder, Rust/Cargo per preparare le dipendenze. Su Debian/Ubuntu:

```sh
sudo apt install flatpak flatpak-builder build-essential pkg-config
flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user flathub org.freedesktop.Platform//25.08 org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08
bash scripts/prepare-flatpak.sh
flatpak-builder --user --force-clean --repo=repo-flatpak build-flatpak packaging/flatpak/it.marzianetto.app.yml
mkdir -p dist/linux
flatpak build-bundle repo-flatpak dist/linux/marzianetto.flatpak it.marzianetto.app --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
```

`prepare-flatpak.sh` scarica e vendorizza le crate in `packaging/flatpak/sources/`. Il manifest usa il Rust SDK extension e compila **offline**: non concede rete alla fase di build né all'app installata. I sorgenti generati, il repository locale e gli artefatti non vanno aggiunti a Git.

Installa e avvia il bundle:

```sh
flatpak install --user dist/linux/marzianetto.flatpak
flatpak run it.marzianetto.app
```

Su un altro PC Linux copia il `.flatpak`; servono comunque Flatpak, il remote Flathub e il runtime Freedesktop, che non sono inclusi nel bundle. Per ARM64/aarch64 costruisci il bundle su un sistema/runner Linux ARM64 con i runtime della stessa architettura. Questo è un **bundle locale**, non una pubblicazione su Flathub: per Flathub servono repository dei sorgenti, URL/versioni/checksum, AppStream completo e revisione del manifest.

### Permessi e menu contestuale con Flatpak

Il manifest concede **`--filesystem=host`** perché l'app deve scansionare cartelle e salvare copie o sostituire originali in percorsi arbitrari. È un permesso ampio, non una sandbox con accesso limitato ai soli file scelti. Non viene concessa rete. Il selettore file usa XDG Desktop Portal; i drag-and-drop di cartelle dipendono anche dal compositor/file manager.

Il menu dell'host non va installato dall'interno della sandbox (l'eseguibile `/app/bin/...` non è avviabile dall'host). Dopo l'installazione Flatpak esegui **sull'host Linux**:

```sh
python3 scripts/install-flatpak-context-menu.py
```

Questo aggiunge le voci per Dolphin, Nautilus e Nemo che invocano `flatpak run`. Per rimuoverle:

```sh
python3 scripts/install-flatpak-context-menu.py --remove
```

Negli altri file manager usa **Apri con → marzianetto**. Gli script richiedono Python 3 (3.8+). Le preferenze Flatpak sono isolate sotto `~/.var/app/it.marzianetto.app/`.

## Build automatiche con GitHub Actions

Il workflow [`.github/workflows/build.yml`](.github/workflows/build.yml) compila i pacchetti direttamente sui runner GitHub. **Non devi caricare la tua `.app`, `target/`, `dist/` o le crate vendorizzate**: bastano sorgenti, `Cargo.toml`, `Cargo.lock`, script, asset e workflow. `.gitignore` esclude già i file generati.

Parte automaticamente su push a `main`/`master`, tag `v*` e pull request verso questi branch. Puoi anche avviarlo manualmente da **Actions → Build desktop packages → Run workflow**. I tre job sono indipendenti:

| Job | Artefatto |
| --- | --- |
| macOS | `.dmg` contenente l'app, per l'architettura effettiva del runner `macos-latest` |
| Windows | ZIP portabile con `.exe` x64 e documentazione |
| Linux | `.flatpak` x64, documentazione e script per il menu contestuale dell'host |

Trovi i download nella pagina dell'esecuzione, sezione **Artifacts**. GitHub li conserva per 30 giorni (entro gli eventuali limiti del repository). Le build **non vengono aggiunte ai commit e non vengono pubblicate automaticamente come GitHub Release**. Il workflow richiede solo permesso di lettura al repository e non necessita di secret personali.

La versione macOS usa firma ad hoc: **non** è notarizzata e non è una universal binary Intel + Apple Silicon. Le build Windows non sono firmate Authenticode. Per firmare pubblicamente servono certificati/credenziali e un workflow dedicato; non inserirli nei sorgenti. Controlla inoltre disponibilità dei runner, impostazioni Actions del repository e quote/costi del tuo piano GitHub, soprattutto per repository privati.

Windows e Linux riceveranno il primo collaudo completo al primo avvio della Action. La sola aggiunta del file YAML non dimostra che le build remote siano già riuscite: eventuali errori saranno visibili nei log di ciascun job.

## Stato delle verifiche

Sul Mac di sviluppo sono verificati i test Rust, Clippy e la creazione di **`.app` e `.dmg` Apple Silicon** con lo script macOS. Il registratore del menu Flatpak ha test isolati eseguibili con `python3 -m unittest discover -s scripts/tests -v`, senza alterare il menu reale.

Gli script Windows e il manifest Flatpak devono essere collaudati sui rispettivi sistemi: la loro presenza non significa che qui siano stati prodotti o testati artefatti Windows/Linux. Anche la build macOS Intel resta da collaudare. La firma pubblica/notarizzazione richiede le tue credenziali e non è automatizzata.
