#!/bin/sh
# Runs inside the Debian build container (see build-deb.sh): builds the app
# from /work/src and packs it as /work/virt-manager-nextgen_<version>_<arch>.deb.
set -eu

app_id=io.github.inittux111.VirtManagerNextgen
cd /work/src
version=$(sed -n 's/^Version: *//p' packaging/virt-manager-nextgen.spec)
arch=$(dpkg --print-architecture)

cargo build --release --locked -p virt-manager-nextgen
desktop-file-validate "data/$app_id.desktop"
appstreamcli validate --no-net "data/$app_id.metainfo.xml"

root=/work/root
rm -rf "$root"
install -Dm755 target/release/virt-manager-nextgen "$root/usr/bin/virt-manager-nextgen"
install -Dm644 "data/$app_id.desktop" "$root/usr/share/applications/$app_id.desktop"
install -Dm644 "data/$app_id.metainfo.xml" "$root/usr/share/metainfo/$app_id.metainfo.xml"
install -Dm644 "data/icons/hicolor/scalable/apps/$app_id.svg" "$root/usr/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 "data/icons/hicolor/symbolic/apps/$app_id-symbolic.svg" \
    "$root/usr/share/icons/hicolor/symbolic/apps/$app_id-symbolic.svg"
install -Dm644 LICENSE "$root/usr/share/doc/virt-manager-nextgen/copyright"

# The libraries the program links to, as Debian package dependencies (plus
# the GTK and libadwaita versions the app needs, which the scan can't see).
mkdir -p /work/shlibs/debian
printf 'Source: virt-manager-nextgen\n\nPackage: virt-manager-nextgen\nArchitecture: any\n' >/work/shlibs/debian/control
libs=$(cd /work/shlibs && dpkg-shlibdeps -O "$root/usr/bin/virt-manager-nextgen" | sed -n 's/^shlibs:Depends=//p')

mkdir -p "$root/DEBIAN"
cat >"$root/DEBIAN/control" <<CONTROL
Package: virt-manager-nextgen
Version: $version
Architecture: $arch
Maintainer: ${MAINTAINER:-virt-manager-nextgen builder <noreply@localhost>}
Depends: $libs, libgtk-4-1 (>= 4.20), libadwaita-1-0 (>= 1.8), osinfo-db
Recommends: libvirt-daemon-system, qemu-system-x86 | qemu-system
Suggests: swtpm, swtpm-tools
Section: admin
Priority: optional
Homepage: https://github.com/inittux111/virt-manager-nextgen
Description: Create, run and manage QEMU/KVM virtual machines
 A modern desktop app for virtual machines on QEMU/KVM, through libvirt:
 create machines with automatic OS detection, run and snapshot them, use
 their screen or text console, change their hardware, and manage the
 host's virtual networks and storage pools.
CONTROL
deb="/work/virt-manager-nextgen_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$deb"
echo "$deb"
