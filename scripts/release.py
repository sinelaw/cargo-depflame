#!/usr/bin/env python3
"""
Release script for cargo-depflame.

Bumps the patch version in Cargo.toml, updates Cargo.lock, commits, tags,
and pushes to GitHub — which triggers the release workflow that builds
binaries and publishes to crates.io.

Usage:
    python3 scripts/release.py            # bump and release
    python3 scripts/release.py --dry-run  # preview without changes

Requirements: git, cargo, and a clean working tree on the main branch.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"

# ── Helpers ──────────────────────────────────────────────────────────────


def run(cmd: list[str], *, check: bool = True, capture: bool = False) -> str:
    """Run a command, printing it first."""
    print(f"  $ {' '.join(cmd)}")
    result = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        check=check,
        text=True,
        capture_output=capture,
    )
    return result.stdout.strip() if capture else ""


def fatal(msg: str) -> None:
    print(f"\nerror: {msg}", file=sys.stderr)
    sys.exit(1)


# ── Version helpers ──────────────────────────────────────────────────────

VERSION_RE = re.compile(
    r'^(version\s*=\s*")(\d+\.\d+\.\d+)(")', re.MULTILINE
)


def read_current_version() -> str:
    text = CARGO_TOML.read_text()
    # Match the first version = "..." in [package] (before any [dependencies]).
    pkg_section = text.split("[dependencies]")[0] if "[dependencies]" in text else text
    m = VERSION_RE.search(pkg_section)
    if not m:
        fatal("Could not find version in Cargo.toml")
    return m.group(2)


def bump_patch(current: str) -> str:
    major, minor, patch = map(int, current.split("."))
    return f"{major}.{minor}.{patch + 1}"


def set_version_in_cargo_toml(new_version: str) -> None:
    text = CARGO_TOML.read_text()
    # Only replace the first occurrence (in [package], not dependencies).
    new_text, count = VERSION_RE.subn(
        rf"\g<1>{new_version}\g<3>", text, count=1
    )
    if count == 0:
        fatal("Failed to replace version in Cargo.toml")
    CARGO_TOML.write_text(new_text)


# ── Pre-flight checks ───────────────────────────────────────────────────


def preflight() -> None:
    # Must be in a git repo.
    if not (REPO_ROOT / ".git").exists():
        fatal("Not a git repository")

    # Working tree must be clean.
    status = run(["git", "status", "--porcelain"], capture=True)
    if status:
        fatal("Working tree is not clean. Commit or stash changes first.\n" + status)

    # Must be on main branch.
    branch = run(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture=True)
    if branch != "main":
        fatal(f"Not on main branch (currently on {branch!r}). Switch to main first.")

    # Must be up to date with remote.
    run(["git", "fetch", "origin", "main"], check=False)
    local = run(["git", "rev-parse", "HEAD"], capture=True)
    remote = run(["git", "rev-parse", "origin/main"], capture=True, check=False)
    if remote and local != remote:
        fatal(
            "Local main is not up to date with origin/main.\n"
            "  Run: git pull --rebase origin main"
        )


# ── Main ─────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Bump patch version and release cargo-depflame.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would happen without making changes",
    )
    args = parser.parse_args()

    current = read_current_version()
    new_version = bump_patch(current)
    tag = f"v{new_version}"

    print(f"\ncargo-depflame release")
    print(f"  {current} -> {new_version} ({tag})")
    print()

    if args.dry_run:
        print("[dry-run] Would perform the following steps:")
        print(f"  1. Update Cargo.toml version to {new_version}")
        print(f"  2. Run cargo check (updates Cargo.lock)")
        print(f"  3. Run cargo test")
        print(f"  4. Commit: 'Release {tag}'")
        print(f"  5. Tag: {tag}")
        print(f"  6. Push commit and tag to origin")
        print("\nNo changes made.")
        return

    # Pre-flight.
    print("Running pre-flight checks...")
    preflight()
    print()

    # Check that the tag doesn't already exist.
    existing_tags = run(["git", "tag", "-l", tag], capture=True)
    if existing_tags:
        fatal(f"Tag {tag} already exists")

    # Step 1: Bump version.
    print(f"Updating Cargo.toml to {new_version}...")
    set_version_in_cargo_toml(new_version)

    # Step 2: Update Cargo.lock and verify build.
    print("\nRunning cargo check...")
    run(["cargo", "check"])

    # Step 3: Run tests.
    print("\nRunning cargo test...")
    run(["cargo", "test"])

    # Step 4: Commit.
    print("\nCommitting...")
    run(["git", "add", "Cargo.toml", "Cargo.lock"])
    run(["git", "commit", "-m", f"Release {tag}"])

    # Step 5: Tag.
    print(f"\nTagging {tag}...")
    run(["git", "tag", "-a", tag, "-m", f"Release {tag}"])

    # Step 6: Push.
    print("\nPushing to origin...")
    run(["git", "push", "origin", "main"])
    run(["git", "push", "origin", tag])

    print(f"\nDone! Release {tag} has been pushed.")
    print("The GitHub Actions release workflow will now:")
    print("  - Build binaries for Linux, macOS, and Windows")
    print("  - Create a GitHub Release with the binaries")
    print("  - Publish to crates.io (if CARGO_REGISTRY_TOKEN is set)")


if __name__ == "__main__":
    main()
