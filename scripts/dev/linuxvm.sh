#!/usr/bin/env bash
# Grafida Shell — clean Linux test VMs for packaged apps (grafida/grafida#86).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Four VMs, each set up unattended from its distribution's own image, then updated and snapshotted
# as `clean` (nothing of ours installed). See docs/dev/linux-test-vms.md.
#
#   ubuntu-22.04   the floor: live-server ISO + subiquity autoinstall + ubuntu-desktop-minimal (GNOME)
#   fedora-43      Everything netinstall + kickstart, @^workstation-product-environment (GNOME)
#   arch           archiso + its `script=` boot hook, plasma-meta + SDDM (KDE Plasma)
#   ubuntu-22.04-arm64   the arm64 tarball's floor: Ubuntu's signed arm64 cloud image + cloud-init,
#                  no desktop; emulated (TCG) by qemu-system-aarch64, always headless, not quickemu.
#                  `install` fetches and verifies the image itself. session, key and the pointer
#                  and screen commands need a desktop and refuse it.
#
# Usage: scripts/dev/linuxvm.sh COMMAND DISTRO [ARGS…]
#   install DISTRO          install from the ISO (headless, serial console logged to install.log);
#                           arm64: fetch and verify the cloud image, make the disk and the seed
#   start DISTRO [--headless]  boot it with quickemu (a window, unless --headless); always with a QMP
#                           socket, $LINUXVM_DIR/DISTRO/DISTRO-qmp.socket, for the input commands below;
#                           arm64: qemu-system-aarch64, headless, serial console to console.log
#   wait DISTRO             wait until SSH answers
#   ssh DISTRO [CMD…]       a shell or a command, as user `tester`
#   session DISTRO CMD…     a command in the logged-in desktop session (WAYLAND_DISPLAY, D-Bus, …)
#   prepare DISTRO          update, and switch off screen locking and blanking, then power off
#   key DISTRO KEY          press a key on the VM's keyboard (QEMU sendkey), e.g. `key fedora-43 esc`
#                           to leave the GNOME Overview a login lands in, so new windows get focus
#   move DISTRO X Y         real pointer input (QMP, the USB tablet), at the screen pixel X,Y
#   click DISTRO X Y [left|right|middle] [MODS]   a click there; MODS = ctrl, shift, alt, altgr,
#                           super, joined by - (`click arch 640 400 right ctrl-shift`)
#   drag DISTRO X1 Y1 X2 Y2 [left|right|middle] [MODS]   press at X1,Y1, move there in steps, release
#   screen DISTRO           the guest's screen size now, WxH
#   screendump DISTRO OUT.png|OUT.ppm   the guest's screen; the pointer is in it only when the guest
#                           draws it there (Fedora's GNOME does, Ubuntu 22.04's does not)
#   stop DISTRO             power off
#   snapshot DISTRO create|apply|list [NAME]   internal qcow2 snapshots (the VM must be off)
#
# Pointer coordinates are pixels of the guest's screen as a screendump shows them (physical pixels,
# origin top left). The events are absolute, 0…32767 across the screen, so each command first reads
# the screen's current size from a screendump and scales to it; a resolution change in the guest is
# picked up on the next command. Real input is for what the automation channel only fakes (focus,
# hover, drags, the compositor); see docs/dev/linux-test-vms.md.
#
# The VMs live in $LINUXVM_DIR (default ~/VMs), next to the Windows one. The ISOs are fetched with
# quickget (`quickget ubuntu-server 22.04`, `quickget archlinux latest`) and, for Fedora, from the
# Fedora mirrors (see the docs). Every account is `tester`, with a dedicated SSH key and sudo.

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
VMS=${LINUXVM_DIR:-$HOME/VMs}
KEY=$HOME/.ssh/grafida-linuxvm_ed25519
PASSWORD=${LINUXVM_PASSWORD:-tester}

die() { echo "linuxvm: $*" >&2; exit 1; }

port() {
    case $1 in
        ubuntu-22.04) echo 22221 ;;
        fedora-43) echo 22222 ;;
        arch) echo 22223 ;;
        ubuntu-22.04-arm64) echo 22225 ;; # 22224 is winvm.sh's windows-10
        *) die "unknown distro '$1' (ubuntu-22.04, fedora-43, arch, ubuntu-22.04-arm64)" ;;
    esac
}

