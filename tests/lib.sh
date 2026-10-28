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

# make_iso <file>: a tiny installer image for the test VMs (needs xorriso).
make_iso() {
    dir=$(mktemp -d)
    echo "virt-manager-nextgen test image" >"$dir/README"
    xorriso -as mkisofs -quiet -V VMNG_TEST -o "$1" "$dir"
    rm -rf "$dir"
}
