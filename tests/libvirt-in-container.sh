#!/bin/sh
# Starts Debian's libvirt inside a test container: QEMU runs as root,
# without cgroups or private namespaces (the container already isolates it).
cat >>/etc/libvirt/qemu.conf <<CONF
user = "root"
group = "root"
cgroup_controllers = [ ]
namespaces = [ ]
remember_owner = 0
CONF
virtlogd -d
libvirtd -d
for _ in $(seq 30); do virsh -c qemu:///system version >/dev/null 2>&1 && break; sleep 1; done
