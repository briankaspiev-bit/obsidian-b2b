"""Print every Cargo workspace in the repo that the Rust CI job should test and build.

One line per workspace: its root relative to the repo, a tab, then cargo flags that
leave out Tauri app crates (those are built by the desktop job, which builds their
web UI first; tauri-build fails without it).

The engine workspace lives at the repo root, but some tools (tools/merge) and possibly
services/ build standalone with their own [workspace]. Asking cargo for each
manifest's workspace finds all of them without hard-coding paths, so new Rust code is
picked up by CI as soon as it lands.
"""

import json
import pathlib
import subprocess
import sys

repo = pathlib.Path(__file__).resolve().parents[2]
manifests = subprocess.run(
    ["git", "ls-files", "*Cargo.toml"], cwd=repo, capture_output=True, text=True, check=True
).stdout.split()


def is_tauri_app(manifest_dir: pathlib.Path) -> bool:
    return any(
        (manifest_dir / name).exists()
        for name in ("tauri.conf.json", "tauri.conf.json5", "Tauri.toml")
    )


workspaces = {}  # root -> set of Tauri package names to exclude
for manifest in manifests:
    if is_tauri_app((repo / manifest).parent):
        continue
    meta = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest],
        cwd=repo,
        capture_output=True,
        text=True,
    )
    if meta.returncode != 0:
        sys.stderr.write(f"cargo metadata failed for {manifest}:\n{meta.stderr}\n")
        sys.exit(1)
    data = json.loads(meta.stdout)
    root = pathlib.Path(data["workspace_root"]).resolve().relative_to(repo).as_posix()
    excludes = workspaces.setdefault(root, set())
    for package in data["packages"]:
        if is_tauri_app(pathlib.Path(package["manifest_path"]).parent):
            excludes.add(package["name"])

for root in sorted(workspaces):
    flags = " ".join(f"--exclude {name}" for name in sorted(workspaces[root]))
    print(f"{root}\t{flags}".rstrip())
