#!/bin/sh
# The shared test suite: real-libvirt checks with throwaway VMs, and
# screenshots of every main screen. Run by tests/fedora.sh (on the host) and
# tests/debian.sh (inside the Debian 13 test container); see test-pipeline.sh.
#
# Environment:
#   BIN            folder with the debug build (virt-manager-nextgen, examples/)
#   OUT            report folder (results.txt, logs, screenshots)
#   ISO            a small installer image for the test VMs
#   POOL           a storage pool on VM_URI for test disks
#   VM_URI         libvirt for the VM checks
#   NET_URI        libvirt for the network/pool check
#   REMOTE_URI     another computer's libvirt over SSH (qemu+ssh://…) for the
#                  remote checks; skipped when empty
#   WAYLAND_DISPLAY  the headless test screen for screenshots
#
# Everything it creates is named vmng-test-… and removed again.
set -u

mkdir -p "$OUT"
. "$(dirname "$0")/lib.sh"

# ── libvirt checks ───────────────────────────────────────────────────────
tmp=$(mktemp -d)
mkdir -p "$tmp/selftest" "$tmp/netpool" "$tmp/options"
export VMNG_TEST_URI="$VM_URI"
check vm-snapshot-clone-rename-delete "$BIN/examples/selftest" "$tmp/selftest"
check vm-create-start-delete "$BIN/examples/newvm_check" "$VM_URI" "$POOL" "$ISO"
check hardware-add-edit-remove "$BIN/examples/hw_check" "$VM_URI" "$POOL" "$ISO"
check firmware-bios-uefi-files "$BIN/examples/firmware_check"
check tpm-2.0 "$BIN/examples/tpm_check" "$ISO" 1
check networks-and-storage-pools "$BIN/examples/net_pool_check" "$NET_URI" "$tmp/netpool"
check cpu-model-layout-mitigations "$BIN/examples/cpu_check"
check os-type-change "$BIN/examples/os_check"
check machine-options "$BIN/examples/options_check" "$tmp/options"
check network-boot-pxe "$BIN/examples/pxe_check"
check_gpu video-3d-acceleration "$BIN/examples/video3d_check"
rm -rf "$tmp"

# ── Screens ──────────────────────────────────────────────────────────────
# A running VM (SPICE screen, serial console) for the preview and console.
cat >"$OUT/vmng-test-ui.xml" <<XML
<domain type="kvm"><name>vmng-test-ui</name><memory unit="MiB">256</memory><vcpu>1</vcpu>
<os><type arch="x86_64" machine="q35">hvm</type><bios useserial="yes"/><boot dev="cdrom"/></os>
<features><acpi/><apic/></features>
<devices>
<disk type="file" device="cdrom"><driver name="qemu" type="raw"/><source file="$ISO"/><target dev="sda" bus="sata"/><readonly/></disk>
<serial type="pty"/><console type="pty"><target type="serial"/></console>
<input type="tablet" bus="usb"/><graphics type="spice" autoport="yes"/><video><model type="virtio"/></video>
</devices></domain>
XML
check screens-test-vm-starts virsh -c "$VM_URI" create "$OUT/vmng-test-ui.xml"

# The app's settings: connect to VM_URI, plain Adwaita (no custom theme).
config="$OUT/config"
mkdir -p "$config/virt-manager-nextgen"
printf '[connections]\nautoconnect=%s\nlast=%s\n' "$VM_URI" "$VM_URI" >"$config/virt-manager-nextgen/settings.ini"

