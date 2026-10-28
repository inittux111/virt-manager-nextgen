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
