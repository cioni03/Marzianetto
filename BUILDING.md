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

Produce anche `marzianetto-<target>.app.zip` per gli aggiornamenti in-app. Lo ZIP non sostituisce il DMG per l'installazione manuale.

Per scegliere il target esplicitamente:

```sh
bash scripts/package-macos.sh aarch64-apple-darwin
bash scripts/package-macos.sh x86_64-apple-darwin
```

Per Intel su Apple Silicon servono le librerie/SDK x86_64 di Xcode. Meglio testare ciascun artefatto sul relativo hardware o con Rosetta. Questi comandi producono **due app separate**, non una universal binary; una universal richiede compilare entrambi i target e unirli con `lipo`, poi rifirmare il bundle.

Apri il DMG e copia `marzianetto.app` in **Applicazioni**. Avvia la copia installata e reinstalla il menu contestuale dalla sua UI, così non rimarrà collegato all'eseguibile di `target/` o al DMG.

### Aggiornamenti in-app

All'avvio la `.app` installata cerca su GitHub una release stabile più recente con lo ZIP della stessa architettura e un digest SHA-256. **Aggiorna** scarica e verifica lo ZIP, poi chiude, sostituisce e riavvia l'app. **Salta questa versione** ricorda solo il tag selezionato. L'aggiornamento richiede una cartella scrivibile, non chiede privilegi elevati e non cambia le regole di Gatekeeper. Windows, Linux e Flatpak non usano questo updater.

Per pubblicare un aggiornamento, avvia il workflow manuale descritto sotto e inserisci una versione nuova, ad esempio `0.1.1`. Non servono token sul client o chiavi dedicate all'updater: la fiducia si basa su GitHub Releases e il digest verifica l'integrità del download.

### Distribuzione pubblica e Gatekeeper

Lo script applica una **firma ad hoc**, adatta al collaudo locale ma non sufficiente per distribuire un'app riconosciuta da Gatekeeper. Per una distribuzione pubblica servono Apple Developer Program, certificato **Developer ID Application**, firma con hardened runtime e notarizzazione. Dopo avere configurato un profilo `notarytool` nel Portachiavi:

```sh
APP="dist/macos/aarch64-apple-darwin/marzianetto.app"
DMG="dist/macos/aarch64-apple-darwin/marzianetto-aarch64-apple-darwin.dmg"
codesign --force --deep --options runtime --timestamp --sign "Developer ID Application: NOME (TEAMID)" "$APP"
STAGE="$(mktemp -d)"
ditto "$APP" "$STAGE/marzianetto.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "marzianetto" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
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

## Linux: AppImage

Il workflow compila su Ubuntu 22.04 e produce `marzianetto-linux-x86_64.AppImage`, includendo le librerie distribuibili tramite linuxdeploy. Per crearlo manualmente su Linux x64, installa le dipendenze della build nativa più `curl`, `file`, `patchelf` e `desktop-file-utils`, poi esegui `bash scripts/package-linux.sh`. Lo script scarica linuxdeploy dalla sua release `continuous` e richiede accesso alla rete.

Per avviare il download: `chmod +x marzianetto-linux-x86_64.AppImage`, poi apri il file. Se FUSE non è disponibile, usa `./marzianetto-linux-x86_64.AppImage --appimage-extract-and-run`. Servono comunque driver grafici e, per il selettore file, un portale desktop funzionante. Non è una garanzia di compatibilità con ogni distribuzione: la build richiede glibc 2.35 o successiva e non è destinata ad Alpine/musl o Linux ARM64. Sposta il file nella posizione definitiva prima di installare il menu contestuale dall'app.

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

## Build manuale e release automatica con GitHub Actions

Il workflow [`.github/workflows/build.yml`](.github/workflows/build.yml) compila i pacchetti direttamente sui runner GitHub. **Non devi caricare la tua `.app`, `target/`, `dist/` o le crate vendorizzate**: bastano sorgenti, `Cargo.toml`, `Cargo.lock`, script, asset e workflow. `.gitignore` esclude già i file generati.

Vai in **Actions → Build desktop packages → Run workflow**, seleziona `main`, inserisci **version** (ad esempio `0.1.1`) e avvia. I push, i tag e le pull request non avviano la build. La versione viene applicata a `Cargo.toml` e `Cargo.lock` solo sui runner: non devi modificarli, fare un commit di versione o creare un tag a mano. Usa una versione stabile più recente di quelle pubblicate.

| Job | Artefatto |
| --- | --- |
| macOS | `.dmg` e `.app.zip` per l'architettura del runner `macos-latest` |
| Windows | ZIP portabile con `.exe` x64 e documentazione |
| Linux | `.AppImage` x64 |

Trovi i file anche nella sezione **Artifacts** dell'esecuzione, per 30 giorni. Se tutte e tre le build riescono, il workflow carica i pacchetti in una bozza e poi pubblica automaticamente **marzianetto vVERSIONE** come ultima release. GitHub crea il tag sul commit compilato; non vengono sovrascritte release esistenti. Al prossimo avvio, le app macOS compatibili che includono già l'updater propongono il nuovo aggiornamento.

La versione macOS usa firma ad hoc: **non** è notarizzata e non è una universal binary Intel + Apple Silicon. Le build Windows non sono firmate Authenticode. Per firmare pubblicamente servono certificati/credenziali e un workflow dedicato; non inserirli nei sorgenti. Controlla inoltre disponibilità dei runner, impostazioni Actions del repository e quote/costi del tuo piano GitHub, soprattutto per repository privati.

Windows e Linux riceveranno il primo collaudo completo al primo avvio della Action. La sola aggiunta del file YAML non dimostra che le build remote siano già riuscite: eventuali errori saranno visibili nei log di ciascun job.

Sotto il nome dell'app compare solo la versione `X.Y.Z`, incorporata nella build e corrispondente al tag della release. Le build locali usano la versione di `Cargo.toml`. Il workflow distribuisce Linux come AppImage, non come bundle Flatpak; per Flatpak segui la procedura manuale descritta sopra. Il DMG macOS contiene solo l'app e il collegamento ad Applicazioni, senza documentazione aggiuntiva.

## Stato delle verifiche

Sul Mac di sviluppo sono verificati i test Rust, Clippy e la creazione di **`.app` e `.dmg` Apple Silicon** con lo script macOS. Il registratore del menu Flatpak ha test isolati eseguibili con `python3 -m unittest discover -s scripts/tests -v`, senza alterare il menu reale.

Gli script Windows e il manifest Flatpak devono essere collaudati sui rispettivi sistemi: la loro presenza non significa che qui siano stati prodotti o testati artefatti Windows/Linux. Anche la build macOS Intel resta da collaudare. La firma pubblica/notarizzazione richiede le tue credenziali e non è automatizzata.