is_arm64() { [ "$1" = ubuntu-22.04-arm64 ]; }

iso() {
    local pattern
    case $1 in
        ubuntu-22.04) pattern="$VMS/ubuntu-server-22.04/*.iso" ;;
        fedora-43) pattern="$VMS/fedora-43/Fedora-Everything-netinst-*.iso" ;;
        arch) pattern="$VMS/archlinux-latest/*.iso" ;;
    esac
    # shellcheck disable=SC2086
    ls $pattern 2>/dev/null | head -1 || true
}

ssh_opts() {
    echo -p "$(port "$1")" -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=5
}

render() { # template → stdout, with the password hash and the key filled in
    local hash
    hash=$(openssl passwd -6 "$PASSWORD")
    sed -e "s|@PASSWORD_HASH@|$hash|g" -e "s|@SSH_KEY@|$(cat "$KEY.pub")|g" "$1"
}

volume_id() { isoinfo -d -i "$1" | sed -n 's/^Volume id: //p'; }

install_vm() {
    local distro=$1 dir=$VMS/$1 src
    src=$(iso "$distro")
    [ -n "$src" ] || die "no ISO for $distro under $VMS (see docs/dev/linux-test-vms.md)"
    [ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N '' -C grafida-linuxvm -f "$KEY"
    [ ! -e "$dir/disk.qcow2" ] || die "$dir/disk.qcow2 exists; remove it to reinstall"
    mkdir -p "$dir/boot" "$dir/seed"
    qemu-img create -q -f qcow2 "$dir/disk.qcow2" 48G

    local kernel initrd append extra=()
    case $distro in
        ubuntu-22.04)
            bsdtar -xOf "$src" casper/vmlinuz > "$dir/boot/vmlinuz"
            bsdtar -xOf "$src" casper/initrd > "$dir/boot/initrd"
            render "$HERE/linuxvm/ubuntu-22.04.user-data" > "$dir/seed/user-data"
            echo "instance-id: ubuntu2204" > "$dir/seed/meta-data"
            genisoimage -quiet -o "$dir/boot/seed.iso" -volid cidata -joliet -rock "$dir/seed"
            extra=(-drive "file=$dir/boot/seed.iso,format=raw,if=virtio,readonly=on")
            append="autoinstall ds=nocloud console=ttyS0,115200n8"
            ;;
        fedora-43)
            bsdtar -xOf "$src" images/pxeboot/vmlinuz > "$dir/boot/vmlinuz"
            bsdtar -xOf "$src" images/pxeboot/initrd.img > "$dir/boot/initrd"
            render "$HERE/linuxvm/fedora-43.ks" > "$dir/seed/ks.cfg"
            genisoimage -quiet -o "$dir/boot/oemdrv.iso" -volid OEMDRV -joliet -rock "$dir/seed"
            extra=(-drive "file=$dir/boot/oemdrv.iso,format=raw,if=virtio,readonly=on")
            append="inst.stage2=hd:LABEL=$(volume_id "$src") inst.ks=hd:LABEL=OEMDRV:/ks.cfg inst.text console=ttyS0,115200n8"
            ;;
        arch)
            bsdtar -xOf "$src" arch/boot/x86_64/vmlinuz-linux > "$dir/boot/vmlinuz"
            bsdtar -xOf "$src" arch/boot/x86_64/initramfs-linux.img > "$dir/boot/initrd"
            render "$HERE/linuxvm/arch-install.sh" > "$dir/seed/arch-install.sh"
            # archiso fetches script= over the network: serve it on the host's loopback, which
            # QEMU's user network shows the guest as 10.0.2.2.
            (cd "$dir/seed" && exec python3 -I -m http.server 18800 --bind 127.0.0.1 >/dev/null 2>&1) &
            SERVER_PID=$!
            trap 'kill "${SERVER_PID:-}" 2>/dev/null || true' EXIT
            append="archisobasedir=arch archisolabel=$(volume_id "$src") script=http://10.0.2.2:18800/arch-install.sh console=ttyS0,115200n8"
            ;;
    esac
    kernel=$dir/boot/vmlinuz
    initrd=$dir/boot/initrd

    echo "Installing $distro (serial console in $dir/install.log); this takes a while…"
    # The same machine type quickemu runs the VM with, so devices keep their names after the install.
    qemu-system-x86_64 -enable-kvm -machine pc -cpu host -smp 4 -m 6144 \
        -drive "file=$dir/disk.qcow2,if=virtio" -cdrom "$src" "${extra[@]}" \
        -kernel "$kernel" -initrd "$initrd" -append "$append" \
        -netdev user,id=n0 -device virtio-net-pci,netdev=n0 \
        -display none -serial "file:$dir/install.log" -monitor none -no-reboot
    echo "$distro installed; it powered itself off."

    cat > "$VMS/$distro.conf" <<CONF
