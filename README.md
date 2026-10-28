# Virtual Machines

A modern desktop app for QEMU/KVM virtual machines, through libvirt: a
GNOME-style alternative to virt-manager, written in Rust with GTK 4 and
libadwaita.

- Create machines from installer images, with the operating system
  detected automatically (libosinfo)
- Start, pause, shut down, clone and delete machines, and take snapshots
- Each machine's screen (SPICE or VNC) or text console in its own window
- Edit hardware, choose BIOS, UEFI or a specific UEFI firmware file, add a
  TPM 2.0 (Windows 11)
- Manage the host's virtual networks and storage pools
- CPU, memory, disk and network graphs, and guest details through the
  QEMU guest agent

The app needs **GTK 4.18 or newer and libadwaita 1.7 or newer** (Debian 13
and newer, Fedora 44 and newer). With GTK 4.20 / libadwaita 1.8 it uses
GNOME's own Keyboard Shortcuts dialog; with the older versions, its own
version of it. Both builds below happen inside a container, so no
development packages are installed on your computer.

## Fedora: RPM, built in a toolbox

Needs `toolbox` (`sudo dnf install toolbox` if it's missing). Fedora 44 or
newer.

```sh
git clone git@github.com:inittux111/virt-manager-nextgen.git
cd virt-manager-nextgen
./build-rpm.sh
sudo dnf install ./target/rpmbuild/RPMS/x86_64/virt-manager-nextgen-<version>-1.fc44.x86_64.rpm
```

The first run creates a toolbox container called `virt-manager-nextgen`
with the same Fedora release as your computer, and installs the build tools
into it. Every run makes a new version number, builds the committed code,
and prints the exact `dnf` command at the end.

See [BUILDING.md](BUILDING.md) for version numbers, keeping the app up to
date and secure, and what to do after a Fedora upgrade.

To start the app, open **Virtual Machines** from the app grid, or run
`virt-manager-nextgen`.

## Debian: .deb package, built in a podman container

Needs `podman` on the computer that builds; that computer doesn't have to
run Debian. By default the package is for **Debian 13 (trixie)**.

```sh
git clone git@github.com:inittux111/virt-manager-nextgen.git
cd virt-manager-nextgen
./build-deb.sh
```

This prepares a Debian 13 build container (the first time only), builds the
committed code in it, and puts the package in `target/debian/`. The
container gets a current Rust through `rustup`, because Debian 13's own
Rust is too old for the app. Cargo's download cache is kept in a podman
volume called `virt-manager-nextgen-cargo`.

To build for a newer Debian release, name it: `./build-deb.sh testing`. A
package only fits the release it was built for, so build one per release.

Copy the package to the Debian machine and install it with:

```sh
sudo apt install ./virt-manager-nextgen_<version>_amd64.deb
```

`apt` installs the libraries it needs, and by default also the libvirt
daemon and QEMU (recommended, to run machines on that computer). `swtpm`
(for TPM 2.0) is only suggested: install it yourself if you need it.
`build-deb.sh` doesn't make a new version number: it packs the current one,
which `build-rpm.sh` sets.

To remove the build container image and cache later:

```sh
podman rmi localhost/virt-manager-nextgen-debian:trixie
podman volume rm virt-manager-nextgen-cargo
```

## Running VMs

The app talks to libvirt. To run machines on the same computer, the libvirt
daemon and QEMU must be installed and running:

- Fedora: `sudo dnf install libvirt-daemon-kvm` (the RPM recommends it)
- Debian: `sudo apt install libvirt-daemon-system qemu-system-x86`

To manage the system-wide machines (`QEMU/KVM · System`) without a password
prompt each time, add yourself to the `libvirt` group and log in again:
`sudo usermod -aG libvirt $USER`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
