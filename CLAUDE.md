# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

`PLAN.md` is the source of truth for goals, scope and the required feature list. Read it before starting any feature work.

Milestones (in order): 1 foundation ✔, 2 main window (VM list, details, host summary, title menu) ✔, 3 VM lifecycle + clone + snapshots ✔, 4 New VM wizard, 5 hardware details / Add Hardware / XML editing, 6 console (SPICE/VNC/serial), 7 host overview, networks, storage, Preferences, 8 graphs, guest agent, tray, 9 app icon + polish, then the RPM. Screens of later milestones show as disabled actions or "arrives in a later update" placeholders.

## Commands

All commands run inside the toolbox. The built binary runs on the host (its runtime libraries are installed there).

```sh
toolbox run -c virt-manager-nextgen cargo build                       # debug build
toolbox run -c virt-manager-nextgen cargo test --workspace            # unit tests (osinfo tests need osinfo-db, present in the toolbox)
toolbox run -c virt-manager-nextgen cargo clippy --workspace --all-targets
toolbox run -c virt-manager-nextgen cargo fmt --all
./target/debug/virt-manager-nextgen                                   # run on the host
toolbox run -c virt-manager-nextgen cargo run -p vmng-virt --example inspect -- qemu:///system   # dump what the libvirt layer sees
./target/debug/examples/selftest <empty-dir>   # end-to-end snapshot/clone/rename/delete on a throwaway qemu:///session VM (build with --example selftest)
```

UI review without touching the real desktop: start a headless compositor on the host (`mutter --headless --wayland --no-x11 --virtual-monitor 1400x900 --wayland-display=vmng-test &`), then run the debug binary with `WAYLAND_DISPLAY=vmng-test VMNG_SCREENSHOT=/path/out.png` (optional `VMNG_SCREENSHOT_DELAY=<secs>`); it renders the window to a PNG and quits (debug builds only, `crates/app/src/dev.rs`). `VMNG_DEV_SELECT=<vm name>` and `VMNG_DEV_ACTION=<action>` (e.g. `vm.snapshots`, `vm.clone`) open deeper screens first. Theme variants: `ADW_DEBUG_COLOR_SCHEME=prefer-dark|prefer-light`, `ADW_DEBUG_HIGH_CONTRAST=1`; for plain Adwaita (without Abyss) point `XDG_CONFIG_HOME` at a directory without `gtk-4.0/gtk.css`. Only one instance per app id runs per session, so take screenshots one at a time. For a running-VM test, use a transient domain on `qemu:///session` (`virsh create`) and destroy it afterwards; never start or change the user's own VMs.

## Architecture

Cargo workspace (edition 2024):

- `crates/xml` (`vmng-xml`): in-house XML element tree for libvirt XML. Preserves unknown content so edits never drop data; rejects DTDs, limits depth and size. No dependencies.
- `crates/virt` (`vmng-virt`): the only code that talks to libvirt (`virt` crate, plus `virt::sys` for bulk stats). Blocking API returning plain, `Send` Rust types: `Connection`, `DomainSummary`/`DomainConfig` (typed read-only view of domain XML), `DomainStats`, `HostStats`, `PoolSummary`, `GuestDetails`, `PowerAction`.
- `crates/osinfo` (`vmng-osinfo`): hand-written FFI to libosinfo (OS lookup, install-media detection). `OsDb` is `Send` but not `Sync`.
- `crates/app` (binary `virt-manager-nextgen`): GTK4 + libadwaita UI.
  - `backend/`: `Conn` runs libvirt calls on GLib's thread pool (`gio::spawn_blocking`) and is awaited from `glib::spawn_future_local`; `Monitor` turns polled counter samples into rates and 60-sample histories.
  - `osdb.rs`: loads osinfo-db once on its own thread; display names and logo keys for osinfo ids.
  - `settings.rs`: preferences in `~/.config/virt-manager-nextgen/settings.ini` (GLib `KeyFile`, no GSettings schema).
  - `ui/widgets/`: shared building blocks (OS logo tile, sparkline, status dot + label, usage meter, card/list helpers). New screens must reuse these.
  - `ui/window.rs` owns connection state, the poll timer, the `adw::NavigationView` (main page + per-VM page) and dispatches `vm.*` actions.
  - `ui/vm_actions.rs`: the single `vm.*` action group (enabled per VM state) plus the shared VM buttons and menus; every VM control uses these action names.
  - `ui/feedback.rs` (toasts, error toast with details, confirmations) and `ui/widgets/form_dialog.rs` (Cancel / primary / inline error) are the standard dialogs.
  - Machines view: `ui/vm_list.rs`, `ui/vm_details.rs`, `ui/title_menu.rs`. VM page: `ui/vm_page.rs` (tabs), `ui/snapshots.rs`. `ui/vm_dialogs.rs`: clone, rename, delete.
  - `style.css` is loaded at APPLICATION priority and uses only libadwaita CSS variables. Graph colours: `sparkline.cpu|memory|disk|network`.

Long-running operations (clone) show progress in the window's job bar; libvirt work reports progress through atomics polled by a GLib timeout.

Dependencies so far: `gtk4`, `libadwaita`, `virt` (all required to use the platform). Everything else is in-house.

## What is being built

A modern rewrite of virt-manager: a desktop front-end for QEMU/KVM through libvirt. It must cover virt-manager's core features plus the specific list in `PLAN.md`: connections, the New VM wizard, Add Hardware, VM lifecycle, snapshots and cloning, console, host networks and storage, preferences, and qemu-guest-agent support. The UI has to look like a polished commercial product, not a typical open-source GTK app. Visual and UX quality counts toward "done" just as much as feature parity.

## Hard constraints

- **Language and UI:** Rust with GTK4 + libadwaita. The user's custom theme (Abyss, in `~/.themes/Abyss`) is applied through `~/.config/gtk-4.0/gtk.css`, which overrides libadwaita's CSS color variables (`--window-bg-color`, `--accent-bg-color`, `--card-bg-color`, …) at user priority. That means libadwaita apps get the full theme, while plain GTK4 apps only get the theme's minimal `gtk-4.0/gtk.css`. Style only with libadwaita's named colors/variables, never hard-coded colors, and never load app CSS that outranks user CSS. Color rules are in `PLAN.md`.
- **Dependencies:** add an external crate only when it is truly necessary, for security, performance and modularity reasons. Prefer the standard library and in-house code. Justify every new dependency.
- **Code quality:** structure the code the way a professional Rust project would be structured, with a long-term maintainability focus. Security and performance are first-class concerns, especially since the app handles libvirt connections, XML and guest consoles.
- **Development environment:** all builds and dev libraries live in a **toolbox container**. Never install development packages on the host. The project's toolbox is `virt-manager-nextgen` (Fedora 44, with Rust, GTK4, libadwaita, libvirt, libosinfo and rpm-build dev packages installed). Run cargo and system-package commands inside it, for example `toolbox run -c virt-manager-nextgen cargo build`.
- **Packaging:** the end deliverable includes an RPM built from source inside the toolbox (rpmbuild with a `.spec` file kept in the repo). The host is Fedora 44, so the toolbox must use the same Fedora release, or the RPM's library dependencies won't match the host. The RPM must install the binary, a `.desktop` file, the app icon and AppStream metainfo, and declare runtime `Requires`. Once the build exists, document the single command that produces the RPM in this file.
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
