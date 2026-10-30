## Goal 
The goal is to create replicate virt-manager that has modern look but has most of the same features, I will list what features I want as I don't need all but I want all the same core features.

## Constraints 
It has to be written in rust and for the GUI/interface I want it to support GTK4. For modularity, performance and security I want to only use external libraries when absolutely necessary. I want to code to be structured like a professional rust programmer would because I want professional grade looking program and also the code itself should be professionally setup and structured so that it's easy to maintain on the long term. Keep security and performance also in mind when writing the code, we want a secure and performant next generation virt-manager. 

## Scope
The development should be done in a toolbox container as I don't want any development libraries installed on my system itself. The GUI/Interface has to look like it's been designed by a professional graphical designer, modern, easy to use and logical. If I had to use a word it should also look like a GUI/Interface that could be sold as a product created by a bit Tech company.

## Definition of done
The end result should be that it has all the same features as the current virt-manager, so it's compatible with a Qemu/kvm/libvirt setup, so that I can use that as a front-end for managing my vm's. Also as stated before, done is also that the GUI/interface looks like it's designed by a professional UX designer so that it looks like professionally designed software and not like some ugly looking opensource project who most GUI/interfaces look ugly.

I want the the same core features as the current virt-manager and also the following features should be included.

Connections / hypervisors
- QEMU/KVM, either system-wide or a per-user session
- Connecting automatically at startup
- Remote QEMU/KVM hosts over SSH, system-wide or a user session, with SSH key login

New VM wizard
- virtual machine only
- Install methods:
  - Local ISO image or CD-ROM
  - Network install from an install tree, or network boot (PXE)
  - Import an existing disk image
  - Manual install
- Choose the architecture, machine type and virtualization type
- Detect the guest OS automatically from the install media, using libosinfo and osinfo-db 20251212
- Set memory, CPUs and storage
- Edit the configuration before the install starts

Add Hardware (device types)
- Disks and controllers: Storage, Controller, Filesystem (shared folders)
- Network: Network
- Display and input: Graphics (SPICE or VNC), Video, Input, Sound
- Character devices: Serial, Console, Channel

VM management
- No permanent sidebar: GNOME-style header tabs switch between Machines, Host, Networks and Storage (a bar at the bottom in narrow windows); the main menu switches between connections (system / user session), so the VM list gets the full window
- The VM list shows a modern-looking logo for each VM, matching its guest OS (for example Fedora, Ubuntu, Windows), with a clean generic icon when the OS is unknown
- Run, pause, shut down, reboot, force off, save, delete, and optionally delete the VM's storage
- Start a VM when the host boots (autostart)
- Clone VMs
- Take, restore and manage snapshots
- Edit hardware details for every device
- Edit the raw libvirt XML (turned on in Preferences)
- Performance graphs for guest and host CPU, memory, disk I/O and network I/O
- System tray icon. The tray library isn't installed (AyatanaAppIndicator3 is missing), so this may not work.

Console
- Graphical console over SPICE or VNC, plus a serial text console
- Fullscreen and scaling (always, never, or only in fullscreen)
- Resize the guest display to match the window, or the window to match the VM
- Send key combinations, with configurable grab keys
- Take screenshots
- Connect to the console automatically

Host / connection details
- Virtual networks: create, start, stop, edit
- Storage pools and volumes: create, browse, delete
- Host overview of CPU, memory and usage

Preferences
- Turn on the tray icon and XML editing (libguestfs inspection dropped: the qemu-guest-agent covers it)
- Choose which stats are collected and how often
- Defaults for new VMs: graphics type, disk format, CPU model, and BIOS or UEFI firmware
- Turn individual confirmation prompts on or off

Other features
- It should work with qemu-guest-agent in a guest os.
- Follow the system theme: light/dark mode and the system accent color are applied automatically, and custom GTK themes installed on the system must work too
- A compact host summary (CPU usage, memory used / total and by running VMs, default storage pool free space) is always visible below the VM list; clicking it opens the Host overview
- Consistency: every screen follows the same patterns: the same layout, spacing, button placement (primary action in the same spot), wording, icons, dialogs and confirmation style. A new screen reuses existing building blocks rather than inventing its own look.
- Colors (agreed design rules):
  - No own color scheme: all colors come from the active GTK theme (custom themes included)
  - Mostly neutral surfaces; the accent color is only used for the selected item, the one primary button per screen, and switches that are on
  - Status colors always mean the same: green = running, yellow = paused/warning, red = error/destructive, gray = shut off; always paired with a word or icon
  - OS logos are the main color in the VM list
  - Graphs use fixed colors everywhere: CPU = accent, memory = green, disk = orange, network = purple
  - Text must meet contrast requirements in light, dark, high-contrast and custom themes
