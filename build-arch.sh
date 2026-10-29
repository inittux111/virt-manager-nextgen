#!/bin/sh
# Builds an Arch Linux package (.pkg.tar.zst) of the current version inside
# a podman container with the newest Arch Linux. Run it on the host:
#
#   ./build-arch.sh
#
# Unofficial and not tested: the package is built, but the app isn't tested
# on Arch. Only committed code is built; the package lands in target/arch/.
# Nothing is installed on the host.
set -eu
cd "$(dirname "$0")"

if ! command -v podman >/dev/null; then
    echo "podman is not installed." >&2
    exit 1
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes; they are not in this build." >&2
fi
version=$(packaging/version.sh)
out="$PWD/target/arch"
rm -rf "$out/build"
mkdir -p "$out/build"
sed "s/^pkgver=.*/pkgver=$version/" packaging/arch/PKGBUILD >"$out/build/PKGBUILD"
git archive --prefix="virt-manager-nextgen-$version/" -o "$out/build/virt-manager-nextgen-$version.tar.gz" HEAD

echo "Building the Arch Linux package in a container (unofficial, not tested)…"
# makepkg refuses to run as root, so it runs as a throwaway user.
podman run --rm -v "$out:/work:Z" -v virt-manager-nextgen-arch-cargo:/cargo docker.io/library/archlinux:latest sh -c '
    set -e
    pacman -Syu --noconfirm --needed base-devel rust pkgconf gtk4 libadwaita libvirt libosinfo osinfo-db \
        spice-gtk gtk-vnc vte4 hicolor-icon-theme >/dev/null
    useradd -m builder
    chown -R builder /work/build /cargo
    su builder -c "cd /work/build && CARGO_HOME=/cargo makepkg -f --noconfirm"
    cp /work/build/virt-manager-nextgen-*.pkg.tar.zst /work/
    # Hand everything back to the person running the script.
    chown -R root:root /work /cargo
'
rm -rf "$out/build"
pkg=$(ls -t "$out"/virt-manager-nextgen-*.pkg.tar.zst | head -1)
echo
echo "Built $pkg"
echo "Install it on Arch Linux with:"
echo "  sudo pacman -U ./target/arch/$(basename "$pkg")"
