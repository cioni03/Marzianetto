"""Set the crate version in a CI checkout without updating any dependencies."""

import os
from pathlib import Path
import re


def replace_version(text, section, name, version):
    header = re.compile(r"(?m)^\[[^\r\n]+\][ \t]*$")
    sections = list(header.finditer(text))
    matches = []
    for index, match in enumerate(sections):
        if match.group().strip() != section:
            continue
        end = sections[index + 1].start() if index + 1 < len(sections) else len(text)
        body = text[match.end():end]
        if not re.search(rf'(?m)^name\s*=\s*"{re.escape(name)}"[ \t]*$', body):
            continue
        updated, count = re.subn(
            r'(?m)^(version[ \t]*=[ \t]*)"[^"\r\n]+"([ \t]*)$',
            lambda m: f'{m[1]}"{version}"{m[2]}',
            body,
        )
        if count != 1:
            raise ValueError(f"Expected exactly one version for {name} in {section}")
        matches.append((match.end(), end, updated))
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one {section} for {name}")
    start, end, updated = matches[0]
    return text[:start] + updated + text[end:]


def main():
    version = os.environ["RELEASE_VERSION"]
    if not re.fullmatch(r"(?:0|[1-9][0-9]{0,9})(?:\.(?:0|[1-9][0-9]{0,9})){2}", version):
        raise SystemExit("RELEASE_VERSION must be a stable X.Y.Z version.")

    root = Path(__file__).resolve().parent.parent
    files = [(root / "Cargo.toml", "[package]"), (root / "Cargo.lock", "[[package]]")]
    changes = [
        (path, replace_version(path.read_text(encoding="utf-8"), section, "marzianetto", version))
        for path, section in files
    ]
    for path, updated in changes:
        with path.open("w", encoding="utf-8", newline="\n") as output:
            output.write(updated)
    print(f"Build version: {version}")


if __name__ == "__main__":
    main()
