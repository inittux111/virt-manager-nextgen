#!/bin/sh
# Debian 13 tests (see test-pipeline.sh): builds and runs everything inside
# a Debian 13 container with Debian's own libvirt and QEMU (using this
# computer's KVM). This computer's libvirt and VMs are not touched.
set -u

cd "$(dirname "$0")/.."
release=${DEBIAN_RELEASE:-trixie}
OUT="$PWD/target/test-report/debian13"
src="$PWD/target/test-debian-src"
rm -rf "$OUT" "$src"
mkdir -p "$OUT" "$src"

echo "Preparing the Debian $release test container…"
sh tests/debian-image.sh "$release" || exit 1

# The working tree as it is now (including uncommitted changes).
git ls-files -z --cached --others --exclude-standard | tar --null -T - -c | tar -x -C "$src"

runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
# --privileged (still within this user's rootless namespace) lets libvirt set
# up the test network's bridge inside the container.
podman run --rm --name vmng-test-debian \
    --privileged --device /dev/kvm --security-opt label=disable \
    -e XDG_RUNTIME_DIR=/run/vmng -e WAYLAND_DISPLAY=vmng-test \
    -v "$runtime/vmng-test:/run/vmng/vmng-test" \
    -v "$src:/src" -v "$OUT:/out" -v virt-manager-nextgen-cargo:/cargo \
    "localhost/virt-manager-nextgen-debian-test:$release" sh /src/tests/debian-inside.sh
rm -rf "$src"
! grep -q '^FAIL' "$OUT/results.txt"
