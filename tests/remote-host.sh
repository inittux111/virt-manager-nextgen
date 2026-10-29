#!/bin/sh
# Starts or stops the "other computer" for the remote (SSH) tests: a Debian
# container with its own libvirt and an SSH server on 127.0.0.1:<port>, with
# a throwaway key in <dir>. Prints the libvirt address to use.
#
#   tests/remote-host.sh start <dir> [port]   # prints qemu+ssh://root@127.0.0.1:<port>/system?…
#   tests/remote-host.sh stop
set -u
cd "$(dirname "$0")/.."
name=vmng-test-remote
case ${1:-} in
start)
    dir=$2
    port=${3:-2222}
    mkdir -p "$dir"
    rm -f "$dir/key" "$dir/key.pub" "$dir/ready"
    ssh-keygen -q -t ed25519 -N '' -C vmng-test -f "$dir/key" || exit 1
    sh tests/debian-image.sh >&2 || exit 1
    podman rm -f "$name" >/dev/null 2>&1
    # Its own network (its libvirt never touches this computer's); only the
    # SSH port is reachable, on this computer's 127.0.0.1.
    podman run -d --rm --name "$name" -p "127.0.0.1:$port:22" \
        --privileged --device /dev/kvm --security-opt label=disable \
        -v "$PWD/tests:/src/tests:ro" -v "$dir:/remote" \
        localhost/virt-manager-nextgen-debian-test:trixie sh /src/tests/remote-inside.sh >/dev/null || exit 1
    for _ in $(seq 60); do [ -f "$dir/ready" ] && break; sleep 1; done
    sleep 2
    echo "qemu+ssh://root@127.0.0.1:$port/system?keyfile=$dir/key&no_verify=1"
    ;;
stop)
    podman rm -f "$name" >/dev/null 2>&1
    ;;
*)
    echo "usage: $0 start <dir> [port] | stop" >&2
    exit 2
    ;;
esac
