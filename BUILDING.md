# Building the RPM

The app is installed as an RPM package. `build-rpm.sh` makes a new one.
It builds inside the `virt-manager-nextgen` toolbox container, so nothing
except the finished package ends up on your computer.

## Build the next version

From the project folder (`~/Git/claude/virt-manager-nextgen`):

```sh
./build-rpm.sh
sudo dnf install ./target/rpmbuild/RPMS/x86_64/virt-manager-nextgen-<version>-1.fc44.x86_64.rpm
```

The script prints the exact `dnf` command at the end, so you can copy it
from there. `dnf install` upgrades the app you already have. Your settings
stay as they are.

## What the script does

1. **Creates the toolbox container if it's missing.** It uses the same
   Fedora release as your computer and installs the build tools into the
   container. This happens only the first time, or after the container
   has been removed.
2. **Picks the next version number** and saves it in git, together with a
   "what's new" list made from the changes since the previous version.
   That list ends up in the package's changelog and in GNOME Software.
3. **Builds the package** from the saved (committed) code. It doesn't need
   the internet for this part.
4. **Prints where the package is** and the command to install it.

## Choosing the version number

| Command                      | Example         | Use it for                          |
|------------------------------|-----------------|-------------------------------------|
| `./build-rpm.sh`             | 0.1.1 → 0.1.2   | fixes and small changes (default)   |
| `./build-rpm.sh minor`       | 0.1.2 → 0.2.0   | new features                        |
| `./build-rpm.sh major`       | 0.2.0 → 1.0.0   | a big milestone                     |
| `./build-rpm.sh 1.4.0`       | → 1.4.0         | a number of your choice             |
| `./build-rpm.sh --rebuild`   | stays the same  | the same version again, no new number |

Add `--latest` to any of these to fetch the newest code from GitHub first
(see the README).

`dnf install` only upgrades to a higher number. To put a `--rebuild` of the
version you already have on top of it, use `sudo dnf reinstall ./…rpm`.

## Updated sources

The package is always built from the code that is saved in git. When
Claude finishes a change, it commits it, so running `./build-rpm.sh`
afterwards includes it.

- If the script prints *"there are uncommitted changes; they are not in
  this build"*, some edits haven't been saved in git yet. Ask Claude to
  commit them, then run the script again.
- On another computer, get the latest code first with `git pull`, then
  run `./build-rpm.sh`.

## Optional packages

The package doesn't require these, but some features need them:

- **`swtpm` and `swtpm-tools`**: the software TPM 2.0 that Windows 11 needs
  (the "TPM 2.0" switch in the New VM wizard and Add Hardware → TPM).
  Without them, a VM with a TPM won't start. Install them with
  `sudo dnf install swtpm swtpm-tools`.

## Keeping it secure and up to date

The app's building blocks come from three places:

- **System libraries** (GTK, libadwaita, libvirt, libosinfo, SPICE, VNC,
  VTE, the OS list) come from Fedora. Your normal system updates
  (`sudo dnf upgrade` or GNOME Software) keep them patched, and the app
  uses the new versions right away. You don't need to rebuild.
- **Rust crates** (about 55 small libraries) are built into the app. Fixes
  in them only reach you through a new build.
- **The build tools** live in the toolbox container.

About once a month, or when GitHub emails you a **Dependabot alert** (they
are switched on for this repository), run:

```sh
./maintenance.sh
./build-rpm.sh
```

`maintenance.sh` updates the build tools in the toolbox, moves the crates to
their newest compatible releases, and checks them against the public list
of known security problems (RustSec). It then builds the app, runs all the
tests, and does an end-to-end check that creates and deletes a throwaway VM
in your user session. If everything passes, it saves (commits) the updated
crate list. `build-rpm.sh` then makes a new version with the fixes.

If it reports a problem, it doesn't save anything. Ask Claude to look at
the output, or just ask Claude to "do a maintenance check".

### After upgrading Fedora

After your computer moves to a new Fedora release, `build-rpm.sh` and
`maintenance.sh` stop with a message: the toolbox still runs the old
release, and packages built there may not fit. Recreate the toolbox with:

```sh
toolbox rm --force virt-manager-nextgen
./build-rpm.sh
```

## Going back to an older version

Every version is tagged in git (`v0.1.0`, `v0.1.1`, …), and every package
you've built stays in `target/rpmbuild/RPMS/x86_64/`. To go back to an
older one:

```sh
sudo dnf downgrade ./target/rpmbuild/RPMS/x86_64/virt-manager-nextgen-<older version>-1.fc44.x86_64.rpm
```

## Removing the app

```sh
sudo dnf remove virt-manager-nextgen
```

Your settings stay in `~/.config/virt-manager-nextgen/`. Delete that folder
too if you want a clean slate.
