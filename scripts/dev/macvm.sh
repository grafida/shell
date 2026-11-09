#!/usr/bin/env bash
# Grafida Shell — clean macOS test VMs for packaged apps (grafida/grafida#88).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Two guests, each restored by tart from one of Apple's own IPSW restore images (not a preconfigured
# image), taken through Setup Assistant by hand, updated, and kept as a `<name>-clean` VM that nothing
# of ours has touched. Every test run works on a throwaway clone of it. See docs/dev/macos-test-vms.md.
#
#   macos-12   the floor (ADR 0001): restored from 12.6, updated in the guest to the last 12.x
#   macos-27   the current release
#
# Usage: scripts/dev/macvm.sh COMMAND [NAME] [ARGS…]
#   fetch NAME              download the IPSW into $IPSW_DIR and check its SHA-256 (pinned below)
#   create NAME             restore a new VM NAME from its IPSW (then do Setup Assistant by hand)
#   run NAME [--no-graphics]  boot it (a window, unless --no-graphics); returns at once, logs to $TART_HOME
#   ip NAME                 the guest's IP address, once it has one
#   wait NAME               wait until SSH answers
#   ssh NAME [CMD…]         a shell or a command, as user `tester`
#   prepare NAME            install the SSH key and passwordless sudo; no sleep, screen saver or auto-updates
#   update NAME             the newest release of the guest's own major version, then (run again) the
#                           recommended updates; it restarts the guest when an OS install needs it
#   stop NAME               shut it down (tart stop)
#   seal NAME               NAME (stopped) becomes NAME-clean: the clean state every run starts from
#   fresh NAME              throw away NAME and clone it again from NAME-clean
#   list                    tart list
#
# The VMs live in $TART_HOME and the IPSWs in $IPSW_DIR, both set in scripts/dev/macvm.conf, which is
# git-ignored (copy macvm.conf.sample). Apple's licence allows two macOS guests at a time.

set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CONF=${MACVM_CONF:-$HERE/macvm.conf}
[ -f "$CONF" ] || { echo "macvm: no $CONF; copy $HERE/macvm.conf.sample and edit it" >&2; exit 1; }
# shellcheck source-path=SCRIPTDIR source=macvm.conf.sample
. "$CONF"
: "${TART_HOME:?set TART_HOME in $CONF}"
: "${IPSW_DIR:?set IPSW_DIR in $CONF}"
export TART_HOME
KEY=${MACVM_KEY:-$HOME/.ssh/grafida-macvm_ed25519}
USER_NAME=tester
# The guest account's password. macOS refuses one that contains the account name, so not "tester".
PASSWORD=${MACVM_PASSWORD:-grafida-vm}

die() { echo "macvm: $*" >&2; exit 1; }

# The restore image per guest: Apple's URL and the SHA-256 ipsw.me publishes for it, checked here
# against the download. 12.6 is the newest VirtualMac restore image of macOS 12.
ipsw() {
    case $1 in
        macos-12) echo "https://updates.cdn-apple.com/2022FallFCS/fullrestores/012-40537/0EC7C669-13E9-49FB-BD64-9EECC1D174B2/UniversalMac_12.6_21G115_Restore.ipsw 5113f8d3c77fd725ea6357ced5764bc57183d2d388492c58c46ff2729d0658e5" ;;
        macos-27) echo "https://updates.cdn-apple.com/2026FallFCS/c9671975-52db-4fb5-9af2-00217169ba8d/UniversalMac_27.2_26B123_Restore.ipsw 6613b34e8589db2202a01ec37a2648afb141ece13428175183f8f3e38438534f" ;;
        *) die "unknown guest '$1' (macos-12, macos-27)" ;;
    esac
}

ipsw_file() { local url; read -r url _ <<<"$(ipsw "$1")"; echo "$IPSW_DIR/$(basename "$url")"; }

fetch() {
    local url sha file
    read -r url sha <<<"$(ipsw "$1")"
    file=$(ipsw_file "$1")
    mkdir -p "$IPSW_DIR"
    if [ ! -f "$file" ]; then
        curl -fL --retry 3 -C - -o "$file.part" "$url"
        mv "$file.part" "$file"
    fi
    echo "checking $(basename "$file") …"
    [ "$(shasum -a 256 "$file" | awk '{print $1}')" = "$sha" ] \
        || die "$file does not match its pinned SHA-256 ($sha); delete it and fetch again"
    echo "ok: $file"
}

