#!/bin/sh
# Builds a new version of the RPM inside the project's toolbox container.
# Run it on the host:
#
#   ./build-rpm.sh            next version, e.g. 0.1.1 → 0.1.2 (fixes)
#   ./build-rpm.sh minor      e.g. 0.1.2 → 0.2.0 (new features)
#   ./build-rpm.sh major      e.g. 0.2.0 → 1.0.0
#   ./build-rpm.sh 1.2.3      a version of your choice
#   ./build-rpm.sh --rebuild  the current version again, no new number
#
# Add --latest (e.g. `./build-rpm.sh --latest`, `./build-rpm.sh minor
# --latest`) to fetch the newest code from GitHub first.
#
# Each new version is committed and tagged in git (packaging/new-version.sh),
# with the commit titles since the last version as its "what's new".
# The first run creates the container (same Fedora release as this
# computer, so the package's library versions match) and installs the
# build tools into it; nothing is installed on the host. Only committed
# changes are built.
set -eu

# --latest may come anywhere; the remaining argument is the version choice.
latest=false
choice=""
for arg in "$@"; do
    case $arg in
        --latest) latest=true ;;
        *) choice=$arg ;;
    esac
done

box=virt-manager-nextgen
spec=packaging/virt-manager-nextgen.spec
cd "$(dirname "$0")"

# Fetch the newest code, then start again with it (this script may have
# changed too).
if $latest; then
    packaging/update-sources.sh
    exec ./build-rpm.sh ${choice:+"$choice"}
fi

if ! command -v toolbox >/dev/null; then
    echo "toolbox is not installed (sudo dnf install toolbox)." >&2
    exit 1
fi

if ! podman container exists "$box"; then
    release=$(rpm -E %fedora)
    echo "Creating the $box container (Fedora $release)…"
    toolbox create --container "$box" --release "$release"
    toolbox run -c "$box" sudo dnf -y install rpm-build rpmdevtools git 'dnf-command(builddep)'
    toolbox run -c "$box" sudo dnf -y builddep "$spec"
    # Cargo's cache lives in the container, never in the shared home folder.
    toolbox run -c "$box" sh -c 'sudo mkdir -p /opt/cargo && sudo chown "$(id -u):$(id -g)" /opt/cargo'
    # The security check used by maintenance.sh.
    toolbox run -c "$box" env CARGO_HOME=/opt/cargo cargo install --locked --quiet cargo-audit
fi

packaging/toolbox-release.sh

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes; they are not in this build." >&2
fi

if [ "$choice" != "--rebuild" ]; then
    packaging/new-version.sh ${choice:+"$choice"}
fi

toolbox run -c "$box" packaging/build-rpm.sh

version=$(packaging/version.sh)
rpm=$(ls target/rpmbuild/RPMS/*/virt-manager-nextgen-"$version"-*.rpm)
echo
echo "Built $rpm"
echo "Install or upgrade it with:"
echo "  sudo dnf install ./$rpm"
if [ "$choice" != "--rebuild" ]; then
    echo "(The new version is committed and tagged here; 'git push --follow-tags' publishes it.)"
fi
