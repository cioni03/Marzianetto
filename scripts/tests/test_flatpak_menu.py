"""Test del registratore host senza modificare il menu reale del sistema."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "install-flatpak-context-menu.py"
SPEC = importlib.util.spec_from_file_location("flatpak_menu", SCRIPT)
MENU = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MENU)


class MenuTests(unittest.TestCase):
    def call(self, data, remove=False):
        args = [str(SCRIPT)] + (["--remove"] if remove else [])
        with patch.object(MENU.sys, "platform", "linux"), \
                patch.object(MENU.sys, "argv", args), \
                patch.dict(os.environ, {"XDG_DATA_HOME": str(data)}, clear=True), \
                patch.object(MENU.shutil, "which", return_value="/usr/bin/flatpak"), \
                patch.object(MENU.subprocess, "run"):
            MENU.main()

    def test_install_remove_and_reinstall(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            self.call(data)
            files = [path for path in data.rglob("*") if path.is_file()]
            self.assertEqual(len(files), 4)
            for path in files:
                self.assertIn(MENU.OWNER, path.read_text())
                self.assertTrue(path.stat().st_mode & 0o111)
            self.call(data)
            self.call(data, remove=True)
            self.assertFalse(any(path.exists() for path in files))

    def test_user_file_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            data = Path(directory)
            path = data / "nemo/scripts" / MENU.LABEL
            path.parent.mkdir(parents=True)
            path.write_text("user work")
            with self.assertRaises(RuntimeError):
                self.call(data)
            self.assertEqual(path.read_text(), "user work")
            self.assertEqual(len([file for file in data.rglob("*") if file.is_file()]), 1)


if __name__ == "__main__":
    unittest.main()