#!/usr/bin/quickemu --vm
guest_os="linux"
# The install ran under QEMU's default BIOS (SeaBIOS), so the disk has a BIOS GRUB; quickemu would
# otherwise boot a Linux guest with UEFI and find nothing to boot.
boot="legacy"
disk_img="$distro/disk.qcow2"
ram="6G"
cpu_cores="4"
port_forwards=("$(port "$distro"):22")
CONF
}

wait_ssh() {
    # Under emulation a boot takes minutes, and the first one (cloud-init) longer.
    local tries=180
    is_arm64 "$1" && tries=360
    # shellcheck disable=SC2046
    for _ in $(seq 1 $tries); do
        ssh $(ssh_opts "$1") tester@localhost true 2>/dev/null && return 0
        if is_arm64 "$1" && ! arm64_running "$1"; then die "QEMU exited; see $VMS/$1/console.log"; fi
        sleep 5
    done
    die "$1 did not answer on SSH within $((tries / 12)) minutes"
}

# --- ubuntu-22.04-arm64 -------------------------------------------------------------------------
# Ubuntu's cloud image, checked against the release directory's SHA256SUMS, whose signature must be
# by the "UEC Image Automatic Signing Key <cdimage@ubuntu.com>": the key Ubuntu's own ubuntu-keyring
# package ships as ubuntu-cloudimage-keyring.gpg. It is fetched by its full fingerprint into a
# throwaway keyring, so nothing is added to the host's.
ARM64_BASE=https://cloud-images.ubuntu.com/releases/22.04/release
ARM64_IMAGE=ubuntu-22.04-server-cloudimg-arm64.img
ARM64_KEY=D2EB44626FDDC30B513D5BB71A5D6C4C7DB87C81
# cloud-init's netplan matches the NIC by MAC on the first boot and is never rewritten after it
# (cloud-init is disabled then), so every boot must use the same one.
ARM64_MAC=52:54:00:12:34:a6

arm64_running() { [ -f "$VMS/$1/qemu.pid" ] && kill -0 "$(cat "$VMS/$1/qemu.pid")" 2>/dev/null; }

aarch64_firmware() { # the UEFI code image: Arch/Fedora edk2-aarch64, Debian/Ubuntu qemu-efi-aarch64, QEMU's own
    local f
    for f in /usr/share/edk2/aarch64/QEMU_CODE.fd /usr/share/edk2/aarch64/QEMU_EFI.fd \
        /usr/share/AAVMF/AAVMF_CODE.fd /usr/share/qemu/edk2-aarch64-code.fd; do
        [ -f "$f" ] && { echo "$f"; return; }
    done
    die "no aarch64 UEFI firmware (Arch: edk2-aarch64; Debian/Ubuntu: qemu-efi-aarch64)"
}

