# marzianetto

Applicazione desktop Rust per Windows, macOS e Linux, con UI italiana `eframe/egui`.

English: see [README.en.md](README.en.md).

## Avvio

Installa [Rust](https://rustup.rs/) stabile e gli strumenti di compilazione della piattaforma, quindi:

```sh
cargo run --release
```

Per compilare e verificare:

```sh
cargo build --release
cargo test
```

L'eseguibile è `target/release/marzianetto` (`marzianetto.exe` su Windows).
La prima compilazione scarica le dipendenze e può richiedere alcuni minuti.

### Dipendenze di sistema

- **macOS:** Command Line Tools (`xcode-select --install`).
- **Windows:** Rust MSVC e Visual Studio Build Tools con il workload C++ e Windows SDK.
- **Linux (Debian/Ubuntu):** `build-essential pkg-config libx11-dev libxrandr-dev libxi-dev libxcursor-dev libxinerama-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev`. Il selettore file usa XDG Desktop Portal; installa `xdg-desktop-portal` e il backend adatto al desktop (per esempio `xdg-desktop-portal-gtk`).

WebP compila `libwebp` tramite la crate; non richiede un'installazione separata. AVIF usa l'encoder Rust `ravif` senza una dipendenza di sistema da libavif. Per creare **`.app`/`.dmg` macOS, `.exe`/ZIP Windows o `.flatpak` Linux**, trovi script e istruzioni in [BUILDING.md](BUILDING.md).

**Build automatiche:** il workflow `.github/workflows/build.yml` produce i tre pacchetti su GitHub Actions caricando solo i sorgenti. I download sono nella sezione **Artifacts** dell'esecuzione; nessun binario va committato. Dettagli in [BUILDING.md](BUILDING.md#build-automatiche-con-github-actions).

## Utilizzo

1. Aggiungi immagini oppure trascina file/cartelle. Per le cartelle scegli se includere le sottocartelle.
2. Imposta formato, qualità, eventuale ridimensionamento sul **lato lungo o lato corto** e destinazione.
3. Avvia la compressione. L'importazione e la codifica avvengono in background.
4. Passa sulla miniatura per vedere un'anteprima più grande, dimensioni e numero esatto di byte. La tabella mostra risultati, percorsi output e dettagli degli errori tramite hover.

### Formati e qualità

| Formato in uscita | Comportamento dello slider |
| --- | --- |
| JPEG | Qualità lossy 1–100; alpha composto su bianco |
| WebP | Qualità lossy 0–100; conserva alpha |
| AVIF | Qualità lossy 1–100; conserva alpha |
| PNG | Lossless: 0–33 veloce, 34–66 normale, 67–100 massima compressione |
| BMP / TIFF | Slider disabilitato; nessuna garanzia di riduzione del peso |

Input supportati: JPEG, PNG, WebP, BMP, GIF, TIFF. **AVIF è supportato solo in uscita** in questa configurazione per evitare dipendenze native di decodifica. GIF animate e TIFF multipagina vengono convertiti usando il primo frame/pagina; la sostituzione è bloccata per entrambi per evitare perdita di contenuti. L'orientamento EXIF viene applicato ai pixel prima del ridimensionamento e della conversione, anche nelle anteprime.

Il ridimensionamento mantiene le proporzioni (arrotondamento al pixel più vicino) e non ingrandisce. Con **lato lungo 1000 px**, una foto 3000×2000 diventa 1000×667; con **lato corto 1000 px**, diventa 1500×1000. Se il lato scelto è già sotto il limite, le dimensioni restano invariate. La scelta del lato è persistente e le vecchie impostazioni rimangono compatibili.

### Privacy e metadati

**Rimuovi tutti i metadati** è attivo per default e salvato nelle preferenze. La ricodifica non copia EXIF (inclusi GPS, dati fotocamera e miniature incorporate), ICC, XMP, commenti o altri metadati descrittivi. Restano naturalmente le informazioni strutturali indispensabili al formato (dimensioni, codifica, canali, ecc.). Non elimina i metadati dagli originali conservati nelle modalità di copia e non è una garanzia di anonimizzazione del contenuto visivo.

Se lo disattivi, la conservazione è **parziale**, secondo gli encoder disponibili:

| Output | Metadati conservati, se presenti |
| --- | --- |
| JPEG / PNG | EXIF e ICC |
| AVIF | EXIF |
| TIFF | ICC |
| WebP / BMP | Nessuno con gli encoder attuali |

L'orientamento EXIF reinserito viene normalizzato per evitare doppie rotazioni. I metadati non supportati producono un avviso sul singolo risultato. XMP e metadati arbitrari non sono garantiti; i campi EXIF dimensionali e le miniature conservate potrebbero descrivere ancora l'originale dopo un ridimensionamento. Rimuovere ICC può cambiare la resa dei colori: l'app non converte preventivamente tutti i profili in sRGB. Non usare questa conversione come archivio fotografico fedele o pipeline professionale con gestione colore.

## Menu contestuale del sistema

Nella barra superiore apri **Menu di sistema → Installa menu**. L'installazione riguarda solo l'utente corrente e non cambia le associazioni predefinite. Puoi reinstallare o rimuovere l'integrazione dalla stessa finestra.

**Versione Flatpak:** installa il menu dall'host Linux con `python3 scripts/install-flatpak-context-menu.py`, non dalla sandbox. Vedi [BUILDING.md](BUILDING.md).

- **macOS:** installa l'Azione rapida `Invia a marzianetto` in `~/Library/Services`. Nel Finder seleziona un'immagine o una cartella, clic destro → **Azioni rapide** oppure **Servizi**. Se non appare, abilitala in Impostazioni di Sistema → Tastiera → Abbreviazioni → Servizi (o nella gestione delle estensioni Finder, secondo la versione di macOS). Potrebbe essere necessario riaprire Finder.
- **Windows:** aggiunge la voce **Invia a marzianetto** a Explorer per cartelle e immagini supportate, nel registro `HKCU\Software\Classes`. Su Windows 11 si trova in **Mostra altre opzioni**. La voce gestisce un elemento selezionato per volta; non installa un'estensione COM per il nuovo menu compatto.
- **Linux:** installa una voce **Azioni** per Dolphin (KDE 5/6), script per **Nautilus/Nemo** e un launcher `.desktop` per **Apri con**. Gli script richiedono **Python 3** e usano gli URI della selezione per gestire in modo sicuro spazi e caratteri speciali. Le risorse seguono `$XDG_DATA_HOME` (default `~/.local/share`). Riapri il file manager o abilita il servizio in Dolphin se necessario. Thunar e altri file manager possono usare **Apri con → marzianetto**, ma non ricevono un'azione personalizzata specifica: Linux non offre un unico menu contestuale condiviso.

Il programma si apre e carica la selezione, senza avviare automaticamente la compressione. Le cartelle richiedono comunque la scelta sulle sottocartelle. Ogni invio avvia una nuova finestra/processo; non è implementato l'inoltro a un'istanza già aperta.

Puoi anche usare il terminale:

```sh
./target/release/marzianetto -- "/percorso/foto.png" "/percorso/cartella"
./target/release/marzianetto --install-context-menu
./target/release/marzianetto --remove-context-menu
```

Su Windows usa il percorso dell'eseguibile `.exe`. **Installa dopo `cargo build --release`**, non dalla build temporanea di sviluppo: il menu memorizza il percorso dell'eseguibile. Se sposti o elimini il programma, reinstalla/rimuovi il menu. L'installer non sovrascrive risorse non marcate come proprie e il disinstallatore rimuove solo le risorse di questa integrazione.

### Salvataggio sicuro

- **Altra cartella:** per `/foto/viaggio/a.png` importato dalla radice `/foto`, crea `/foto_compressed/viaggio/a_compressed.jpg`. La radice è quella della cartella trascinata; per un singolo file è la sua cartella genitore.
- **Stessa cartella:** crea copie accanto agli originali con il suffisso.
- **Sostituzione:** richiede conferma. Con lo stesso codec, sostituisce atomicamente il file originale. Cambiando codec, salva prima il nuovo file e solo dopo elimina l'originale. Il suffisso non si applica in questa modalità. File di destinazione preesistenti non vengono sovrascritti.
- Le collisioni ricevono `_1`, `_2`, ecc. I file temporanei sono scritti nella cartella di destinazione e sincronizzati prima della pubblicazione. Gli originali modificati dopo l'importazione vengono rifiutati.
- Dopo una sostituzione, svuota e ricarica la lista per un nuovo ciclo. Mantieni sempre un backup: la sostituzione non è reversibile e può anche aumentare la dimensione.
- L'interruzione termina **dopo il file in corso**. La chiusura della finestra durante la compressione chiede se continuare o interrompere e chiudere al termine del file. La terminazione forzata del processo non può essere protetta. La persistenza atomica non equivale a una garanzia contro ogni guasto hardware o modifica concorrente esterna.

Le statistiche comprendono solo i file riusciti e mostrano anche incrementi di peso come risparmio negativo. Nelle modalità di copia, il risparmio è potenziale: tenere originali e copie aumenta l'occupazione totale su disco.

### Impostazioni persistenti

JSON locale, salvato automaticamente con breve debounce e scrittura atomica nella directory di configurazione determinata da `directories::ProjectDirs`:

- Windows: directory di configurazione sotto `%APPDATA%`.
- macOS: `~/Library/Application Support/it.Marzianetto.marzianetto/settings.json`.
- Linux: `$XDG_CONFIG_HOME/marzianetto/settings.json` (default `~/.config/marzianetto/settings.json`).

Il percorso esatto è visibile passando il mouse su “Percorso impostazioni”. Impostazioni non valide vengono segnalate senza sovrascrivere subito il file esistente.

## Struttura

- `src/main.rs`: avvio e finestra nativa.
- `src/ui.rs`: UI, miniature, dialoghi, progresso e riepilogo.
- `src/theme.rs`: palette, tipografia, spaziature e componenti visivi condivisi.
- `src/integration.rs`: menu contestuali nativi per utente e installazione/rimozione.
- `src/importer.rs`: scansione cartelle, deduplicazione e decodifica in background.
- `src/compressor.rs`: encoder, ridimensionamento e scritture protette.
- `src/metadata.rs`: gestione EXIF/ICC, normalizzazione dell'orientamento e rimozione dei metadati.
- `src/settings.rs`: configurazione, validazione e persistenza.
- `src/model.rs`: modelli e formattazione delle statistiche.

I test coprono codec, ridimensionamento, collisioni, percorsi, sostituzione e validazione. La compilazione sulle altre piattaforme va verificata sui rispettivi sistemi.
