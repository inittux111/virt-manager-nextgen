# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

`PLAN.md` is the source of truth for goals, scope and the required feature list. Read it before starting any feature work.

Milestones (in order): 1 foundation ✔, 2 main window (VM list, details, host summary, title menu) ✔, 3 VM lifecycle + clone + snapshots ✔, 4 New VM wizard ✔, 5 hardware details / Add Hardware / XML editing ✔, 6 console (SPICE/VNC/serial) ✔, 7 host overview, networks, storage, Preferences ✔, 8 graphs, guest agent, tray ✔, 9 app icon + polish ✔, RPM ✔. Screens of later milestones show as disabled actions or "arrives in a later update" placeholders.

## Commands

All commands run inside the toolbox. Toolbox shares the home directory with the host, so cargo must use the container-only `CARGO_HOME=/opt/cargo` (created inside the container; plain `toolbox run` does not read `/etc/profile.d`, so pass it explicitly as below). Never let cargo write to `~/.cargo` on the host. The built binary in `target/` (inside the repo, gitignored) runs on the host because its runtime libraries are installed there.

```sh
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo build                       # debug build
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo test --workspace            # unit tests (osinfo tests need osinfo-db, present in the toolbox)
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo clippy --workspace --all-targets
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo fmt --all
./target/debug/virt-manager-nextgen                                   # run on the host
./build-rpm.sh [minor|major|X.Y.Z|--rebuild]                         # the RPM, run on the host: bumps the version (packaging/new-version.sh: commit + tag vX.Y.Z), builds in the toolbox (creates it if missing) from HEAD → target/rpmbuild/RPMS/x86_64/
./maintenance.sh                                                      # maintenance check on the host: toolbox dnf upgrade, cargo update, cargo audit (installed in /opt/cargo/bin), build, clippy -D warnings, tests, session selftest; commits Cargo.lock if all pass
sudo dnf install ./target/rpmbuild/RPMS/x86_64/virt-manager-nextgen-*.rpm   # install it on the host (the user runs this)
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo run -p vmng-virt --example inspect -- qemu:///system   # dump what the libvirt layer sees
./target/debug/examples/newvm_check qemu:///system default <iso>   # create/start/delete a throwaway vmng-test-newvm
./target/debug/examples/hw_check qemu:///system default <iso>   # device add/edit/live-update/remove on a throwaway vmng-test-hw
./target/debug/examples/net_pool_check qemu:///system <empty-dir>   # create/edit/remove vmng-test-net and vmng-test-pool
./target/debug/examples/identify <iso>…   # OS detection (vmng-osinfo example)
./target/debug/examples/selftest <empty-dir>   # end-to-end snapshot/clone/rename/delete on a throwaway qemu:///session VM (build with --example selftest)
```

