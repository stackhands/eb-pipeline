#!/usr/bin/env python3
"""Fail when a release archive misses a declared binary.

The declaration file is also checked against the workspace's built binaries, so
the release check fails when a new src/bin tool is added without updating the
release declaration.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tarfile


def read_declared(path: pathlib.Path) -> list[str]:
    items: list[str] = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        items.append(line)
    declared = sorted(set(items))
    if declared != items:
        print(
            f"{path} must be sorted with no duplicate entries; expected order: "
            + ", ".join(declared),
            file=sys.stderr,
        )
        sys.exit(1)
    return declared


def workspace_bins() -> list[str]:
    out = subprocess.check_output(
        ["cargo", "metadata", "--format-version=1", "--no-deps"],
        text=True,
    )
    meta = json.loads(out)
    bins: set[str] = set()
    for package in meta["packages"]:
        for target in package["targets"]:
            if "bin" in target["kind"]:
                bins.add(target["name"])
    return sorted(bins)


def archive_bins(path: pathlib.Path) -> set[str]:
    names: set[str] = set()
    with tarfile.open(path, mode="r:xz") as tar:
        for member in tar.getmembers():
            if not member.isfile() or member.mode & 0o111 == 0:
                continue
            parts = pathlib.PurePosixPath(member.name).parts
            if len(parts) != 2:
                continue
            names.add(parts[1])
    return names


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--declared", required=True, type=pathlib.Path)
    p.add_argument("--archive", required=True, type=pathlib.Path)
    args = p.parse_args()

    declared = read_declared(args.declared)
    built = workspace_bins()
    if declared != built:
        print(
            "declared release binaries differ from workspace binaries",
            file=sys.stderr,
        )
        print("declared: " + ", ".join(declared), file=sys.stderr)
        print("workspace: " + ", ".join(built), file=sys.stderr)
        return 1

    archived = archive_bins(args.archive)
    missing = [name for name in declared if name not in archived]
    if missing:
        print(
            f"{args.archive} is missing declared binaries: " + ", ".join(missing),
            file=sys.stderr,
        )
        print("archive files:", file=sys.stderr)
        for name in sorted(archived):
            print(f"  {name}", file=sys.stderr)
        return 1

    print(
        f"{args.archive} contains all declared binaries: " + ", ".join(declared),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
