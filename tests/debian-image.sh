#!/bin/sh
# Builds the Debian test image (Debian's libvirt, QEMU, SSH server, …) on
# top of the Debian build image: localhost/virt-manager-nextgen-debian-test:<release>.
# Used by tests/debian.sh and tests/remote-host.sh.
set -u
cd "$(dirname "$0")/.."
release=${1:-trixie}
podman build --quiet --build-arg DEBIAN_RELEASE="$release" \
    -t "localhost/virt-manager-nextgen-debian:$release" packaging/debian >/dev/null || exit 1
podman build --quiet --build-arg BUILD_IMAGE="localhost/virt-manager-nextgen-debian:$release" \
    -t "localhost/virt-manager-nextgen-debian-test:$release" tests/debian >/dev/null || exit 1
