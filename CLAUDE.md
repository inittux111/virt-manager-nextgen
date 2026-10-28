# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project status

The repository is pre-implementation. Only `PLAN.md` exists, and it is the source of truth for goals, scope and the required feature list. Read it before starting any feature work. Once the Rust project is scaffolded, update this file with the real build, lint and test commands and a short architecture overview.

## What is being built

A modern rewrite of virt-manager: a desktop front-end for QEMU/KVM through libvirt. It must cover virt-manager's core features plus the specific list in `PLAN.md`: connections, the New VM wizard, Add Hardware, VM lifecycle, snapshots and cloning, console, host networks and storage, preferences, and qemu-guest-agent support. The UI has to look like a polished commercial product, not a typical open-source GTK app. Visual and UX quality counts toward "done" just as much as feature parity.

## Hard constraints

- **Language and UI:** Rust with GTK4 + libadwaita. The user's custom theme (Abyss, in `~/.themes/Abyss`) is applied through `~/.config/gtk-4.0/gtk.css`, which overrides libadwaita's CSS color variables (`--window-bg-color`, `--accent-bg-color`, `--card-bg-color`, …) at user priority. That means libadwaita apps get the full theme, while plain GTK4 apps only get the theme's minimal `gtk-4.0/gtk.css`. Style only with libadwaita's named colors/variables, never hard-coded colors, and never load app CSS that outranks user CSS. Color rules are in `PLAN.md`.
- **Dependencies:** add an external crate only when it is truly necessary, for security, performance and modularity reasons. Prefer the standard library and in-house code. Justify every new dependency.
- **Code quality:** structure the code the way a professional Rust project would be structured, with a long-term maintainability focus. Security and performance are first-class concerns, especially since the app handles libvirt connections, XML and guest consoles.
- **Development environment:** all builds and dev libraries live in a **toolbox container**. Never install development packages on the host. Run cargo and system-package commands inside the toolbox (for example `toolbox run -c <name> cargo build`).
- **Packaging:** the end deliverable includes an RPM built from source inside the toolbox (rpmbuild with a `.spec` file kept in the repo). The host is Fedora 44, so the toolbox must use the same Fedora release, or the RPM's library dependencies won't match the host. The RPM must install the binary, a `.desktop` file, the app icon and AppStream metainfo, and declare runtime `Requires`. Once the build exists, document the single command that produces the RPM in this file.
- **Guest OS detection:** uses libosinfo with osinfo-db `20251212`.
- **System tray:** AyatanaAppIndicator3 is not installed, so tray support may not work. Treat it as optional and degrade gracefully.

## UI design

The approved UI mockup is the design reference: https://claude.ai/artifact/R9CVZEsdNRp2Fx4fCuPz3w (read it with the Artifact tool, not a web fetch). It covers the main window (VM list + details panel + host summary, title-menu view/connection switcher), console, VM hardware and snapshots pages, Add Hardware, Preferences, New VM wizard, host overview, network and storage lists and settings, and empty/error/progress/confirmation states. Layout and behavior decisions are also recorded in `PLAN.md`.

The mockup is HTML that imitates libadwaita. Its colors are hand-copied theme values for previewing only. In the real app every color must come from the theme. Check UI work in Adwaita light, Adwaita dark, High Contrast and the user's Abyss theme.

## Skills

When coding, use the coding skills that are available, including the Superpowers plugin skills (for example planning, test-driven development, debugging and code review workflows). Check the available skills before starting a task and follow any that apply.

## Working with the user

The user does not know Rust. Do not ask them about programming or implementation decisions; make those yourself. Ask only about features, and tell them when something is ready to be tested on their real libvirt setup. If a technical question can't be avoided, explain it in plain, non-technical language so they can give an informed answer.
