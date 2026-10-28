#!/bin/sh
# The test pipeline: checks that every feature still works on Fedora 44 and
# Debian 13. Run it on the host (Fedora, with toolbox and podman):
#
#   ./test-pipeline.sh            both
#   ./test-pipeline.sh fedora     Fedora 44 only
#   ./test-pipeline.sh debian     Debian 13 only
#
# Fedora 44: code checks and unit tests in the toolbox, then throwaway VMs on
# your user session (the network check uses the System connection), and
# screenshots of every main screen. Debian 13: the same, inside a Debian 13
# container with Debian's own libvirt and QEMU. Everything the tests create
# is named vmng-test-… and removed again; your own VMs are never touched.
#
# Results, logs and screenshots: target/test-report/<system>/.
set -u

cd "$(dirname "$0")"
which=${1:-all}
status=0
case $which in
    all) targets="fedora debian" ;;
    fedora | debian) targets=$which ;;
    *) echo "Usage: $0 [fedora|debian]" >&2; exit 2 ;;
esac

for t in $targets; do
    sh "tests/$t.sh" || status=1
    echo
done

echo "== Summary"
for t in $targets; do
    case $t in fedora) dir=fedora44 label="Fedora 44" ;; debian) dir=debian13 label="Debian 13" ;; esac
    results="target/test-report/$dir/results.txt"
    if [ -f "$results" ]; then
        pass=$(grep -c '^PASS' "$results")
        fail=$(grep -c '^FAIL' "$results")
        printf '%-10s %3s passed, %s failed\n' "$label" "$pass" "$fail"
        grep '^FAIL' "$results" | sed 's/^/           /'
    else
        printf '%-10s did not run (see the output above)\n' "$label"
        status=1
    fi
done
echo "Details: target/test-report/"
exit $status