ssh_opts() {
    echo -i "$KEY" -o IdentitiesOnly=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o LogLevel=ERROR -o ConnectTimeout=5
}

guest_ip() { tart ip "$1" 2>/dev/null; }

wait_ssh() {
    local ip=''
    for _ in $(seq 1 120); do
        ip=$(guest_ip "$1" || true)
        # shellcheck disable=SC2046
        if [ -n "$ip" ] && ssh $(ssh_opts) -o BatchMode=yes "$USER_NAME@$ip" true 2>/dev/null; then
            echo "$ip"; return 0
        fi
        sleep 5
    done
    die "$1 did not answer SSH (is Remote Login on, and was \`prepare\` run?)"
}

command=${1:-}; shift || true
case $command in
    list) exec tart list ;;
    '' | -h | --help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac

name=${1:?guest name (macos-12, macos-27)}; shift
ipsw "$name" >/dev/null

case $command in
    fetch) fetch "$name" ;;
    create)
        fetch "$name"
        tart create "$name" --from-ipsw "$(ipsw_file "$name")" --disk-size 80
        echo "Now: scripts/dev/macvm.sh run $name, and do Setup Assistant by hand (docs/dev/macos-test-vms.md)."
        ;;
    run)
        mkdir -p "$TART_HOME/logs"
        nohup tart run "$name" "$@" >"$TART_HOME/logs/$name.log" 2>&1 &
        echo "started $name (pid $!, log $TART_HOME/logs/$name.log)"
        ;;
    ip) tart ip "$name" --wait 120 ;;
    wait) wait_ssh "$name" ;;
    ssh)
        ip=$(guest_ip "$name") || die "$name has no IP; is it running?"
        # shellcheck disable=SC2046
        exec ssh $(ssh_opts) "$USER_NAME@$ip" "$@"
        ;;
    prepare)
        [ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N '' -C grafida-macvm -f "$KEY"
        ip=$(guest_ip "$name") || die "$name has no IP; is it running?"
        # The one step that needs tester's password. OpenSSH takes it from an askpass helper
        # (SSH_ASKPASS_REQUIRE=force, no terminal needed); password auth is asked for explicitly,
        # since a user's own ~/.ssh/config may prefer publickey only. sudo reads it from stdin.
        echo "Installing the SSH key and passwordless sudo for $USER_NAME …"
        askpass=$(mktemp -t macvm-askpass)
        trap 'rm -f "$askpass"' EXIT
        # shellcheck disable=SC2016 # expanded by the helper, from its environment
        printf '#!/bin/sh\necho "$MACVM_ASKPASS_PASSWORD"\n' > "$askpass"
        chmod 700 "$askpass"
        MACVM_ASKPASS_PASSWORD=$PASSWORD SSH_ASKPASS=$askpass SSH_ASKPASS_REQUIRE=force \
            ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
            -o PubkeyAuthentication=no -o PasswordAuthentication=yes -o KbdInteractiveAuthentication=yes \
            -o PreferredAuthentications=keyboard-interactive,password -o NumberOfPasswordPrompts=1 \
            "$USER_NAME@$ip" "
                set -e
                mkdir -p ~/.ssh && chmod 700 ~/.ssh
                touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
                grep -qxF '$(cat "$KEY.pub")' ~/.ssh/authorized_keys || echo '$(cat "$KEY.pub")' >> ~/.ssh/authorized_keys
                echo '$PASSWORD' | sudo -S -p '' sh -c 'echo \"$USER_NAME ALL=(ALL) NOPASSWD: ALL\" > /etc/sudoers.d/$USER_NAME && chmod 440 /etc/sudoers.d/$USER_NAME'
            " </dev/null
        rm -f "$askpass"
        # shellcheck disable=SC2046,SC2029
        ssh $(ssh_opts) "$USER_NAME@$ip" "PASSWORD='$PASSWORD' sh -s" <<'GUEST'
            set -e
            sudo pmset -a sleep 0 displaysleep 0 disksleep 0
            defaults -currentHost write com.apple.screensaver idleTime -int 0
            # A locked screen hides every window, and WebKit then clamps a page's timers to 1 s: a
            # timing check would measure the lock (macOS 27 locked after its update's restart).
            sysadminctl -screenLock off -password "$PASSWORD"
            sudo defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload -bool false
            sudo defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticallyInstallMacOSUpdates -bool false
            sw_vers
GUEST
        ;;
    update)
        # Bring the guest to the newest release of ITS major version (12 → 12.7.x), never the next
        # major. A macOS 12 VM restored from 12.6 is offered no 12.7.x by Software Update at all, only
        # the full installer, so: the newest same-major full installer through startosinstall (on
        # Apple Silicon it needs the volume owner's password, given on stdin), and after that, run
        # again, the recommended updates (Safari, which also replaces the system WebKit).
        ip=$(guest_ip "$name") || die "$name has no IP; is it running?"
        # The password is meant to expand here, on the host.
        # shellcheck disable=SC2046,SC2029
        ssh $(ssh_opts) "$USER_NAME@$ip" "PASSWORD='$PASSWORD' sh -s" <<'GUEST'
            set -e
            current=$(sw_vers -productVersion); major=${current%%.*}
            labels=$(softwareupdate --list 2>&1 | sed -n 's/^\* Label: //p')
            # 1. An ordinary update of this major version, when Software Update offers one.
            os_label=$(printf '%s\n' "$labels" | grep -E "^macOS( [A-Za-z ]+)? $major\." | head -1 || true)
            if [ -n "$os_label" ]; then
                echo "running $current; installing '$os_label' (the guest restarts)"
                # On Apple Silicon an OS update needs the volume owner's credentials.
                echo "$PASSWORD" | sudo softwareupdate --install "$os_label" --agree-to-license \
                    --restart --user "$USER" --stdinpass
                exit 0
            fi
            # 2. Otherwise the newest full installer of this major version (a 12.6 VM is offered no
            #    12.7.x update at all, only the full installer).
            newest=$(softwareupdate --list-full-installers 2>/dev/null \
                | sed -n "s/.*Version: \($major\.[0-9.]*\),.*/\1/p" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)
            echo "running $current; newest full installer of macOS $major: ${newest:-none}"
            if [ -n "$newest" ] && [ "$(printf '%s\n%s\n' "$current" "$newest" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)" != "$current" ]; then
                sudo softwareupdate --fetch-full-installer --full-installer-version "$newest"
                app=$(ls -d /Applications/Install\ macOS*.app | head -1)
                echo "installing $newest from $app; the guest restarts"
                # --cleanup is not offered on every release: the installer is removed by hand after.
                echo "$PASSWORD" | sudo "$app/Contents/Resources/startosinstall" --agreetolicense \
                    --nointeraction --forcequitapps --user "$USER" --stdinpass
                exit 0
            fi
            # 3. Up to date: the other updates, by label (Safari, which replaces the system WebKit;
            #    background security updates). NEVER --recommended: on a 12.7.6 guest it includes the
            #    upgrade to the current major release, and downloads and prepares it.
            sudo rm -rf /Applications/Install\ macOS*.app
            others=$(printf '%s\n' "$labels" | grep -v -E '^macOS' || true)
            if [ -n "$others" ]; then
                printf 'installing: %s\n' "$others"
                printf '%s\n' "$others" | while IFS= read -r label; do
                    sudo softwareupdate --install "$label" --agree-to-license
                done
            fi
            skipped=$(printf '%s\n' "$labels" | grep -E '^macOS' || true)
            [ -z "$skipped" ] || printf 'not installed (another major version): %s\n' "$skipped"
            echo "up to date: $(sw_vers -productVersion)"
GUEST
        ;;
    stop) tart stop "$name" ;;
    seal)
        tart list | awk -v n="$name" '$2 == n && $NF == "running" { exit 1 }' || die "stop $name first"
        if tart list | awk -v n="$name-clean" '$2 == n { found = 1 } END { exit !found }'; then
            die "$name-clean exists; delete it (tart delete $name-clean) to reseal"
        fi
        tart clone "$name" "$name-clean"
        echo "sealed: $name-clean. Every run: scripts/dev/macvm.sh fresh $name"
        ;;
    fresh)
        tart list | awk -v n="$name-clean" '$2 == n { found = 1 } END { exit !found }' || die "no $name-clean; seal it first"
        tart stop "$name" >/dev/null 2>&1 || true
        tart delete "$name" >/dev/null 2>&1 || true
        tart clone "$name-clean" "$name"
        echo "$name is a fresh clone of $name-clean"
        ;;
    *) die "unknown command '$command' (see --help)" ;;
esac