UI review without touching the real desktop: start a headless compositor on the host (`mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 --wayland-display=vmng-test &`), then run the debug binary with `WAYLAND_DISPLAY=vmng-test VMNG_SCREENSHOT=/path/out.png` (optional `VMNG_SCREENSHOT_DELAY=<secs>`); it renders the window to a PNG and quits (debug builds only, `crates/app/src/dev.rs`). `VMNG_DEV_SELECT=<vm name>` and `VMNG_DEV_ACTION=<action>` (e.g. `vm.snapshots`, `vm.clone`, `win.new-vm`) open deeper screens first; in the wizard `VMNG_DEV_WIZARD_STEP=<0-3>`, `VMNG_DEV_WIZARD_CHOOSE=1` (OS chooser) and `VMNG_DEV_HW_ROW=<n>` (hardware list row, negative = from the end), `VMNG_DEV_HW_ADD=1` / `VMNG_DEV_HW_ADD_CLICK=1` (Add Hardware, press Add), `VMNG_DEV_HW_MEMORY=<MiB>` (edit + Apply: it saves, so only on a `vmng-test-…` VM), `VMNG_DEV_CONSOLE_KEYS=<evdev,…>` / `VMNG_DEV_CONSOLE_TEXT=1` / `VMNG_DEV_CONSOLE_FULLSCREEN=1` (console input / text console / fullscreen; test with a transient `virsh create` VM, `--paused` then `virsh resume` to catch BIOS serial output), `VMNG_DEV_VIEW=host|networks|storage`, `VMNG_DEV_FORM_SUBMIT=1` (presses the primary button of any form dialog), `VMNG_DEV_FORM_NAME=<name>` (New Network name; always a `vmng-test-…` name), `VMNG_DEV_WIZARD_CREATE=<name>` (creates a VM without network: use a `vmng-test-…` name and delete it afterwards with `virsh undefine <name> --nvram --storage vda`, never `--remove-all-storage`, which would also delete the ISO). Theme variants: `ADW_DEBUG_COLOR_SCHEME=prefer-dark|prefer-light`, `ADW_DEBUG_HIGH_CONTRAST=1`; for plain Adwaita (without Abyss) point `XDG_CONFIG_HOME` at a directory without `gtk-4.0/gtk.css`. Screenshot runs (`VMNG_SCREENSHOT` set) are non-unique, so they never hand over to an instance the user has open; still take them one at a time. Never `pkill` the app by name: the user may be running it. Don't `pgrep -f` for a string that also appears in your own command line (it matches itself). `VMNG_DEV_HW_REMOVE=1` removes the selected hardware device via its menu action. Small windows: `VMNG_DEV_SIZE=<w>x<h>` (e.g. `360x700`), `VMNG_DEV_DETAIL=1` (narrow list/detail layouts start on the details), `VMNG_DEV_OVERFLOW=1` (prints widgets narrower than their minimum; add `VMNG_DEV_OVERFLOW_ALL=1` to list everything wider than 360 px). `VMNG_DEV_TREE=<css class>` prints the sizes of that widget's ancestors and children. `VMNG_DEV_CONSOLES=<name>,<name>` opens several console windows (screenshots capture the newest window). Right-click menus: `VMNG_DEV_VM_MENU=1` (with `VMNG_DEV_SELECT`) or `VMNG_DEV_HW_MENU=1` (with `VMNG_DEV_HW_ROW`), plus `VMNG_DEV_SHOT_POPOVER=1` to capture the popup instead of the window. For a running-VM test, use a transient domain on `qemu:///session` (`virsh create`) and destroy it afterwards; never start or change the user's own VMs.

## Architecture

Cargo workspace (edition 2024):