fetch_arm64_image() { # into $1, fetched only when the copy there is missing or not the signed one
    local dl=$1 gnupg want
    mkdir -p "$dl"
    curl -fsSL -o "$dl/SHA256SUMS" "$ARM64_BASE/SHA256SUMS"
    curl -fsSL -o "$dl/SHA256SUMS.gpg" "$ARM64_BASE/SHA256SUMS.gpg"
    gnupg=$(mktemp -d)
    curl -fsSL "https://keyserver.ubuntu.com/pks/lookup?op=get&options=mr&search=0x$ARM64_KEY" \
        | GNUPGHOME=$gnupg gpg -q --import 2>/dev/null
    # VALIDSIG names the signing key and, last, its primary key.
    if ! GNUPGHOME=$gnupg gpg --status-fd 1 --verify "$dl/SHA256SUMS.gpg" "$dl/SHA256SUMS" 2>/dev/null \
        | awk -v k="$ARM64_KEY" '$2 == "VALIDSIG" && ($3 == k || $NF == k) { ok = 1 } END { exit !ok }'; then
        rm -rf "$gnupg"
        die "$ARM64_BASE/SHA256SUMS: no good signature by $ARM64_KEY"
    fi
    rm -rf "$gnupg"
    echo "SHA256SUMS: good signature by $ARM64_KEY"
    want=$(awk -v f="*$ARM64_IMAGE" '$2 == f { print $1 }' "$dl/SHA256SUMS")
    [ -n "$want" ] || die "$ARM64_IMAGE is not in SHA256SUMS"
    if ! echo "$want  $dl/$ARM64_IMAGE" | sha256sum -c --status - 2>/dev/null; then
        curl -fL -# -o "$dl/$ARM64_IMAGE.part" "$ARM64_BASE/$ARM64_IMAGE"
        mv "$dl/$ARM64_IMAGE.part" "$dl/$ARM64_IMAGE"
        echo "$want  $dl/$ARM64_IMAGE" | sha256sum -c --status - || die "$ARM64_IMAGE: not the signed SHA-256"
    fi
    echo "$ARM64_IMAGE: SHA-256 $want, as signed"
}

