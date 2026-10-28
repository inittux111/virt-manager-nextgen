#!/bin/sh
# Builds the RPM from the committed code (HEAD). Run inside the toolbox:
#   toolbox run -c virt-manager-nextgen packaging/build-rpm.sh
# The package lands in target/rpmbuild/RPMS/<arch>/.
set -eu

cd "$(dirname "$0")/.."
name=virt-manager-nextgen
version=$(sed -n 's/^Version: *//p' packaging/$name.spec)
top="$PWD/target/rpmbuild"
work="$top/work"

# Cargo's cache stays in the container, never in the shared home directory.
export CARGO_HOME=/opt/cargo

# Earlier packages in RPMS stay, so an older version can be reinstalled.
rm -rf "$top/SOURCES" "$top/BUILD" "$top/SRPMS" "$work"
mkdir -p "$top/SOURCES" "$work"

# The code, exactly as committed.
git archive --format=tar.gz --prefix="$name-$version/" -o "$top/SOURCES/$name-$version.tar.gz" HEAD

# All crates, so rpmbuild needs no network.
cargo vendor --locked --versioned-dirs "$work/vendor" >/dev/null
tar -C "$work" -czf "$top/SOURCES/$name-$version-vendor.tar.gz" vendor

rpmbuild -bb --define "_topdir $top" packaging/$name.spec
rm -rf "$work"
find "$top/RPMS" -name "$name-$version-*.rpm" -print