screen() { # screen <name> <VAR=value…>: one screenshot, checked for errors
    shot=$1
    shift
    # One retry: the very first picture of a run can come too early.
    for _ in 1 2; do
        env XDG_CONFIG_HOME="$config" VMNG_SCREENSHOT="$OUT/screen-$shot.png" VMNG_SCREENSHOT_DELAY=8 \
            VMNG_DEV_SIZE=1200x800 "$@" timeout 60 "$BIN/virt-manager-nextgen" >"$OUT/app-$shot.log" 2>&1
        test -s "$OUT/screen-$shot.png" && break
    done
    test -s "$OUT/screen-$shot.png" && ! grep -qiE 'panicked|critical|assertion' "$OUT/app-$shot.log"
}
vm="VMNG_DEV_SELECT=vmng-test-ui"
check screen-machines screen machines "$vm"
check screen-machines-narrow screen machines-narrow "$vm" VMNG_DEV_SIZE=400x760
check screen-hardware screen hardware "$vm" VMNG_DEV_ACTION=vm.hardware
check screen-overview screen overview "$vm" VMNG_DEV_ACTION=vm.hardware VMNG_DEV_HW_ROW=0
# Regression: just looking at every hardware page must not count as a change.
check regression-viewing-hardware-changes-nothing screen hardware-all-pages "$vm" VMNG_DEV_ACTION=vm.hardware \
    VMNG_DEV_HW_ALL_CLEAN=1 VMNG_SCREENSHOT_DELAY=14
check screen-boot-firmware screen boot-firmware "$vm" VMNG_DEV_ACTION=vm.hardware VMNG_DEV_HW_ROW=2
check screen-add-hardware screen add-hardware "$vm" VMNG_DEV_ACTION=vm.hardware VMNG_DEV_HW_ADD=1
check screen-snapshots screen snapshots "$vm" VMNG_DEV_ACTION=vm.snapshots
check screen-performance screen performance "$vm" VMNG_DEV_ACTION=vm.performance VMNG_SCREENSHOT_DELAY=14
# Browsing to a new disc image for the running VM's CD-ROM drive.
cp "$ISO" "$OUT/other-disc.iso"
check screen-insert-disc screen insert-disc "$vm" VMNG_DEV_ACTION=vm.hardware VMNG_DEV_HW_ROW=3 \
    VMNG_DEV_CDROM_FILE="$OUT/other-disc.iso"
check disc-inserted-in-running-vm sh -c "virsh -c '$VM_URI' domblklist vmng-test-ui | grep -q other-disc.iso"
check screen-console screen console "$vm" VMNG_DEV_ACTION=vm.console VMNG_SCREENSHOT_DELAY=12
check screen-text-console screen text-console "$vm" VMNG_DEV_ACTION=vm.console VMNG_DEV_CONSOLE_TEXT=1 VMNG_SCREENSHOT_DELAY=12
check screen-host screen host VMNG_DEV_VIEW=host
check screen-networks screen networks VMNG_DEV_VIEW=networks
# A network's XML (on NET_URI, which has networks), only looked at.
config_vm=$config
config="$OUT/config-net"
mkdir -p "$config/virt-manager-nextgen"
printf '[connections]\nautoconnect=%s\nlast=%s\n' "$NET_URI" "$NET_URI" >"$config/virt-manager-nextgen/settings.ini"
check screen-network-xml screen network-xml VMNG_DEV_VIEW=networks VMNG_DEV_NET_XML=1
# Editing a network's XML in the app and pressing Apply saves it.
printf '<network><name>vmng-test-netxml</name><ip address="192.168.231.1" netmask="255.255.255.0"/></network>' \
    >"$OUT/vmng-test-netxml.xml"
virsh -c "$NET_URI" net-define "$OUT/vmng-test-netxml.xml" >"$OUT/network-xml-edit.log" 2>&1
check screen-network-xml-edit screen network-xml-edit VMNG_DEV_VIEW=networks VMNG_DEV_NET_EDIT=vmng-test-netxml
check network-xml-edit-saved sh -c "virsh -c '$NET_URI' net-dumpxml --inactive vmng-test-netxml | grep -q \"mtu size='1400'\""
virsh -c "$NET_URI" net-undefine vmng-test-netxml >>"$OUT/network-xml-edit.log" 2>&1
config=$config_vm
check screen-storage screen storage VMNG_DEV_VIEW=storage
check screen-new-volume screen new-volume VMNG_DEV_VIEW=storage VMNG_DEV_NEW_VOLUME=1
for step in 0 1 2 3; do
    check "screen-new-vm-step-$step" screen "new-vm-step-$step" VMNG_DEV_ACTION=win.new-vm VMNG_DEV_WIZARD_STEP=$step
