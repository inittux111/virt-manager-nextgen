#!/bin/sh
# Builds a Debian package (.deb) inside a podman container. Run it on the
# host (any distribution with podman):
#
#   ./build-deb.sh            next version for fixes (0.1.1 → 0.1.2), Debian 13
#   ./build-deb.sh minor      next version for new features (0.1.2 → 0.2.0)
#   ./build-deb.sh major      next major version (0.2.0 → 1.0.0)
#   ./build-deb.sh 1.2.3      a version of your choice
#   ./build-deb.sh --rebuild  the current version again, no new number
#   ./build-deb.sh testing    for another Debian release (13 or newer)
#   ./build-deb.sh --latest   fetch the newest code from GitHub first
#
# Options combine, e.g. `./build-deb.sh testing minor --latest`. It works on
# its own, without build-rpm.sh or the Fedora toolbox; each new version is
# committed and tagged in git (packaging/new-version.sh), just like there.
# To build both packages for one version, run one script normally and the
# other with --rebuild. Only committed code is built; the package lands in
# target/debian/. Nothing is installed on the host.
set -eu

latest=false
release=trixie
choice=""
for arg in "$@"; do
    case $arg in
        --latest) latest=true ;;
        --rebuild | patch | minor | major | [0-9]*.[0-9]*.[0-9]*) choice=$arg ;;
        *) release=$arg ;;
    esac
done
image=localhost/virt-manager-nextgen-debian:$release
cd "$(dirname "$0")"

# Fetch the newest code, then start again with it (this script may have
# changed too).
if $latest; then
    packaging/update-sources.sh
    exec ./build-deb.sh "$release" ${choice:+"$choice"}
fi

if ! command -v podman >/dev/null; then
    echo "podman is not installed." >&2
    exit 1
fi
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes; they are not in this build." >&2
fi
if [ "$choice" != "--rebuild" ]; then
    packaging/new-version.sh ${choice:+"$choice"}
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
echo "  sudo apt install ./target/debian/$(basename "$deb")"
if [ "$choice" != "--rebuild" ]; then
    echo "(The new version is committed and tagged here; 'git push --follow-tags' publishes it.)"
fi
