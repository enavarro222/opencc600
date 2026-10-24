#!/usr/bin/env python3
"""Prepare the Cargo manifests and lockfiles for a vX.Y.Z release; does not commit or tag."""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FILES = (ROOT / "Cargo.toml", ROOT / "firmware/Cargo.toml", ROOT / "Cargo.lock", ROOT / "firmware/Cargo.lock")
VERSION_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def fail(message: str) -> None:
    raise SystemExit(message)


def run(*args: str, cwd: Path = ROOT) -> str:
    result = subprocess.run(args, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode:
        fail(result.stdout.rstrip() or f"command failed: {' '.join(args)}")
    return result.stdout


def section_version(path: Path, section: str) -> str:
    lines = path.read_text().splitlines()
    start = next((i for i, line in enumerate(lines) if line.strip() == f"[{section}]"), None)
    if start is None:
        fail(f"section [{section}] not found in {path.relative_to(ROOT)}")
    end = next((i for i in range(start + 1, len(lines)) if re.match(r"\s*\[", lines[i])), len(lines))
    for line in lines[start + 1:end]:
        match = re.match(r'^\s*version\s*=\s*"([^"]+)"', line)
        if match:
            return match.group(1)
    fail(f"version not found in [{section}] in {path.relative_to(ROOT)}")


def update_section(path: Path, section: str, version: str) -> None:
    text = path.read_text()
    lines = text.splitlines(keepends=True)
    start = next((i for i, line in enumerate(lines) if line.strip() == f"[{section}]"), None)
    if start is None:
        fail(f"section [{section}] not found in {path.relative_to(ROOT)}")
    end = next((i for i in range(start + 1, len(lines)) if re.match(r"\s*\[", lines[i])), len(lines))
    for i in range(start + 1, end):
        match = re.match(r'^(\s*version\s*=\s*)"[^\"]+"(.*)$', lines[i])
        if match:
            lines[i] = f'{match.group(1)}"{version}"{match.group(2)}'
            path.write_text("".join(lines))
            return
    fail(f"version not found in [{section}] in {path.relative_to(ROOT)}")


def check_tag(tag: str) -> None:
    if not tag.startswith("v") or not VERSION_RE.fullmatch(tag[1:]):
        fail(f"release tag {tag!r} must have the form vX.Y.Z")
    if tag not in run("git", "tag", "--points-at", "HEAD").splitlines():
        fail(f"HEAD is not tagged {tag}")
    version = tag[1:]
    for manifest, section in ((FILES[0], "workspace.package"), (FILES[1], "package")):
        found = section_version(manifest, section)
        if found != version:
            fail(f"{manifest.relative_to(ROOT)} declares {found}; release tag {tag} requires {version}")
    for manifest in (ROOT, ROOT / "firmware"):
        metadata = json.loads(run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1", cwd=manifest))
        wrong = [f"{p['name']}={p['version']}" for p in metadata["packages"] if p["version"] != version]
        if wrong:
            fail(f"{manifest.relative_to(ROOT)} Cargo metadata does not match {tag}: {', '.join(wrong)}")
    print(f"release tag {tag} matches all Cargo package versions and lockfiles")


def prepare(version: str) -> None:
    current = section_version(FILES[0], "workspace.package")
    firmware_current = section_version(FILES[1], "package")
    if current != firmware_current:
        fail(f"workspace version {current} and firmware version {firmware_current} already differ")
    if tuple(map(int, version.split("."))) <= tuple(map(int, current.split("."))):
        fail(f"{version} must be greater than the current Cargo version {current}")
    if run("git", "status", "--porcelain", "--untracked-files=no").strip():
        fail("tracked Git files must be clean; commit or stash changes before preparing a release")
    for config in (ROOT / ".cargo/config", ROOT / ".cargo/config.toml"):
        if config.exists() and re.search(r"^\s*\[patch(?:\.|\])", config.read_text(), re.MULTILINE):
            fail(f"disable local Cargo patches in {config.relative_to(ROOT)} before preparing a release; "
                 "they would write local paths into Cargo.lock")
    tag = f"v{version}"
    if subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}"], cwd=ROOT,
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        fail(f"tag {tag} already exists")

    originals = {path: path.read_bytes() for path in FILES}
    try:
        update_section(FILES[0], "workspace.package", version)
        update_section(FILES[1], "package", version)
        for manifest in (ROOT, ROOT / "firmware"):
            print(run("cargo", "update", "--workspace", "--offline", cwd=manifest), end="")
            manifest_path = manifest / "Cargo.toml"
            section = "workspace.package" if manifest == ROOT else "package"
            expected = section_version(manifest_path, section)
            if expected != version:
                fail(f"Cargo version verification failed in {manifest}")
            run("cargo", "metadata", "--locked", "--no-deps", "--format-version", "1", cwd=manifest)
        print(run("make", "check"), end="")
    except BaseException:
        for path, contents in originals.items():
            path.write_bytes(contents)
        raise

    print(f"Prepared Cargo version {version} in the workspace and firmware; lockfiles are updated.")
    print("Review and commit the changes, then create and push the release tag:")
    print(f"  git diff -- Cargo.toml Cargo.lock firmware/Cargo.toml firmware/Cargo.lock")
    print(f"  git add Cargo.toml Cargo.lock firmware/Cargo.toml firmware/Cargo.lock && git commit -m 'release: {tag}'")
    print(f"  git tag {tag} && git push origin HEAD {tag}")


def main() -> None:
    if len(sys.argv) == 3 and sys.argv[1] == "--check-tag":
        check_tag(sys.argv[2])
    elif len(sys.argv) == 2 and VERSION_RE.fullmatch(sys.argv[1]):
        prepare(sys.argv[1])
    else:
        fail("usage: make prepare-release VERSION=0.0.4, or prepare_release.py --check-tag v0.0.4")


if __name__ == "__main__":
    main()
