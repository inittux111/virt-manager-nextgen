#!/bin/sh
# Prepares an SSH server in a test container that accepts the key in $1
# (root login with that key only). Start it afterwards with /usr/sbin/sshd.
set -u
mkdir -p /root/.ssh /run/sshd
cp "$1" /root/.ssh/authorized_keys
chmod 700 /root/.ssh
chmod 600 /root/.ssh/authorized_keys
ssh-keygen -A >/dev/null
cat >/etc/ssh/sshd_config.d/vmng-test.conf <<CONF
PasswordAuthentication no
PermitRootLogin prohibit-password
CONF
