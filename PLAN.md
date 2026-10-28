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

New VM wizard
- virtual machine only
- Install methods:
  - Local ISO image or CD-ROM
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
- Run, pause, shut down, reboot, force off, save, delete, and optionally delete the VM's storage
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
- Turn on the tray icon, libguestfs inspection and XML editing
- Choose which stats are collected and how often
- Defaults for new VMs: graphics type, disk format, CPU model, and BIOS or UEFI firmware
- Turn individual confirmation prompts on or off

Other features
- It should work with qemu-guest-agent in a guest os.

## Autonomy 
I don't know rust, so don't ask me about programming decisions, only ask me about features and when something needs to be tested. If you really need to ask something about rust or programming then explain it to me in easy to understand language so that with that information I can give you a good answer to help reach the end result software that I want to create.
