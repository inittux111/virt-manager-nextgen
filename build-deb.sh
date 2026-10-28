#!/bin/sh
# Builds a Debian package (.deb) inside a podman container. Run it on the
# host (any distribution with podman):
#
#   ./build-deb.sh                 Debian testing (the default)
#   ./build-deb.sh unstable        another Debian release (needs GTK 4.20+)
#
# Builds the committed code (HEAD) at its current version; it doesn't make
# a new version number (build-rpm.sh does that). The package lands in
# target/debian/. Nothing is installed on the host.
set -eu

release=${1:-testing}
image=localhost/virt-manager-nextgen-debian:$release
cd "$(dirname "$0")"

if ! command -v podman >/dev/null; then
    echo "podman is not installed." >&2
    exit 1
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes; they are not in this build." >&2
fi

echo "Preparing the Debian $release build container…"
podman build --quiet --build-arg DEBIAN_RELEASE="$release" -t "$image" packaging/debian >/dev/null

# The code exactly as committed, in a folder the container can use.
out="$PWD/target/debian"
rm -rf "$out/src" "$out/root" "$out/shlibs"
mkdir -p "$out/src"
git archive HEAD | tar -x -C "$out/src"

podman run --rm \
    -e MAINTAINER="$(git config user.name) <$(git config user.email)>" \
    -v "$out:/work:Z" \
    -v virt-manager-nextgen-cargo:/cargo \
    "$image" sh /work/src/packaging/debian/make-deb.sh

rm -rf "$out/src" "$out/root" "$out/shlibs"
deb=$(ls -t "$out"/virt-manager-nextgen_*.deb | head -1)
echo
echo "Built $deb"
echo "Install it on Debian with:"
echo "  sudo apt install ./$(basename "$deb")"
