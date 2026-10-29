#!/bin/sh
# The "other computer" for the remote (SSH) tests: Debian's libvirt and an
# SSH server that accepts the test key in /remote/key.pub (see
# tests/remote-host.sh). Runs until the container is stopped.
set -u
sh /src/tests/libvirt-in-container.sh
sh /src/tests/sshd-in-container.sh /remote/key.pub
echo ready >/remote/ready
exec /usr/sbin/sshd -D -e