- Usually fewer than 10 VMs (max ~10). The main window shows the VMs as a list; clicking a VM shows its details (console preview, controls, resources, storage used per disk, guest agent info) in a panel next to the list

Packaging / installation
- Build an RPM from the source code inside the toolbox container, so the app can be installed on the host with a single command (`sudo dnf install ./<package>.rpm`) and removed cleanly with dnf
- The RPM installs the app with its menu entry and icon so it shows up in the GNOME app grid, and pulls in its runtime dependencies (GTK4, libadwaita, libvirt, libosinfo, osinfo-db, …) automatically

Done after the first round
- App icon ("machines inside a machine", teal), with a white or colored tray icon (Preferences)
- Polish: animations, keyboard shortcuts with a Keyboard Shortcuts dialog, every screen working down to phone width
- Live screen preview in the VM details; each console in its own window (several at once)
- The window remembers its size, maximized state, view and selected VM
- Stricter GNOME look: a plain "+" button, standard header tabs (main window and VM page), connection choice in the main menu

Done after the virt-manager comparison (0.3.0)
- CPU: any CPU model the host runs, "copy host CPU", security mitigations, manual CPU layout (sockets, cores, threads)
- 3D acceleration for VirtIO video (through an egl-headless display, shown by every console)
- Snapshots: inside the disk or as separate files, a folder for the memory file, a picture of the screen
- Changing a machine's operating system type
- New VM: network install from an HTTP(S) install tree, and network boot (PXE)

Done after the virt-manager comparison, second round (0.4.0)
- Title and description per VM (Overview in the hardware list; shown right away on a running machine)
- Current and maximum memory (raise it while running, through the guest's balloon driver)
- Shared folders: read-only, and virtiofs or 9p
- Disk: removable (USB/SCSI), detect zeroes, I/O mode
- Network card: editable MAC address, direct connection (macvtap) on a wired host card that isn't part of a bridge
- New Volume: allocate the whole size now, based on another image (backing file)
- New VM wizard: use an existing disk image as the machine's disk (the installer still starts), or no disk
- Networks: view and edit a network's XML (with "Allow editing the raw XML")
- Preferences → General → Appearance: follow the system, light or dark
- Other computers over SSH (Add Remote Computer in the main menu; SSH key login; screens through an SSH tunnel)
- Starting a machine when the host boots (Boot & firmware → Startup)
- Regression tests: one per fixed bug (crates/virt/tests/regressions.rs, locally and on GitHub) and a "viewing changes nothing" check of every hardware page

Done after the health review (not released yet)
- Log file for bug reports (~/.local/state/virt-manager-nextgen/app.log), shown in About → Troubleshooting
- Console: impossible screen and pointer sizes from a VM are refused instead of crashing
- Network install: a secure (https) download can't be redirected to an insecure (http) one
- Randomized stress tests for the XML reader and writer; GitHub checks the minimum Rust version

Still open from the comparison (the user decides later)
- Not built: connections over TLS or with a password typed into the app, hypervisors other than QEMU/KVM
- Not recommended for a desktop: direct kernel boot, rare devices (parallel, watchdog, smartcard, panic, vsock, mediated devices), more pool types (disk, ZFS, Ceph/RBD, Gluster, multipath, SCSI host adapters), TPM 1.2 / passthrough

Decided against (desktop use only)
- USB host devices / USB redirection, PCI passthrough, migration, libguestfs (remote hosts over SSH were later added on request)

## Autonomy 
I don't know rust, so don't ask me about programming decisions, only ask me about features and when something needs to be tested. If you really need to ask something about rust or programming then explain it to me in easy to understand language so that with that information I can give you a good answer to help reach the end result software that I want to create.