install_arm64() {
    local distro=$1 dir=$VMS/$1 code vars
    [ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N '' -C grafida-linuxvm -f "$KEY"
    [ ! -e "$dir/disk.qcow2" ] || die "$dir/disk.qcow2 exists; remove it to reinstall"
    code=$(aarch64_firmware)
    fetch_arm64_image "$dir/download"
    mkdir -p "$dir/seed"
    qemu-img convert -O qcow2 "$dir/download/$ARM64_IMAGE" "$dir/disk.qcow2"
    qemu-img resize -q "$dir/disk.qcow2" 24G
    render "$HERE/linuxvm/ubuntu-22.04-arm64.user-data" > "$dir/seed/user-data"
    printf 'instance-id: ubuntu2204-arm64\nlocal-hostname: ubuntu2204-arm64\n' > "$dir/seed/meta-data"
    genisoimage -quiet -o "$dir/seed.iso" -volid cidata -joliet -rock "$dir/seed/user-data" "$dir/seed/meta-data"
    # The UEFI variable store: the firmware's own empty template, or zeroes the size of the code.
    vars=$(dirname "$code")/$(basename "$code" | sed -E 's/(CODE|EFI)\.fd$/VARS.fd/; s/-code\.fd$/-vars.fd/')
    if [ -f "$vars" ] && [ "$vars" != "$code" ]; then cp "$vars" "$dir/efivars.fd"; else truncate -s "$(stat -c %s "$code")" "$dir/efivars.fd"; fi
    echo "$distro created. Its first boot runs cloud-init: \`start $distro\`, then \`prepare $distro\`."
}

start_arm64() {
    local distro=$1 dir=$VMS/$1
    [ -f "$dir/disk.qcow2" ] || die "no $dir/disk.qcow2: run \`install $distro\` first"
    ! arm64_running "$distro" || die "$distro is already running (pid $(cat "$dir/qemu.pid"))"
    # TCG with a thread per vCPU (MTTCG). -cpu max with the cheap pointer-authentication algorithm:
    # the default (QARMA5) is several times slower under emulation. No display: the guest has no
    # desktop; the serial console goes to console.log. The seed stays attached; cloud-init ignores it
    # after the first boot. The monitor socket is named relative to the VM's folder, QEMU's working
    # directory, because a unix socket's path must be shorter than 108 bytes.
    cd "$dir"
    qemu-system-aarch64 -machine virt,gic-version=3 -accel tcg,thread=multi \
        -cpu max,pauth-impdef=on -smp 8 -m 8G \
        -drive "if=pflash,format=raw,readonly=on,file=$(aarch64_firmware)" \
        -drive "if=pflash,format=raw,file=$dir/efivars.fd" \
        -drive "if=virtio,format=qcow2,file=$dir/disk.qcow2,cache=writeback,discard=unmap" \
        -drive "if=virtio,format=raw,readonly=on,file=$dir/seed.iso" \
        -netdev "user,id=n0,hostfwd=tcp:127.0.0.1:$(port "$distro")-:22" \
        -device "virtio-net-pci,netdev=n0,mac=$ARM64_MAC" -device virtio-rng-pci \
        -display none -serial "file:$dir/console.log" \
        -monitor "unix:$distro-monitor.socket,server=on,wait=off" \
        -pidfile "$dir/qemu.pid" -daemonize
    echo "$distro started (pid $(cat "$dir/qemu.pid")), headless; \`wait $distro\` for SSH on $(port "$distro")"
}

stop_arm64() { # power off and wait for QEMU to exit: a snapshot needs the disk closed
    local distro=$1
    arm64_running "$distro" || { echo "$distro is not running"; return 0; }
    # shellcheck disable=SC2046
    ssh $(ssh_opts "$distro") tester@localhost 'sudo systemctl poweroff' 2>/dev/null || true
    for _ in $(seq 1 120); do
        arm64_running "$distro" || { rm -f "$VMS/$distro/qemu.pid"; echo "$distro stopped"; return 0; }
        sleep 2
    done
    die "$distro still running after 4 minutes"
}

# The logged-in desktop session's environment, exported, then the command. GNOME and Plasma both
# import WAYLAND_DISPLAY & co. into the systemd user manager; reading a compositor's
# /proc/<pid>/environ instead fails for kwin_wayland, which has capabilities and is not dumpable.
SESSION_ENV='export XDG_RUNTIME_DIR=/run/user/$(id -u)
env=$(systemctl --user show-environment | grep -E "^(WAYLAND_DISPLAY|DISPLAY|XAUTHORITY|XDG_CURRENT_DESKTOP|XDG_SESSION_TYPE|DBUS_SESSION_BUS_ADDRESS)=")
[ -n "$env" ] || { echo "no desktop session" >&2; exit 1; }
eval "$(printf "%s\n" "$env" | sed "s/^/export /")"
: "${DBUS_SESSION_BUS_ADDRESS:=unix:path=$XDG_RUNTIME_DIR/bus}"; export DBUS_SESSION_BUS_ADDRESS'

cmd=${1:-}; distro=${2:-}
[ -n "$cmd" ] && [ -n "$distro" ] || { sed -n '9,/^$/p' "$0"; exit 64; }
port "$distro" >/dev/null
shift 2

if is_arm64 "$distro"; then
    case $cmd in
        session | key | move | click | drag | screen | screendump)
            die "$distro has no desktop and no display, which '$cmd' needs" ;;
    esac
fi

# shellcheck disable=SC2046
case $cmd in
    install) if is_arm64 "$distro"; then install_arm64 "$distro"; else install_vm "$distro"; fi ;;
    start)
        if is_arm64 "$distro"; then start_arm64 "$distro"; exit; fi
        display=()
        [ "${1:-}" = --headless ] && display=(--display none)
        # ⚠️ quickemu adds an SSH forward of its own on the first free port from 22220 up. With a
        # Windows guest on 22220 that is 22221, the forward ubuntu-22.04.conf already has, and QEMU
        # refuses the duplicate ("Could not set up host forwarding rule"). Name one nothing uses.
        # The QMP socket is always opened, beside the HMP monitor socket quickemu makes: it gives no
        # more than that monitor already does, and a VM started without it would have to be
        # restarted, losing what was under test, before it could take real pointer input. quickemu
        # (4.9) has no -qmp of its own. The path is relative to $VMS, QEMU's working directory, since
        # quickemu splits --extra_args on spaces; the leading space keeps it apart from any
        # extra_args the .conf has, which quickemu appends it to.
        (cd "$VMS" && quickemu --vm "$distro.conf" --ssh-port "$(($(port "$distro") + 10))" "${display[@]}" \
            --extra_args " -qmp unix:$distro/$distro-qmp.socket,server=on,wait=off")
        ;;
    wait) wait_ssh "$distro" ;;
    ssh) exec ssh $(ssh_opts "$distro") tester@localhost "$@" ;;
    session) exec ssh $(ssh_opts "$distro") tester@localhost "$SESSION_ENV"$'\n'"$*" ;;
    prepare)
        wait_ssh "$distro"
        if is_arm64 "$distro"; then
            # The first boot's cloud-init must be done before apt runs; keep the image's own
            # configuration files where an update ships new ones.
            ssh $(ssh_opts "$distro") tester@localhost 'cloud-init status --wait >/dev/null
                export DEBIAN_FRONTEND=noninteractive
                sudo -E apt-get update -q && sudo -E apt-get -yq -o Dpkg::Options::=--force-confold full-upgrade &&
                sudo apt-get -yq autoremove && sudo apt-get clean'
            stop_arm64 "$distro"
            exit
        fi
        case $distro in
            ubuntu-22.04) up='sudo DEBIAN_FRONTEND=noninteractive apt-get update -q && sudo DEBIAN_FRONTEND=noninteractive apt-get -yq full-upgrade' ;;
            fedora-43) up='sudo dnf -y upgrade --refresh' ;;
            arch) up='sudo pacman -Syu --noconfirm' ;;
        esac
        ssh $(ssh_opts "$distro") tester@localhost "$up"
        ssh $(ssh_opts "$distro") tester@localhost "$SESSION_ENV"'
