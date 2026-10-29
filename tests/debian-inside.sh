#!/bin/sh
# Runs inside the Debian 13 test container (see tests/debian.sh): starts
# Debian's libvirt, builds the app for Debian 13 and runs the shared suite.
set -u

cd /src
OUT=/out
mkdir -p "$OUT"
. tests/lib.sh

sh tests/libvirt-in-container.sh
virsh -c qemu:///system version >"$OUT/libvirt-version.log" 2>&1

echo "== Debian 13 ($(. /etc/os-release; echo "$PRETTY_NAME"))"
# GTK 4.18 / libadwaita 1.7: the build without the gnome49 feature.
check lint cargo clippy --workspace --all-targets --no-default-features -- -D warnings
check unit-tests cargo test --workspace --no-default-features
check regression-tests cargo test -p vmng-virt --test regressions
check build cargo build --workspace --all-targets --no-default-features
make_iso "$OUT/test.iso" >/dev/null 2>&1

pool=vmng-test-pipeline
mkdir -p /tmp/pool
virsh -c qemu:///system pool-create-as "$pool" dir --target /tmp/pool >"$OUT/pool.log" 2>&1

# Remote checks: this container reaches its own libvirt over SSH.
ssh-keygen -q -t ed25519 -N '' -C vmng-test -f /root/vmng-test-key
sh tests/sshd-in-container.sh /root/vmng-test-key.pub
/usr/sbin/sshd
remote="qemu+ssh://root@127.0.0.1/system?keyfile=/root/vmng-test-key&no_verify=1"

BIN=/src/target/debug OUT="$OUT" ISO="$OUT/test.iso" POOL="$pool" REMOTE_URI="$remote" \
    VM_URI=qemu:///system NET_URI=qemu:///system sh tests/suite.sh
