# Developing

Tools for working on the code: the checks that run against real libvirt,
and the debug-build hooks used to take screenshots of any screen without
touching the desktop. How to build packages is in [BUILDING.md](BUILDING.md);
the project's rules are in [CLAUDE.md](CLAUDE.md).

Every command below runs on the host unless it starts with `toolbox run`.
Build the examples first with
`toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo build --workspace --examples`.

## Checks against real libvirt

Each check creates throwaway `vmng-test-…` objects and deletes them again.
Checks without a connection argument use `qemu:///session`, or the
connection in `VMNG_TEST_URI`.

| Command | What it does |
|---|---|
| `toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo run -p vmng-virt --example inspect -- qemu:///system` | Dumps what the libvirt layer sees |
| `./target/debug/examples/selftest <empty-dir>` | Snapshot, clone, rename and delete on a `qemu:///session` VM |
| `./target/debug/examples/newvm_check qemu:///system default <iso>` | Creates, starts and deletes `vmng-test-newvm` |
| `./target/debug/examples/hw_check qemu:///system default <iso>` | Device add/edit/live update/remove on `vmng-test-hw` |
| `./target/debug/examples/net_pool_check qemu:///system <empty-dir>` | Creates, edits (also as XML) and removes `vmng-test-net` and `vmng-test-pool` (thin, fully allocated and backing-file volumes) |
| `./target/debug/examples/firmware_check` | `vmng-test-firmware` on `qemu:///session` with BIOS, UEFI and each UEFI firmware file, switching between them (`firmware.rs`) |
| `./target/debug/examples/tpm_check <win11.iso> [secs]` | Windows 11 style `vmng-test-tpm` (UEFI, Secure Boot, TPM 2.0); prints the running VM's TPM |
| `./target/debug/examples/cpu_check` | `vmng-test-cpu`: each CPU mode, mitigations and a manual layout, running (`cpu.rs`) |
| `./target/debug/examples/video3d_check` | `vmng-test-3d`: 3D acceleration on and off (`video.rs`; needs a GPU QEMU can use) |
| `./target/debug/examples/os_check` | `vmng-test-os`: changes the libosinfo OS id (`set_os_id`) |
| `./target/debug/examples/pxe_check` | `vmng-test-pxe`: network boot |
| `./target/debug/examples/options_check <empty-dir>` | `vmng-test-opts` and pool `vmng-test-optpool`: existing disk in the wizard, title/description, live memory, 9p/virtiofs read-only, disk options, MAC |
| `./target/debug/examples/remote_check <qemu+ssh://…>` | `vmng-test-remote` on another computer over SSH: create, autostart, VNC through the tunnel, text console, stats. Start a test computer with `tests/remote-host.sh start <dir> [port]` (Debian container with libvirt and sshd on 127.0.0.1), stop it with `tests/remote-host.sh stop` |
| `./target/debug/examples/identify <iso or url>…` | OS detection (an http(s) address reads a network install tree) |

## Randomized tests

`crates/xml/tests/fuzz.rs` feeds the XML parser mutated libvirt documents,
random bytes and generated trees, and checks that what the writer produces
parses back. It runs with the normal tests; for a longer run:

```sh
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo VMNG_FUZZ_ITERS=1000000 cargo test --release -p vmng-xml --test fuzz
```

A failure names its seed; `VMNG_FUZZ_SEED=<n> VMNG_FUZZ_ITERS=1` repeats it.

## Screenshots without touching the desktop

Start a headless compositor, then run the debug build with
`VMNG_SCREENSHOT`: it renders the window to a PNG and quits (debug builds
only, `crates/app/src/dev.rs`).

```sh
virsh -c qemu:///session list   # first: see "session daemon" below
mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 --wayland-display=vmng-test &
WAYLAND_DISPLAY=vmng-test VMNG_SCREENSHOT=/path/out.png ./target/debug/virt-manager-nextgen
```

Rules:

