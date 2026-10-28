#!/bin/sh
# Builds the RPM inside the project's toolbox container. Run it on the host
# from anywhere:
#
#   ./build-rpm.sh
#
# The first run creates the container (same Fedora release as this
# computer, so the package's library versions match) and installs the
# build tools into it; nothing is installed on the host. Only committed
# changes are built.
set -eu

box=virt-manager-nextgen
spec=packaging/virt-manager-nextgen.spec
cd "$(dirname "$0")"

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
fi

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Note: there are uncommitted changes; they are not in this build." >&2
fi

toolbox run -c "$box" packaging/build-rpm.sh

version=$(sed -n 's/^Version: *//p' "$spec")
rpm=$(ls target/rpmbuild/RPMS/*/virt-manager-nextgen-"$version"-*.rpm)
echo
echo "Built $rpm"
echo "Install or upgrade it with:"
echo "  sudo dnf install ./$rpm"
