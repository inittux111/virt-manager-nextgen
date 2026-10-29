# Shared by the test scripts: `check <name> <command…>` runs a command, keeps
# its output in $OUT/<name>.log and records PASS or FAIL in $OUT/results.txt.
check() {
    check_name=$1
    shift
    if "$@" >"$OUT/$check_name.log" 2>&1; then
        echo "PASS  $check_name" | tee -a "$OUT/results.txt"
    else
        echo "FAIL  $check_name   (see $check_name.log)" | tee -a "$OUT/results.txt"
    fi
}

# `check_gpu <name> <command…>`: like check, but a failure because QEMU
# can't use a graphics card (none, or a driver the container lacks) is
# recorded as SKIP.
check_gpu() {
    check_name=$1
    shift
    if "$@" >"$OUT/$check_name.log" 2>&1; then
        echo "PASS  $check_name" | tee -a "$OUT/results.txt"
    elif grep -qE 'egl|render node|rendernode' "$OUT/$check_name.log"; then
        echo "SKIP  $check_name   (no graphics card QEMU can use; see $check_name.log)" | tee -a "$OUT/results.txt"
    else
        echo "FAIL  $check_name   (see $check_name.log)" | tee -a "$OUT/results.txt"
    fi
}

# make_iso <file>: a tiny installer image for the test VMs (needs xorriso).
make_iso() {
    dir=$(mktemp -d)
    echo "virt-manager-nextgen test image" >"$dir/README"
    xorriso -as mkisofs -quiet -V VMNG_TEST -o "$1" "$dir"
    rm -rf "$dir"
}