- **Session daemon:** the `qemu:///session` daemon is started by its first
  client and inherits that client's environment. If a screenshot run with a
  test `XDG_CONFIG_HOME` starts it, it keeps its configuration in that
  folder (and long paths break QEMU's monitor socket). Run
  `virsh -c qemu:///session list` first.
- Screenshot runs are non-unique, so they never hand over to an instance
  that is open; still take them one at a time. Never `pkill` the app by
  name, and don't `pgrep -f` for a string that is also in your own command
  line (it matches itself).
- Screenshot runs never restore or save the window size, view or selected
  VM, unless `VMNG_DEV_REMEMBER=1` (use a test `XDG_CONFIG_HOME` then).
- For a running VM, use a transient domain on `qemu:///session`
  (`virsh create`, `--paused` then `virsh resume` to catch BIOS serial
  output) and destroy it afterwards. Never start or change real VMs.
- A VM made with `VMNG_DEV_WIZARD_CREATE` is removed with
  `virsh undefine <name> --nvram --storage vda`, never
  `--remove-all-storage` (that would delete the ISO too).
- Debug builds print error toasts and form errors to stderr (`error: …`,
  `form error: …`) for unattended runs.

Themes: `ADW_DEBUG_COLOR_SCHEME=prefer-dark|prefer-light`,
`ADW_DEBUG_HIGH_CONTRAST=1`; for plain Adwaita without the Abyss theme,
point `XDG_CONFIG_HOME` at a folder without `gtk-4.0/gtk.css`.

### Hooks

| Variable | Effect |
|---|---|
| `VMNG_SCREENSHOT=<file.png>`, `VMNG_SCREENSHOT_DELAY=<secs>` | Take the screenshot (after a delay) and quit |
| `VMNG_DEV_SHOT_POPOVER=1` | Capture the open popup instead of the window |
| `VMNG_DEV_SIZE=<w>x<h>` | Window size, e.g. `360x700` |
| `VMNG_DEV_DETAIL=1` | Narrow list/detail layouts start on the details |
| `VMNG_DEV_OVERFLOW=1`, `VMNG_DEV_OVERFLOW_ALL=1` | Print widgets narrower than their minimum (all: everything wider than 360 px) |
| `VMNG_DEV_TREE=<css class>` | Print the sizes of that widget's ancestors and children |
| `VMNG_DEV_SELECT=<vm name>` | Select a VM |
| `VMNG_DEV_ACTION=<action>` | Activate an action, e.g. `vm.snapshots`, `vm.clone`, `win.new-vm`, `win.add-connection` |
| `VMNG_DEV_VIEW=host\|networks\|storage` | Open a view |
| `VMNG_DEV_MAIN_MENU=1` | Open the main menu |
| `VMNG_DEV_VM_MENU=1` | Right-click menu of the selected VM |
| `VMNG_DEV_CONFIRM=1` | Answer yes to confirmations |
| `VMNG_DEV_FORM_SUBMIT=1` | Press the primary button of any form dialog |
| `VMNG_DEV_FORM_NAME=<name>` | New Network name (always `vmng-test-…`) |
| `VMNG_DEV_REMEMBER=1` | Restore and save the window state anyway |
| `VMNG_DEV_LOG=1` | Write the log file in a screenshot run (use a test `XDG_STATE_HOME`) |
| **New VM wizard** | |
| `VMNG_DEV_WIZARD_STEP=<0-3>` | Open a step |
| `VMNG_DEV_WIZARD_CHOOSE=1` | Open the OS chooser |
| `VMNG_DEV_WIZARD_MEDIA=<text>` | Pick the installer image whose name contains it |
| `VMNG_DEV_WIZARD_URL=<address>` | Network install from that tree; with `VMNG_DEV_WIZARD_CREATE` it waits for the tree, downloads the installer and starts it |
| `VMNG_DEV_WIZARD_PXE=1` | Manual install with network boot |
| `VMNG_DEV_WIZARD_EXISTING=<image>` | Storage step: use an existing disk image |
| `VMNG_DEV_WIZARD_CREATE=<name>` | Create the VM (no network; a `vmng-test-…` name) |
| **Hardware tab** | |
| `VMNG_DEV_HW_ROW=<n>` | Select a list row (negative counts from the end) |
| `VMNG_DEV_HW_MENU=1` | Right-click menu of that row |
| `VMNG_DEV_HW_ADD=1`, `VMNG_DEV_HW_ADD_CLICK=1` | Open Add Hardware, press Add |
| `VMNG_DEV_HW_REMOVE=1` | Remove the selected device |
| `VMNG_DEV_HW_MEMORY=<MiB>` | Edit memory and Apply (saves: `vmng-test-…` VMs only) |
| `VMNG_DEV_HW_ALL_CLEAN=1` | Open every page; panic if merely looking counts as a change |
| `VMNG_DEV_AUTOSTART=1` | Boot & firmware: turn autostart on |
| `VMNG_DEV_CDROM_FILE=<path>` | CD-ROM editor: act as if that file was chosen |
| **Console** | |
| `VMNG_DEV_CONSOLE_KEYS=<evdev,…>` | Send keys |
| `VMNG_DEV_CONSOLE_TEXT=1` | Open the text console |
| `VMNG_DEV_CONSOLE_FULLSCREEN=1` | Fullscreen |
| `VMNG_DEV_CONSOLES=<name>,<name>` | Open several console windows (the screenshot captures the newest) |
| **Networks, storage, remote** | |
| `VMNG_DEV_NET_XML=1` | Networks: open and scroll to the XML |
| `VMNG_DEV_NET_EDIT=<vmng-test-… network>` | Add an MTU in its XML and Apply |
| `VMNG_DEV_NEW_VOLUME=1` | Storage: open New Volume |
| `VMNG_DEV_REMOTE=<qemu+ssh://…>` | Fill Add Remote Computer (with the test key); with `VMNG_DEV_ACTION=win.add-connection` and `VMNG_DEV_FORM_SUBMIT=1` it connects |
