#!/usr/bin/env python3
"""Resolve one release revision and prepare versioned artifacts using only stdlib."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib

STABLE = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
PRIORITY = {"patch": 1, "minor": 2, "major": 3}


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def ancestor(base, target):
    return subprocess.run(["git", "merge-base", "--is-ancestor", base, target],
                          check=False).returncode == 0


def bump_for(messages):
    result = "patch"
    for message in messages:
        tags = re.findall(r"(?<!\w)#(major|minor|patch)(?!\w)", message.lower())
        if re.search(r"^BREAKING[ -]CHANGE:", message, re.MULTILINE) or re.match(
                r"^[a-z]+(?:\([^\n]*\))?!:", message, re.IGNORECASE):
            tags.append("major")
        if re.match(r"^feat(?:\([^\n]*\))?:", message, re.IGNORECASE):
            tags.append("minor")
        for tag in tags:
            if PRIORITY[tag] > PRIORITY[result]:
                result = tag
    return result


def prepare(args):
    # Resolve refs once. Every following job receives the full immutable SHA.
    if not args.revision or args.revision.startswith("-"):
        raise ValueError("A commit SHA, branch or tag is required")
    target = git("rev-parse", "--verify", "--end-of-options", args.revision + "^{commit}")
    if not ancestor(target, "origin/main"):
        raise ValueError("The selected revision must already belong to origin/main")
    versions = sorted((tuple(map(int, match.groups())), tag)
                      for tag in git("tag", "--list").splitlines()
                      if (match := STABLE.fullmatch(tag)))
    latest = versions[-1] if versions else None
    existing = [item for item in versions if git("rev-parse", item[1] + "^{commit}") == target]
    if existing:
        if len(existing) != 1 or existing[0] != latest:
            raise ValueError("Only the latest release revision can be resumed")
        numbers, tag = existing[0]
        previous = versions[-2][1] if len(versions) > 1 else ""
        version = ".".join(map(str, numbers))
    else:
        previous = latest[1] if latest else ""
        if previous and not ancestor(previous, target):
            raise ValueError("The target must descend from the latest stable release")
        log_range = f"{previous}..{target}" if previous else target
        messages = git("log", "--format=%B%x00", log_range).split("\x00")
        bump = bump_for([m.strip() for m in messages]) if args.bump == "auto" else args.bump
        if latest:
            major, minor, patch = latest[0]
            if bump == "major":
                major, minor, patch = major + 1, 0, 0
            elif bump == "minor":
                minor, patch = minor + 1, 0
            else:
                patch += 1
            version = f"{major}.{minor}.{patch}"
        else:
            # First release uses this revision's declared baseline, not the workflow's.
            manifest = tomllib.loads(git("show", f"{target}:Cargo.toml"))
            version = manifest["package"]["version"]
            if not STABLE.fullmatch("v" + version):
                raise ValueError("First release requires a stable Cargo package version")
        tag = "v" + version

    root = Path(args.output)
    root.mkdir(parents=True, exist_ok=True)
    metadata = {"sha": target, "version": version, "tag": tag, "previous": previous}
    (root / "release.json").write_text(json.dumps(metadata, indent=2) + "\n")
    image = f"ghcr.io/{args.repository.lower()}"
    lines = [f"## {tag}", "", f"Revision: `{target}`", "",
             f"- Container: `{image}:{version}`",
             f"- Helm chart: `oci://ghcr.io/{args.repository.split('/')[0].lower()}/charts/nushelf`",
             f"- Chart version: `{version}`", "", "## Changes", ""]
    log_range = f"{previous}..{target}" if previous else target
    for line in git("log", "--reverse", "--format=%H%x09%s", log_range).splitlines():
        sha, subject = line.split("\t", 1)
        lines.append(f"- {subject} ([{sha[:7]}](https://github.com/{args.repository}/commit/{sha}))")
    if previous:
        lines += ["", f"[Full comparison](https://github.com/{args.repository}/compare/{previous}...{tag})"]
    (root / "notes.md").write_text("\n".join(lines) + "\n")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        for key, value in metadata.items():
            output.write(f"{key}={value}\n")
    print(json.dumps(metadata, indent=2))


def stamp(args):
    root = Path(args.root)
    if not STABLE.fullmatch("v" + args.version):
        raise ValueError("Expected a stable release version")
    manifest_path = root / "Cargo.toml"
    lock_path = root / "Cargo.lock"
    manifest_text, lock_text = manifest_path.read_text(), lock_path.read_text()
    manifest = tomllib.loads(manifest_text)
    name, old = manifest["package"]["name"], manifest["package"]["version"]
    lock = tomllib.loads(lock_text)
    own = [p for p in lock["package"] if p["name"] == name and "source" not in p]
    if len(own) != 1 or own[0]["version"] != old:
        raise ValueError("Root manifest/lock version mismatch")
    section = re.compile(r"(\[package\]\s*\n)(.*?)(?=^\[|\Z)", re.MULTILINE | re.DOTALL)
    def replace_manifest(match):
        body, count = re.subn(r'^version\s*=\s*"[^"]+"\s*$',
                             f'version = "{args.version}"', match[2], flags=re.MULTILINE)
        if count != 1:
            raise ValueError("Expected exactly one package version")
        return match[1] + body
    manifest_text, count = section.subn(replace_manifest, manifest_text)
    if count != 1:
        raise ValueError("Expected exactly one package section")
    pattern = re.compile(r'(\[\[package\]\]\nname = "' + re.escape(name) +
                         r'"\nversion = ")' + re.escape(old) + r'(")')
    lock_text, count = pattern.subn(lambda m: m[1] + args.version + m[2], lock_text)
    if count != 1:
        raise ValueError("Expected one local package in Cargo.lock")
    manifest_path.write_text(manifest_text)
    lock_path.write_text(lock_text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    resolve = commands.add_parser("prepare")
    resolve.add_argument("--revision", required=True)
    resolve.add_argument("--bump", choices=["auto", "patch", "minor", "major"], default="auto")
    resolve.add_argument("--repository", required=True)
    resolve.add_argument("--output", default=".release")
    resolve.set_defaults(run=prepare)
    version = commands.add_parser("stamp")
    version.add_argument("--root", required=True)
    version.add_argument("--version", required=True)
    version.set_defaults(run=stamp)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
