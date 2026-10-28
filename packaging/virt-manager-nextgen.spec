# Built locally from this repository with packaging/build-rpm.sh, which
# makes both sources: the code (git archive) and the vendored crates, so
# the build itself runs offline.

# Rust release builds carry no debug info worth a separate package.
%global debug_package %{nil}
%global app_id io.github.inittux111.VirtManagerNextgen

Name:           virt-manager-nextgen
Version:        0.1.1
Release:        1%{?dist}
Summary:        Create, run and manage QEMU/KVM virtual machines

# The app is GPL-3.0-or-later. Crates built into it: the libvirt bindings
# (virt, virt-sys) are LGPL-2.1-or-later; the rest MIT, MIT OR Apache-2.0,
# Unlicense OR MIT, and Unicode-3.0 (unicode-ident).
License:        GPL-3.0-or-later AND LGPL-2.1-or-later AND MIT AND (MIT OR Apache-2.0) AND (Unlicense OR MIT) AND Unicode-3.0
URL:            https://github.com/inittux111/virt-manager-nextgen
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

ExclusiveArch:  x86_64 aarch64

BuildRequires:  cargo >= 1.92
BuildRequires:  rust >= 1.92
BuildRequires:  gcc
BuildRequires:  pkgconfig(gtk4) >= 4.20
BuildRequires:  pkgconfig(libadwaita-1) >= 1.8
BuildRequires:  pkgconfig(libvirt)
BuildRequires:  pkgconfig(libosinfo-1.0)
BuildRequires:  pkgconfig(spice-client-glib-2.0)
BuildRequires:  pkgconfig(gvnc-1.0)
BuildRequires:  pkgconfig(vte-2.91-gtk4)
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

# Shared libraries (GTK, libadwaita, libvirt, libosinfo, SPICE, VNC, VTE)
# are found by rpm from the binary; these add the minimum versions and
# the data the app reads at run time.
Requires:       gtk4%{?_isa} >= 4.20
Requires:       libadwaita%{?_isa} >= 1.8
Requires:       osinfo-db
Requires:       hicolor-icon-theme
# Local QEMU/KVM machines need the libvirt daemon; remote hosts don't.
Recommends:     libvirt-daemon-kvm
Recommends:     libvirt-daemon-config-network

%description
A modern desktop app for virtual machines on QEMU/KVM, through libvirt:
create machines with automatic OS detection, run and snapshot them, use
their screen or text console, change their hardware, and manage the
host's virtual networks and storage pools.

%prep
%autosetup -a 1
mkdir -p .cargo
cat > .cargo/config.toml <<'CFG'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
CFG

%build
# Keep cargo's cache inside the build tree, never in the home directory.
export CARGO_HOME="$PWD/.cargo-home"
cargo build --release --offline --locked -p virt-manager-nextgen

%install
install -Dpm 0755 target/release/virt-manager-nextgen %{buildroot}%{_bindir}/virt-manager-nextgen
install -Dpm 0644 data/%{app_id}.desktop %{buildroot}%{_datadir}/applications/%{app_id}.desktop
install -Dpm 0644 data/%{app_id}.metainfo.xml %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml
install -Dpm 0644 data/icons/hicolor/scalable/apps/%{app_id}.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
install -Dpm 0644 data/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg \
    %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{app_id}.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/%{app_id}.metainfo.xml

%files
%license LICENSE
%{_bindir}/virt-manager-nextgen
%{_datadir}/applications/%{app_id}.desktop
%{_metainfodir}/%{app_id}.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/%{app_id}.svg
%{_datadir}/icons/hicolor/symbolic/apps/%{app_id}-symbolic.svg

%changelog
* Wed Oct 28 2026 Maarten van der Rijst <github@feedmebits.nl> - 0.1.1-1
- Point CLAUDE.md at BUILDING.md
- Add BUILDING.md and keep earlier packages between builds
- Document the versioned RPM builds
- Give every RPM build the next version number
- Open each machine's console in its own window
- Add build-rpm.sh: build the RPM in the toolbox from the host
- Give the hardware list a full right-click menu
- Show a live picture of a running machine's screen in its details

* Wed Oct 28 2026 Maarten van der Rijst <claude@feedmebits.nl> - 0.1.0-1
- First release
