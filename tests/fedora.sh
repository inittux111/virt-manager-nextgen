#!/bin/sh
# Fedora 44 tests (see test-pipeline.sh): code checks and unit tests in the
# toolbox, then the shared suite on this computer against the user session
# (and the System connection for the vmng-test network/pool check).
set -u

cd "$(dirname "$0")/.."
OUT="$PWD/target/test-report/fedora44"
rm -rf "$OUT"
mkdir -p "$OUT"
. tests/lib.sh
box=virt-manager-nextgen
inbox() { toolbox run -c "$box" env CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:/usr/bin:/bin "$@"; }

echo "== Fedora 44"
check code-format inbox cargo fmt --all --check
check lint inbox cargo clippy --workspace --all-targets -- -D warnings
check lint-debian13-variant inbox cargo clippy -p virt-manager-nextgen --all-targets --no-default-features -- -D warnings
check unit-tests inbox cargo test --workspace
check regression-tests inbox cargo test -p vmng-virt --test regressions
check build inbox cargo build --workspace --all-targets
inbox sh -c ". tests/lib.sh && make_iso '$OUT/test.iso'" >/dev/null 2>&1

# The headless test screen (started here if it isn't running).
started_mutter=""
if [ ! -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/vmng-test" ]; then
    mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 --wayland-display=vmng-test >"$OUT/mutter.log" 2>&1 &
    started_mutter=$!
    sleep 3
fi

pool=vmng-test-pipeline
mkdir -p "$OUT/pool"
virsh -c qemu:///session pool-create-as "$pool" dir --target "$OUT/pool" >"$OUT/pool.log" 2>&1

# The "other computer" for the remote checks: a Debian container with its
# own libvirt, reached over SSH on 127.0.0.1:22222.
remote=$(sh tests/remote-host.sh start "$OUT/remote" 22222 2>"$OUT/remote-host.log")

BIN="$PWD/target/debug" OUT="$OUT" ISO="$OUT/test.iso" POOL="$pool" REMOTE_URI="$remote" \
    VM_URI=qemu:///session NET_URI=qemu:///system WAYLAND_DISPLAY=vmng-test sh tests/suite.sh

sh tests/remote-host.sh stop

virsh -c qemu:///session pool-destroy "$pool" >>"$OUT/pool.log" 2>&1
[ -n "$started_mutter" ] && kill "$started_mutter"
! grep -q '^FAIL' "$OUT/results.txt"
