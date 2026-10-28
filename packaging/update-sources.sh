#!/bin/sh
# Brings the project folder up to the newest code on GitHub (used by
# `--latest` in build-rpm.sh and build-deb.sh). Only fast-forwards: it stops
# rather than overwrite uncommitted changes or merge diverging history.
set -eu

cd "$(dirname "$0")/.."
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "There are uncommitted changes in the project folder; commit or undo them first." >&2
    exit 1
fi
branch=$(git rev-parse --abbrev-ref HEAD)
echo "Fetching the newest code from GitHub ($branch)…"
git fetch --quiet --tags origin
if ! git merge --quiet --ff-only "origin/$branch" 2>/dev/null; then
    echo "This folder and GitHub have different changes on $branch; they need merging by hand." >&2
    exit 1
fi
echo "Now at: $(git log -1 --format='%h %s')"