if command -v gsettings >/dev/null && [ "${XDG_CURRENT_DESKTOP#*GNOME}" != "$XDG_CURRENT_DESKTOP" ]; then
  gsettings set org.gnome.desktop.session idle-delay 0
  gsettings set org.gnome.desktop.screensaver lock-enabled false
  gsettings set org.gnome.settings-daemon.plugins.power sleep-inactive-ac-type nothing
fi
if command -v kwriteconfig6 >/dev/null; then
  kwriteconfig6 --file kscreenlockerrc --group Daemon --key Autolock false
  kwriteconfig6 --file kscreenlockerrc --group Daemon --key LockOnResume false
  kwriteconfig6 --file powerdevilrc --group AC --group Display --key TurnOffDisplayWhenIdle false
  kwriteconfig6 --file powerdevilrc --group AC --group SuspendAndShutdown --key AutoSuspendAction 0
fi'
        ssh $(ssh_opts "$distro") tester@localhost 'sudo systemctl poweroff' || true
        ;;
    stop)
        if is_arm64 "$distro"; then stop_arm64 "$distro"; exit; fi
        ssh $(ssh_opts "$distro") tester@localhost 'sudo systemctl poweroff' || true
        ;;
    key) printf 'sendkey %s\n' "${1:?key}" | socat - "unix-connect:$VMS/$distro/$distro-monitor.socket" >/dev/null ;;
    move | click | drag | screen) python3 -I "$HERE/linuxvm/qmp.py" "$VMS/$distro/$distro-qmp.socket" "$cmd" "$@" ;;
    screendump) # QEMU writes the file itself, from its own working directory: hand it an absolute path
        out=$(realpath -m -- "${1:?OUT.png or OUT.ppm}")
        python3 -I "$HERE/linuxvm/qmp.py" "$VMS/$distro/$distro-qmp.socket" screendump "$out"
        ;;
    snapshot)
        disk=$VMS/$distro/disk.qcow2
        ! { is_arm64 "$distro" && arm64_running "$distro"; } || die "stop $distro first"
        case ${1:-} in
            create) qemu-img snapshot -c "${2:?name}" "$disk" ;;
            apply) qemu-img snapshot -a "${2:?name}" "$disk" ;;
            list) qemu-img snapshot -l "$disk" ;;
            *) die "snapshot create|apply|list [NAME]" ;;
        esac
        # The arm64 guest's UEFI variables (its boot entries) are outside the disk: one copy per
        # snapshot, so a snapshot boots with the entries it was taken with.
        if is_arm64 "$distro"; then
            vars=$VMS/$distro/efivars.fd
            case $1 in
                create) cp "$vars" "$vars.$2" ;;
                apply) if [ -f "$vars.$2" ]; then cp "$vars.$2" "$vars"; fi ;;
            esac
        fi
        ;;
    *) die "unknown command '$cmd'" ;;
esac
