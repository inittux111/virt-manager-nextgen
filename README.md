# Virtual Machines

virt-manager-nextgen is a new, modern desktop app for QEMU/KVM virtual
machines, built as a personal-use project and based on
[virt-manager](https://virt-manager.org): the goal is a more modern-looking
virt-manager with most of its features. It has almost all of them, in a
GNOME-style app written from scratch in Rust with GTK 4 and libadwaita (it
doesn't reuse virt-manager's code). In the app menu and app grid it is
called "Virtual Machines". It works with libvirt, like virt-manager, so both
apps see the same machines.

## Use at your own risk

virt-manager-nextgen isn't affiliated with, endorsed by or supported by the
virt-manager, libvirt or QEMU projects or Red Hat.

- It manages your virtual machines through libvirt, the same way
  virt-manager does, but it is young and tested by one person. Changes it
  makes (editing hardware or XML, deleting machines, disks or snapshots,
  stopping networks) are real and some can't be undone, so keep backups of
  machines that matter.
- Changes in libvirt, QEMU or GNOME can break features until
  virt-manager-nextgen is updated.
- It comes with no warranty, as the GPL says: you use it at your own risk.

## Features

Everything below is also in virt-manager.

**Connections**
- The local QEMU/KVM System connection and User session, connected at
  startup if you choose
- Other computers over SSH (their System connection or a user session),
  logging in with your SSH key; their machines' screens come through an SSH
  tunnel

**Creating machines**
- Install from an installer image (ISO) or the computer's optical drive
- Network install from an install tree address (Fedora, RHEL-like, openSUSE,
  Debian), or network boot (PXE)
- Import an existing disk image, or create a machine without an installer
- The operating system is detected automatically (libosinfo), with its
  recommended memory, CPUs and disk size
- A new disk in any storage pool, an existing disk image, or no disk
- BIOS, UEFI or a specific UEFI firmware file, Secure Boot, TPM 2.0
  (Windows 11)
- Changing the hardware before the installation starts

**Managing machines**
- Start, shut down, reboot, force off, reset, pause and resume
- Save and stop (and discard the saved state)
- Start a machine when the host boots (autostart)
- Clone, rename and delete (choosing which disks to delete)
- Snapshots: inside the disk or as separate files, with or without the
  memory, with a picture of the screen; revert and delete

**Hardware**
- Title and description
- CPU count, CPU model ("copy host CPU", similar to the host, or any model the
  host can run), security mitigations, manual CPU layout (sockets, cores,
  threads)
- Current and maximum memory (raised while running), shared memory
- Boot order, firmware, the operating system type
- Disks: bus, cache mode, discard, detect zeroes, I/O mode, read-only,
  shareable, removable, serial number
- CD-ROM: change the disc, also while the machine runs
- Network cards: virtual network, host bridge, direct connection (macvtap) or
  user networking, model, MAC address, cable connected or not
- Display (SPICE or VNC, with password), video model and 3D acceleration,
  sound, input devices, USB and other controllers, serial ports and consoles,
  channels (guest agent, SPICE agent), TPM, random number generator
- Shared folders: virtiofs or 9p, read-only
- Add and remove hardware, also while the machine runs where possible
- View and edit the full XML

**Console**
- The screen (SPICE or VNC) or the text (serial) console, each machine in its
  own window
- Fullscreen, scaling, resizing the guest to the window, sending key
  combinations (Ctrl+Alt+Del, Ctrl+Alt+F1…), screenshots

**Host**
- Host details and usage
- Virtual networks: NAT, routed, isolated or bridge, IPv4/DHCP, IPv6, DNS
  domain, start at boot, and the full XML
- Storage pools: a folder, LVM volume group, NFS share or iSCSI target
- Volumes: create (thin or fully allocated, or based on another image) and
  delete

**Other**
- CPU, memory, disk and network graphs per machine and for the host
- Guest details through the QEMU guest agent (IP addresses, hostname,
  operating system, file systems)
- System tray icon
- Preferences: statistics, console behaviour, defaults for new machines,
  confirmations, and a light, dark or system style

### Not included

virt-manager has these, but virt-manager-nextgen doesn't:

- Connections over TLS or with a password typed into the app (SSH with a
  key works), and other hypervisors (Xen, LXC, bhyve and others): only
  QEMU/KVM
- Moving a running machine to another computer (migration)
- Passing a USB or PCI device of the host to a machine, USB redirection from
  the console, and mediated devices (e.g. a share of a graphics card)
- Booting a machine straight from a kernel file (direct kernel boot)
- Rarely used devices: parallel port, watchdog, smartcard, panic notifier,
  VSOCK
- More storage pool types: whole disks, ZFS, Ceph/RBD, Gluster, multipath and
  SCSI host adapters
- TPM 1.2, and passing the host's own TPM to a machine
- Looking inside a shut-off machine's disks, e.g. for its installed
  applications (libguestfs). Guest details come from the QEMU guest agent
  instead, while the machine runs (see [Guest details](#guest-details))

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

To remove the build container image and cache later:

```sh
podman rmi localhost/virt-manager-nextgen-debian:trixie
podman volume rm virt-manager-nextgen-cargo
```

## Arch Linux: unofficial, not tested

There is an **unofficial** Arch Linux package recipe
(`packaging/arch/PKGBUILD`). It **is not tested**: the package is known to
build, but the app isn't checked on Arch by the test pipeline, and Arch's
fast-moving updates can break it at any time. Fedora 44 and Debian 13 are the
supported systems.

The [latest release](https://github.com/inittux111/virt-manager-nextgen/releases/latest)
has a ready-made Arch package: `sudo pacman -U ./virt-manager-nextgen-<version>-1-x86_64.pkg.tar.zst`.

To build it yourself on Arch, put the recipe and the release's source file
(`virt-manager-nextgen-<version>.tar.gz`) in one folder and run:

```sh
makepkg -si
```

From another system (e.g. Fedora), build it in a podman container with the
newest Arch Linux (nothing is installed on your computer):

```sh
./build-arch.sh                     # → target/arch/virt-manager-nextgen-<version>-1-x86_64.pkg.tar.zst
```

To run machines on the same computer, also install `qemu-desktop`,
`dnsmasq` and `edk2-ovmf` (and `swtpm` for TPM 2.0, `virtiofsd` for shared
folders), and start libvirt: `sudo systemctl enable --now libvirtd`.

## The build scripts

Both scripts run on your computer and do the actual building in a container.
They are independent of each other: `build-rpm.sh` needs only `toolbox`,
`build-deb.sh` only `podman` (plus `git` for both). Each makes its own new
version number by default, with the same options. By default they build the
code as it is committed in the project folder (uncommitted edits are left
out, with a note).

To build **both packages for the same version**, run one script normally and
the other with `--rebuild`, e.g. `./build-rpm.sh minor` then
`./build-deb.sh --rebuild`.

### `./build-rpm.sh`: Fedora RPM

| Command                            | What it does                                            |
|------------------------------------|---------------------------------------------------------|
| `./build-rpm.sh`                   | next version for fixes, e.g. 0.1.6 → 0.1.7, and build    |
| `./build-rpm.sh minor`             | next version for new features, e.g. 0.1.7 → 0.2.0        |
| `./build-rpm.sh major`             | next major version, e.g. 0.2.0 → 1.0.0                   |
| `./build-rpm.sh 1.4.0`             | a version number of your choice                          |
| `./build-rpm.sh --rebuild`         | build the current version again, no new number           |
| `./build-rpm.sh --latest`          | first fetch the newest code from GitHub, then as above   |

`--latest` combines with the others, e.g. `./build-rpm.sh minor --latest`.
Each new version is committed and tagged in git (`v0.1.7`, …), with the
changes since the previous version as its "what's new" notes. The package
lands in `target/rpmbuild/RPMS/x86_64/`; earlier ones stay there.

### `./build-deb.sh`: Debian package

| Command                            | What it does                                            |
|------------------------------------|---------------------------------------------------------|
| `./build-deb.sh`                   | next version for fixes, build for Debian 13 (trixie)     |
| `./build-deb.sh minor`             | next version for new features                            |
| `./build-deb.sh major`             | next major version                                       |
| `./build-deb.sh 1.4.0`             | a version number of your choice                          |
| `./build-deb.sh --rebuild`         | build the current version again, no new number           |
| `./build-deb.sh testing`           | build for another Debian release (13 or newer)           |
| `./build-deb.sh --latest`          | first fetch the newest code from GitHub, then as above   |

Options combine, e.g. `./build-deb.sh testing minor --latest`. New versions
are committed and tagged the same way as with `build-rpm.sh`. The package
lands in `target/debian/`.

### `--latest`: the newest code from GitHub

With `--latest`, the scripts first update the project folder to the newest
code on GitHub, then build that. Use it on a computer where you don't edit
the code, to build whatever was pushed last. To keep your work safe, it
stops without changing anything when the folder has uncommitted changes,
or when the folder and GitHub both have changes the other doesn't (they
then need merging by hand).

## Testing: `./test-pipeline.sh`

One command checks that every feature still works, on both systems
(the separate checks and screenshot tools are described in
[DEVELOPING.md](DEVELOPING.md)):

```sh
./test-pipeline.sh            # Fedora 44 and Debian 13
./test-pipeline.sh fedora     # Fedora 44 only
./test-pipeline.sh debian     # Debian 13 only
```

For each system it runs:

1. **Code checks**: formatting, the lint check (both the full build and the
   Debian 13 variant), all unit tests, and the **regression tests**: one test
   for each bug that was found and fixed (`crates/virt/tests/regressions.rs`),
   so it can't come back unnoticed.
2. **Real libvirt checks with throwaway VMs**:
   - create, start and delete a VM
   - snapshots, clone, rename and delete
   - adding, editing and removing hardware
   - inserting a disc image into a running VM's CD-ROM drive
   - BIOS, UEFI and each UEFI firmware file
   - TPM 2.0
   - CPU models, CPU layout and security mitigations
   - changing the operating system type
   - machine options: a new VM on an existing disk image, title and
     description, current and maximum memory (also while running), shared
     folders (9p and virtiofs, read-only), disk options, a chosen MAC address
   - network boot (PXE) and a network install from Debian's mirror (needs
     internet)
   - 3D acceleration (skipped where QEMU can't use the graphics card, e.g.
     inside the Debian container with an NVIDIA card)
   - snapshots inside the disk and as separate files
   - virtual networks (also edited as XML) and storage pools, with fully
     allocated volumes and a volume based on another image
   - starting a machine when the host boots (autostart), here and on another
     computer
   - **another computer over SSH**: a separate Debian container with its own
     libvirt plays the other computer (on Debian 13 the container connects to
     itself). The app adds it from the Add Remote Computer dialog, lists its
     machines, shows a machine's screen through the SSH tunnel and the text
     console, and removes it again
3. **Screens**: a screenshot of every main screen (machines, hardware,
   firmware, overview, Add Hardware, snapshots, performance, the console and
   text console, host, networks and a network's XML, storage and New Volume,
   every New VM step, a new VM on an existing disk, Preferences, Keyboard
   Shortcuts, narrow window, dark, the Dark appearance setting and high
   contrast), with a running test VM. Each must render without errors.
   Editing a network's XML in the app and creating a VM on an existing disk
   are checked in libvirt afterwards.
4. **Regression checks in the app**: every hardware page of a test VM is
   opened in turn, and the check fails if just looking at one counts as a
   change.

Where it runs:

- **Fedora 44** on this computer: code checks in the toolbox, VMs on your
  user session, and the network/pool check on the System connection.
- **Debian 13** entirely inside a Debian 13 container with Debian's own
  libvirt, QEMU, UEFI firmware and TPM software (using this computer's KVM).
  Your own libvirt and VMs are not touched.

Everything the tests create is named `vmng-test-…` and removed again. The
summary at the end lists what passed and failed; logs and screenshots are in
`target/test-report/fedora44/` and `target/test-report/debian13/`. The
screens use a headless test display (started automatically), so nothing
appears on your desktop. A full run takes about 10 minutes; the first
Debian run takes longer while it downloads Debian's packages.

What it can't check: the tray icon (the test display has no tray) and how
things feel on a real Debian desktop.

### On GitHub

A light version runs automatically on GitHub on every push and pull request
(`.github/workflows/ci.yml`): formatting, the lint check, unit tests, the
regression tests and a full build, for Fedora 44 and Debian 13 (about 5
minutes). Changes to only `.md` files (like this README) don't start it. The repository
page shows a green or red mark for the latest code, and GitHub emails you
when it fails. The VM and screen tests need this computer's KVM, so they
only run with `./test-pipeline.sh`.

Dependabot (`.github/dependabot.yml`) watches the Rust crates, which both
builds share: it sends security alerts, opens pull requests that fix
vulnerable crates, and once a week a pull request with all compatible crate
updates; each is built and tested on both systems before it is merged. It
also keeps the workflow's GitHub Actions current. The system libraries
(GTK, libvirt, …) come from Fedora and Debian and are updated with the
normal system updates; the GitHub check always uses the newest Fedora 44
and Debian 13 images, so an update that breaks the build shows up there.

### Security updates

- **Rust crates** (the libraries built into the app): a GitHub check,
  *Security check* (`.github/workflows/security.yml`), runs `cargo audit`
  once a week and whenever the crate list changes. It compares every crate
  with the RustSec database of known security problems, unmaintained and
  withdrawn crates. Dependabot also sends alerts and pull requests with
  fixes, and `./maintenance.sh` runs the same audit locally.
- **GTK, libadwaita, libvirt, QEMU and the other system libraries** come from
  Fedora, Debian or Arch, so their security fixes arrive with normal system
  updates (`dnf upgrade`, `apt upgrade`, `pacman -Syu`), without a new app
  release.

When the security check turns red (GitHub emails you): the details name the
crate and the problem. Update the crate when a fixed version exists (or merge
Dependabot's pull request once its checks are green), run the tests, and
make a small release if the installed app is affected, since crates are built
into it. Without a fix, check whether the app uses the affected part; if not,
record a reviewed exception with the reason in `.cargo/audit.toml`.
"Unmaintained" warnings aren't urgent: plan a switch to a maintained crate.

## Running VMs

The app talks to libvirt. To run machines on the same computer, the libvirt
daemon and QEMU must be installed and running:

- Fedora: `sudo dnf install libvirt-daemon-kvm` (the RPM recommends it)
- Debian: `sudo apt install libvirt-daemon-system qemu-system-x86 qemu-utils
  qemu-system-modules-spice` (the package recommends these, so `apt`
  normally installs them). Debian keeps the disk tool (`qemu-utils`, needed to
  create disks) and SPICE support (`qemu-system-modules-spice`, for VM
  screens) in separate packages.

To manage the system-wide machines (`QEMU/KVM · System`) without a password
prompt each time, add yourself to the `libvirt` group and log in again:
`sudo usermod -aG libvirt $USER`.

### Guest details

Install the QEMU guest agent inside each machine so the app can show its IP
addresses, hostname, operating system and disk usage, and shut it down
cleanly:

- Fedora, RHEL-like: `sudo dnf install qemu-guest-agent`
- Debian, Ubuntu: `sudo apt install qemu-guest-agent`
- Windows: the guest agent in the virtio-win driver package

The agent answers while the machine runs. Machines created with this app
already have the channel it needs; for others, add it under Add Hardware →
Channel.

### Remote computers

Main menu → Add Remote Computer… manages another computer's machines over
SSH. The app logs in with your SSH key and never asks for a password, so set
the key up once and accept the computer's key in a terminal:

```sh
ssh-copy-id user@computer
ssh user@computer      # once, to accept the computer's key
```

The other computer needs libvirt running (and your user in its `libvirt`
group for the System machines). Screens are reached through SSH, so nothing
extra has to be opened in its firewall.

### 3D acceleration

Hardware → Video → **3D acceleration** lets a Linux guest draw with your
graphics card. It works on the **User session** connection. On the
**System** connection QEMU runs under SELinux confinement, which some
graphics drivers (such as NVIDIA's own) aren't allowed to run in; the machine
then doesn't start, and the app says so. Turn 3D acceleration off for that
machine, or run it on the User session.

### When something goes wrong

The app keeps a small log of failed operations, warnings and crashes in
`~/.local/state/virt-manager-nextgen/app.log`. Main menu → About →
Troubleshooting shows the app's version and the end of that log, with
buttons to copy or save it, which is what a bug report needs. The log stays
on your computer and only your user can read it.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
