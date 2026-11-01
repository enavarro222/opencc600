#!/usr/bin/env python3
"""Include an available Musette web export in the local demo.

Run after copying docs/site/index.html. Without a sibling export, the page
keeps its published Musette URL (including in CI).
"""
import json
import shutil
import sys
from pathlib import Path


def stage(source: Path, site: Path) -> None:
    if not (source / "index.html").is_file():
        return
    shutil.copytree(source, site / "musette", dirs_exist_ok=True)
    page = site / "index.html"
    html = page.read_text()
    marker = "const LOCAL_MUSETTE = null;"
    if marker not in html:
        raise ValueError("local Musette marker missing from demo page")
    page.write_text(html.replace(marker, f"const LOCAL_MUSETTE = {json.dumps('./musette/')};", 1))
    print(f"Local Musette: {source} → {site / 'musette'}")


if __name__ == "__main__":
    stage(Path(sys.argv[1]), Path(sys.argv[2]))
