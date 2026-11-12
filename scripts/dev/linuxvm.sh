#!/usr/bin/env bash
# Grafida Shell — clean Linux test VMs for packaged apps (grafida/grafida#86).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Three VMs, each installed unattended by its distribution's own installer, then updated and
# snapshotted as `clean` (nothing of ours installed). See docs/dev/linux-test-vms.md.
#
#   ubuntu-22.04   the floor: live-server ISO + subiquity autoinstall + ubuntu-desktop-minimal (GNOME)
#   fedora-43      Everything netinstall + kickstart, @^workstation-product-environment (GNOME)
#   arch           archiso + its `script=` boot hook, plasma-meta + SDDM (KDE Plasma)
#
# Usage: scripts/dev/linuxvm.sh COMMAND DISTRO [ARGS…]
#   install DISTRO          install from the ISO (headless, serial console logged to install.log)
#   start DISTRO [--headless]  boot it with quickemu (a window, unless --headless)
#   wait DISTRO             wait until SSH answers
#   ssh DISTRO [CMD…]       a shell or a command, as user `tester`
#   session DISTRO CMD…     a command in the logged-in desktop session (WAYLAND_DISPLAY, D-Bus, …)
#   prepare DISTRO          update, and switch off screen locking and blanking, then power off
#   key DISTRO KEY          press a key on the VM's keyboard (QEMU sendkey), e.g. `key fedora-43 esc`
#                           to leave the GNOME Overview a login lands in, so new windows get focus
#   stop DISTRO             power off
#   snapshot DISTRO create|apply|list [NAME]   internal qcow2 snapshots (the VM must be off)
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
        *) die "unknown distro '$1' (ubuntu-22.04, fedora-43, arch)" ;;
    esac
}

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
    # shellcheck disable=SC2046
    for _ in $(seq 1 180); do
        ssh $(ssh_opts "$1") tester@localhost true 2>/dev/null && return 0
        sleep 5
    done
    die "$1 did not answer on SSH within 15 minutes"
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
[ -n "$cmd" ] && [ -n "$distro" ] || { sed -n '9,25p' "$0"; exit 64; }
port "$distro" >/dev/null
shift 2

# shellcheck disable=SC2046
case $cmd in
    install) install_vm "$distro" ;;
    start)
        display=()
        [ "${1:-}" = --headless ] && display=(--display none)
        # ⚠️ quickemu adds an SSH forward of its own on the first free port from 22220 up. With a
        # Windows guest on 22220 that is 22221, the forward ubuntu-22.04.conf already has, and QEMU
        # refuses the duplicate ("Could not set up host forwarding rule"). Name one nothing uses.
        (cd "$VMS" && quickemu --vm "$distro.conf" --ssh-port "$(($(port "$distro") + 10))" "${display[@]}")
        ;;
    wait) wait_ssh "$distro" ;;
    ssh) exec ssh $(ssh_opts "$distro") tester@localhost "$@" ;;
    session) exec ssh $(ssh_opts "$distro") tester@localhost "$SESSION_ENV"$'\n'"$*" ;;
    prepare)
        wait_ssh "$distro"
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
    stop) ssh $(ssh_opts "$distro") tester@localhost 'sudo systemctl poweroff' || true ;;
    key) printf 'sendkey %s\n' "${1:?key}" | socat - "unix-connect:$VMS/$distro/$distro-monitor.socket" >/dev/null ;;
    snapshot)
        disk=$VMS/$distro/disk.qcow2
        case ${1:-} in
            create) qemu-img snapshot -c "${2:?name}" "$disk" ;;
            apply) qemu-img snapshot -a "${2:?name}" "$disk" ;;
            list) qemu-img snapshot -l "$disk" ;;
            *) die "snapshot create|apply|list [NAME]" ;;
        esac
        ;;
    *) die "unknown command '$cmd'" ;;
esac
