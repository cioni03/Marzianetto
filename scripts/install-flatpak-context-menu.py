#!/usr/bin/env python3
"""Eseguire sull'host Linux, NON dentro la sandbox Flatpak. Nessun sudo."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

APP = "it.marzianetto.app"
OWNER = "it.marzianetto.context-menu.flatpak.v1"
LABEL = "Invia a marzianetto (Flatpak)"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--remove", action="store_true", help="Rimuovi soltanto le voci installate da questo script")
    args = parser.parse_args()
    if not sys.platform.startswith("linux") or os.environ.get("FLATPAK_ID"):
        parser.error("Esegui questo script sull'host Linux.")
    if not args.remove:
        if not shutil.which("flatpak"):
            parser.error("Installa Flatpak prima di registrare il menu.")
        subprocess.run(["flatpak", "info", APP], check=True, stdout=subprocess.DEVNULL)
    data = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share")))
    desktop = f"""# {OWNER}
[Desktop Entry]
Type=Service
MimeType=image/jpeg;image/png;image/webp;image/bmp;image/gif;image/tiff;inode/directory;
X-KDE-ServiceTypes=KonqPopupMenu/Plugin
Actions=compress;

[Desktop Action compress]
Name={LABEL}
Icon={APP}
Exec=flatpak run {APP} -- %F
"""
    script = f'''#!/usr/bin/env python3
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
    subprocess.Popen(['flatpak', 'run', '{APP}', '--', *paths], start_new_session=True)
'''
    files = {
        data / f"kio/servicemenus/{APP}.flatpak.desktop": desktop,
        data / f"kservices5/ServiceMenus/{APP}.flatpak.desktop": desktop,
        data / f"nautilus/scripts/{LABEL}": script,
        data / f"nemo/scripts/{LABEL}": script,
    }
    for path in files:
        if path.is_symlink() or (path.exists() and OWNER not in path.read_text()):
            raise RuntimeError(f"Risorsa non gestita da questo script: {path}")
    for path, content in files.items():
        if args.remove:
            path.unlink(missing_ok=True)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
            path.chmod(0o755)
    print("Menu rimosso." if args.remove else "Menu installato. Riapri il file manager se non appare.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