- `crates/xml` (`vmng-xml`): in-house XML element tree for libvirt XML. Preserves unknown content so edits never drop data; rejects DTDs, limits depth and size. No dependencies.
- `crates/virt` (`vmng-virt`): the only code that talks to libvirt (domains, devices, snapshots, networks, storage pools/volumes, consoles) (`virt` crate, plus `virt::sys` for bulk stats). Blocking API returning plain, `Send` Rust types: `Connection`, `DomainSummary`/`DomainConfig` (typed read-only view of domain XML), `DomainStats`, `HostStats`, `PoolSummary`, `GuestDetails`, `PowerAction`.
- `crates/osinfo` (`vmng-osinfo`): hand-written FFI to libosinfo (OS lookup, install-media detection). `OsDb` is `Send` but not `Sync`.
- `crates/display` (`vmng-display`): hand-written FFI to spice-client-glib-2.0, gvnc-1.0 and vte-2.91-gtk4 (all GObject C libraries; signals/properties go through glib-rs `connect_local`). `GuestScreen` widget (copies the protocol's framebuffer into a texture with dirty regions, scaling, keyboard grab with release keys), `SpiceBackend` (session opened on a libvirt `open_graphics_fd` socket; extra channels get sockets via `open-fd` + `FdProvider`), `VncBackend` (we own the framebuffer memory), `Terminal` (VTE). `keymap.rs`: evdev → XT scan codes.
- `crates/app` (binary `virt-manager-nextgen`): GTK4 + libadwaita UI.
  - `backend/`: `Conn` runs libvirt calls on GLib's thread pool (`gio::spawn_blocking`) and is awaited from `glib::spawn_future_local`; `Monitor` turns polled counter samples into rates and 60-sample histories.
  - `osdb.rs`: loads osinfo-db once on its own thread; display names and logo keys for osinfo ids.
  - `settings.rs`: preferences in `~/.config/virt-manager-nextgen/settings.ini` (GLib `KeyFile`, no GSettings schema).
  - `ui/widgets/`: shared building blocks (OS logo tile, sparkline, status dot + label, usage meter, card/list helpers). New screens must reuse these. `adaptive.rs`: `ListDetail` (list beside details; one pane at a time with a back button when narrow), `reflow` (side-by-side boxes stack into a column), `scrolled_page`. Every screen must work down to 360 px wide.
  - `ui/shortcuts.rs`: the one table of keyboard shortcuts (installs the accelerators and fills the Keyboard Shortcuts dialog). App shortcuts are switched off while the console has the keyboard.
  - `ui/window.rs` owns connection state, the poll timer, the `adw::NavigationView` (main page + per-VM page) and dispatches `vm.*` actions.
  - `ui/vm_actions.rs`: the single `vm.*` action group (enabled per VM state) plus the shared VM buttons and menus; every VM control uses these action names.
  - `ui/feedback.rs` (toasts, error toast with details, confirmations) and `ui/widgets/form_dialog.rs` (Cancel / primary / inline error) are the standard dialogs.
  - New VM wizard: `ui/new_vm.rs` (four steps, OS defaults from osinfo, the dialog owns the wizard until closed) and `ui/os_chooser.rs`. The XML it produces comes from `vmng_virt::build_domain_xml` (disk boots first, installer second, so the installer is skipped once the OS is installed).
  - Hardware tab: `ui/hardware/` (`mod.rs` list + editor + Apply/Revert bar, right-click/Delete → Remove; popovers are parented outside `GtkListBox`, whose `remove_all` loops on non-row children over a working copy of the full domain XML; `editors.rs` per-device editors bound through `Bind`; `add.rs` Add Hardware; `labels.rs` names/icons). Saving redefines the whole persistent XML (`save_domain_config` refuses UUID/name changes); CD and link-state changes go live via `update_device`; add/remove use attach/detach with LIVE+CONFIG falling back to CONFIG.
  - Console: `ui/console.rs` (`ConsoleWindow`: one window per VM, several can be open; the main window keeps them by UUID and updates them each poll; each has its own `vm.*` action group aimed at its VM; screen or text console, floating toolbar, fullscreen, scaling, resize-guest, screenshots). Serial output: reader thread → shared buffer drained by a 16 ms GLib timeout; input: writer thread.
  - Other views (title menu): `ui/host_view.rs`, `ui/networks_view.rs` (list + settings + Apply bar, New Network dialog), `ui/storage_view.rs` (pools, volumes, New Pool/New Volume). They load when shown (`MainWindow::load_view`); the header's main button follows the view (New VM / New Network). `ui/preferences.rs`: `adw::PreferencesDialog`, every change saved at once through `Settings::update`.
  - `ui/performance.rs` (VM page Performance tab, `widgets/chart.rs`), guest agent details via `virDomainGetGuestInfo` (`vmng-virt/src/guest.rs`).
  - `tray.rs`: tray icon speaking StatusNotifierItem + com.canonical.dbusmenu directly over GIO D-Bus (no AppIndicator library; GNOME needs the AppIndicator extension, detected via `org.kde.StatusNotifierWatcher`). With the tray on, closing the window hides it.
  - Machines view: `ui/vm_list.rs`, `ui/vm_details.rs`, `ui/title_menu.rs`. VM page: `ui/vm_page.rs` (tabs), `ui/snapshots.rs`. `ui/vm_dialogs.rs`: clone, rename, delete.
  - `style.css` is loaded at APPLICATION priority and uses only libadwaita CSS variables. Graph colours: `sparkline.cpu|memory|disk|network`.

Long-running operations (clone) show progress in the window's job bar; libvirt work reports progress through atomics polled by a GLib timeout.

Dependencies so far: `gtk4`, `libadwaita`, `virt` (all required to use the platform). Everything else is in-house. System libraries linked: libvirt, libosinfo, spice-client-glib, gvnc, vte-2.91-gtk4 (dev packages `spice-glib-devel gvnc-devel vte291-gtk4-devel` are in the toolbox; the RPM must `Requires` the runtime packages).

## What is being built

A modern rewrite of virt-manager: a desktop front-end for QEMU/KVM through libvirt. It must cover virt-manager's core features plus the specific list in `PLAN.md`: connections, the New VM wizard, Add Hardware, VM lifecycle, snapshots and cloning, console, host networks and storage, preferences, and qemu-guest-agent support. The UI has to look like a polished commercial product, not a typical open-source GTK app. Visual and UX quality counts toward "done" just as much as feature parity.

## Hard constraints

- **Language and UI:** Rust with GTK4 + libadwaita. The user's custom theme (Abyss, in `~/.themes/Abyss`) is applied through `~/.config/gtk-4.0/gtk.css`, which overrides libadwaita's CSS color variables (`--window-bg-color`, `--accent-bg-color`, `--card-bg-color`, …) at user priority. That means libadwaita apps get the full theme, while plain GTK4 apps only get the theme's minimal `gtk-4.0/gtk.css`. Style only with libadwaita's named colors/variables, never hard-coded colors, and never load app CSS that outranks user CSS. Color rules are in `PLAN.md`.
- **Dependencies:** add an external crate only when it is truly necessary, for security, performance and modularity reasons. Prefer the standard library and in-house code. Justify every new dependency.
- **Code quality:** structure the code the way a professional Rust project would be structured, with a long-term maintainability focus. Security and performance are first-class concerns, especially since the app handles libvirt connections, XML and guest consoles.
- **Development environment:** all builds and dev libraries live in a **toolbox container**. Never install development packages on the host. The project's toolbox is `virt-manager-nextgen` (Fedora 44, with Rust, GTK4, libadwaita, libvirt, libosinfo and rpm-build dev packages installed). Run cargo and system-package commands inside it, for example `toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo build`.
- **Packaging:** the end deliverable includes an RPM built from source inside the toolbox (rpmbuild with a `.spec` file kept in the repo). The host is Fedora 44, so the toolbox must use the same Fedora release, or the RPM's library dependencies won't match the host. The RPM must install the binary, a `.desktop` file, the app icon and AppStream metainfo, and declare runtime `Requires`. The single command that produces it is `./build-rpm.sh` on the host (it runs `packaging/build-rpm.sh` in the toolbox) (spec: `packaging/virt-manager-nextgen.spec`; it vendors the crates and builds offline; versions are bumped by `packaging/new-version.sh`, never by hand). The user-facing guide is `BUILDING.md`; keep it in step with the scripts. `.cargo/config.toml` lets `cargo update` take crate releases newer than `rust-version` (the toolbox always has Fedora's current Rust). Dependabot alerts are enabled on the (private) GitHub repo; when the user asks for a "maintenance check", run `./maintenance.sh`, fix what it reports, and commit.
- **Guest OS detection:** uses libosinfo with osinfo-db `20251212`.
- **System tray:** AyatanaAppIndicator3 is not installed, so tray support may not work. Treat it as optional and degrade gracefully.

## UI design

The approved UI mockup is the design reference: https://claude.ai/artifact/R9CVZEsdNRp2Fx4fCuPz3w (read it with the Artifact tool, not a web fetch). It covers the main window (VM list + details panel + host summary, title-menu view/connection switcher), console, VM hardware and snapshots pages, Add Hardware, Preferences, New VM wizard, host overview, network and storage lists and settings, and empty/error/progress/confirmation states. Layout and behavior decisions are also recorded in `PLAN.md`.

**Consistency:** every screen follows the same patterns: the same layout, spacing, button placement (primary action in the same spot), wording, icons, dialogs and confirmation style. A new screen reuses existing building blocks (shared widgets/components in code) rather than inventing its own look.

The mockup is HTML that imitates libadwaita. Its colors are hand-copied theme values for previewing only. In the real app every color must come from the theme. Check UI work in Adwaita light, Adwaita dark, High Contrast and the user's Abyss theme.

## Skills

When coding, use the coding skills that are available, including the Superpowers plugin skills (for example planning, test-driven development, debugging and code review workflows). Check the available skills before starting a task and follow any that apply.

## Working with the user

The user does not know Rust. Do not ask them about programming or implementation decisions; make those yourself. Ask only about features, and tell them when something is ready to be tested on their real libvirt setup. If a technical question can't be avoided, explain it in plain, non-technical language so they can give an informed answer.
