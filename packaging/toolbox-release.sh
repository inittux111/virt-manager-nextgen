#!/bin/sh
# Checks that the toolbox runs the same Fedora release as this computer.
# Exits 1 (and explains) when they differ, e.g. after upgrading to a new
# Fedora: packages built in the old release may not fit the new one.
set -eu

box=virt-manager-nextgen
host=$(rpm -E %fedora)
inside=$(toolbox run -c "$box" sh -c '. /etc/os-release; echo "$VERSION_ID"')
if [ "$inside" != "$host" ]; then
    cat >&2 <<MSG
The $box toolbox runs Fedora $inside, but this computer runs Fedora $host.
Packages built there may not work here. Recreate the toolbox with:

    toolbox rm --force $box
    ./build-rpm.sh

(build-rpm.sh creates a new toolbox for Fedora $host, then builds as usual.)
MSG
    exit 1
fi
