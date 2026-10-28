#!/bin/sh
# Maintenance check: keeps the app's building blocks up to date and checks
# them for known security problems. Run it on the host, about once a month
# or when GitHub sends a Dependabot alert:
#
#   ./maintenance.sh
#
# 1. Updates the build tools in the toolbox (Rust compiler, dev packages).
# 2. Updates the Rust crates to their newest compatible (fix) releases.
# 3. Checks every crate against the RustSec list of known security problems.
# 4. Builds, lints and runs all tests, plus an end-to-end check that creates
#    and deletes a throwaway VM on the user session (qemu:///session).
# 5. If everything passes, commits the updated crate list. Then run
#    ./build-rpm.sh to get a new version with the fixes.
#
# System libraries (GTK, libvirt, …) need nothing here: they come from
# Fedora and are updated with the normal system updates.
set -eu

box=virt-manager-nextgen
cd "$(dirname "$0")"
inbox() { toolbox run -c "$box" env CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:/usr/local/bin:/usr/bin:/bin "$@"; }

if ! podman container exists "$box"; then
    echo "The $box toolbox doesn't exist yet: run ./build-rpm.sh once first." >&2
    exit 1
fi
packaging/toolbox-release.sh

if ! git diff --quiet -- Cargo.lock; then
    echo "Cargo.lock has changes that aren't committed; commit or undo them first." >&2
    exit 1
fi

failed=""
step() { printf '\n== %s\n' "$1"; }

step "1/5 Updating the build tools in the toolbox"
toolbox run -c "$box" sudo dnf -y --refresh upgrade

step "2/5 Updating the crates"
inbox cargo install --locked --quiet cargo-audit
inbox cargo update 2>&1 | grep -v '^ *Locking' || true
if git diff --quiet -- Cargo.lock; then
    echo "All crates were already up to date."
fi

step "3/5 Checking for known security problems"
inbox cargo audit || failed="$failed security-check"

step "4/5 Building, linting and testing"
inbox cargo build --workspace --all-targets || failed="$failed build"
inbox cargo clippy --workspace --all-targets -- -D warnings || failed="$failed lint"
# The Debian 13 build (GTK 4.18, libadwaita 1.7) leaves out `gnome49`.
inbox cargo clippy -p virt-manager-nextgen --all-targets --no-default-features -- -D warnings \
    || failed="$failed lint-debian13"
inbox cargo test --workspace || failed="$failed tests"

step "5/5 End-to-end check (throwaway VM on the user session)"
tmp=$(mktemp -d)
./target/debug/examples/selftest "$tmp" || failed="$failed end-to-end"
rm -rf "$tmp"

echo
if [ -n "$failed" ]; then
    echo "Problems found:$failed"
    echo "The crate updates are left uncommitted. Ask Claude to look at the output above."
    exit 1
fi
if git diff --quiet -- Cargo.lock; then
    echo "Everything is up to date and all checks passed. Nothing to commit."
else
    git commit -q -m "Update crates (maintenance check)" -- Cargo.lock
    echo "All checks passed and the crate updates are committed."
    echo "Run ./build-rpm.sh to build a new version with them."
fi