done
# Network install: read a Debian tree, download its installer, start it
# once from the installer's kernel (the saved configuration boots the disk).
tree=https://deb.debian.org/debian/dists/trixie/
check screen-new-vm-network screen new-vm-network VMNG_DEV_ACTION=win.new-vm VMNG_DEV_WIZARD_URL=$tree
check network-install screen network-install VMNG_DEV_ACTION=win.new-vm VMNG_DEV_WIZARD_URL=$tree \
    VMNG_DEV_WIZARD_STEP=3 VMNG_DEV_WIZARD_CREATE=vmng-test-netinst VMNG_SCREENSHOT_DELAY=45
check network-install-boots-installer-once sh -c "virsh -c '$VM_URI' dumpxml vmng-test-netinst | grep -q '<kernel>' \
    && ! virsh -c '$VM_URI' dumpxml --inactive vmng-test-netinst | grep -q '<kernel>'"
virsh -c "$VM_URI" destroy vmng-test-netinst >"$OUT/network-install-cleanup.log" 2>&1
virsh -c "$VM_URI" undefine vmng-test-netinst --nvram --storage vda >>"$OUT/network-install-cleanup.log" 2>&1
# The New VM wizard with an existing disk image as the machine's disk.
virsh -c "$VM_URI" vol-create-as "$POOL" vmng-test-existing.qcow2 64M --format qcow2 >"$OUT/existing-disk.log" 2>&1
existing=$(virsh -c "$VM_URI" vol-path --pool "$POOL" vmng-test-existing.qcow2 2>>"$OUT/existing-disk.log")
check screen-new-vm-existing-disk screen new-vm-existing-disk VMNG_DEV_ACTION=win.new-vm VMNG_DEV_WIZARD_STEP=2 \
    VMNG_DEV_WIZARD_EXISTING="$existing"
check new-vm-on-existing-disk screen new-vm-on-existing-disk VMNG_DEV_ACTION=win.new-vm VMNG_DEV_WIZARD_STEP=3 \
    VMNG_DEV_WIZARD_EXISTING="$existing" VMNG_DEV_WIZARD_CREATE=vmng-test-existing VMNG_SCREENSHOT_DELAY=12
check new-vm-uses-the-existing-disk sh -c "virsh -c '$VM_URI' domblklist vmng-test-existing | grep -q '$existing'"
virsh -c "$VM_URI" destroy vmng-test-existing >>"$OUT/existing-disk.log" 2>&1
virsh -c "$VM_URI" undefine vmng-test-existing --nvram >>"$OUT/existing-disk.log" 2>&1
virsh -c "$VM_URI" vol-delete --pool "$POOL" vmng-test-existing.qcow2 >>"$OUT/existing-disk.log" 2>&1
# Starting a machine when the host boots, switched on in Boot & firmware.
printf '<domain type="kvm"><name>vmng-test-autostart</name><memory unit="MiB">256</memory><vcpu>1</vcpu><os><type arch="x86_64" machine="q35">hvm</type></os><devices><graphics type="vnc" autoport="yes"/></devices></domain>' \
    >"$OUT/vmng-test-autostart.xml"
virsh -c "$VM_URI" define "$OUT/vmng-test-autostart.xml" >"$OUT/autostart.log" 2>&1
check screen-autostart screen autostart VMNG_DEV_SELECT=vmng-test-autostart VMNG_DEV_ACTION=vm.hardware \
    VMNG_DEV_HW_ROW=2 VMNG_DEV_AUTOSTART=1
