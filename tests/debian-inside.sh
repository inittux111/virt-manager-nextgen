#!/bin/sh
# Runs inside the Debian 13 test container (see tests/debian.sh): starts
# Debian's libvirt, builds the app for Debian 13 and runs the shared suite.
set -u

cd /src
OUT=/out
mkdir -p "$OUT"
. tests/lib.sh

# libvirt in a container: QEMU runs as root, without cgroups or private
# namespaces (the container already isolates it).
cat >>/etc/libvirt/qemu.conf <<CONF
user = "root"
group = "root"
cgroup_controllers = [ ]
namespaces = [ ]
remember_owner = 0
CONF
virtlogd -d
libvirtd -d
for _ in $(seq 30); do virsh -c qemu:///system version >/dev/null 2>&1 && break; sleep 1; done
virsh -c qemu:///system version >"$OUT/libvirt-version.log" 2>&1

echo "== Debian 13 ($(. /etc/os-release; echo "$PRETTY_NAME"))"
# GTK 4.18 / libadwaita 1.7: the build without the gnome49 feature.
check lint cargo clippy --workspace --all-targets --no-default-features -- -D warnings
check unit-tests cargo test --workspace --no-default-features
check build cargo build --workspace --all-targets --no-default-features
make_iso "$OUT/test.iso" >/dev/null 2>&1

pool=vmng-test-pipeline
mkdir -p /tmp/pool
virsh -c qemu:///system pool-create-as "$pool" dir --target /tmp/pool >"$OUT/pool.log" 2>&1

BIN=/src/target/debug OUT="$OUT" ISO="$OUT/test.iso" POOL="$pool" \
    VM_URI=qemu:///system NET_URI=qemu:///system sh tests/suite.sh