check autostart-saved sh -c "virsh -c '$VM_URI' dominfo vmng-test-autostart | grep -qiE 'autostart: +enable'"
virsh -c "$VM_URI" undefine vmng-test-autostart >>"$OUT/autostart.log" 2>&1

# Another computer over SSH: connect from the Add Remote Computer dialog,
# see its machines and a machine's screen through the SSH tunnel, remove it.
if [ -n "${REMOTE_URI:-}" ]; then
    check remote-ssh-connection "$BIN/examples/remote_check" "$REMOTE_URI"
    # Without the installer disc: that file is only on this computer.
    sed -e 's/vmng-test-ui/vmng-test-remote-ui/' -e '/device="cdrom"/d' "$OUT/vmng-test-ui.xml" \
        >"$OUT/vmng-test-remote-ui.xml"
    check remote-test-vm-starts virsh -c "$REMOTE_URI" create "$OUT/vmng-test-remote-ui.xml"
    config_vm=$config
    config="$OUT/config-remote"
    mkdir -p "$config/virt-manager-nextgen"
    printf '[connections]\nautoconnect=%s\nlast=%s\n' "$VM_URI" "$VM_URI" >"$config/virt-manager-nextgen/settings.ini"
    check screen-add-remote screen add-remote VMNG_DEV_ACTION=win.add-connection VMNG_DEV_REMOTE="$REMOTE_URI"
    check screen-remote-machines screen remote-machines VMNG_DEV_ACTION=win.add-connection \
        VMNG_DEV_REMOTE="$REMOTE_URI" VMNG_DEV_FORM_SUBMIT=1 VMNG_DEV_REMEMBER=1 VMNG_DEV_SELECT=vmng-test-remote-ui \
        VMNG_SCREENSHOT_DELAY=14
    check remote-connection-saved grep -q "^remote=qemu+ssh://" "$config/virt-manager-nextgen/settings.ini"
    check screen-remote-console screen remote-console VMNG_DEV_SELECT=vmng-test-remote-ui VMNG_DEV_ACTION=vm.console \
        VMNG_SCREENSHOT_DELAY=14
    check screen-remote-menu screen remote-menu VMNG_DEV_MAIN_MENU=1 VMNG_DEV_SHOT_POPOVER=1
    check screen-remote-removed screen remote-removed VMNG_DEV_ACTION=win.remove-connection VMNG_DEV_CONFIRM=1 \
        VMNG_DEV_REMEMBER=1
    check remote-connection-removed grep -q "^remote=$" "$config/virt-manager-nextgen/settings.ini"
    config=$config_vm
    virsh -c "$REMOTE_URI" destroy vmng-test-remote-ui >"$OUT/remote-cleanup.log" 2>&1
fi

check screen-preferences screen preferences VMNG_DEV_ACTION=app.preferences
check screen-shortcuts screen shortcuts VMNG_DEV_ACTION=app.shortcuts
check screen-dark screen dark "$vm" ADW_DEBUG_COLOR_SCHEME=prefer-dark
# Appearance set to Dark in Preferences (the system asks for light).
config_light=$config
config="$OUT/config-dark"
mkdir -p "$config/virt-manager-nextgen"
printf '[connections]\nautoconnect=%s\nlast=%s\n[general]\nappearance=dark\n' "$VM_URI" "$VM_URI" \
    >"$config/virt-manager-nextgen/settings.ini"
check screen-appearance-dark screen appearance-dark "$vm" ADW_DEBUG_COLOR_SCHEME=prefer-light
config=$config_light
check screen-high-contrast screen high-contrast "$vm" ADW_DEBUG_COLOR_SCHEME=prefer-light ADW_DEBUG_HIGH_CONTRAST=1

virsh -c "$VM_URI" destroy vmng-test-ui >"$OUT/screens-test-vm-stop.log" 2>&1
! grep -q '^FAIL' "$OUT/results.txt"
