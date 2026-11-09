# Clean Linux test VMs (Linux host)

Three local VMs for testing a **packaged** app (an extracted Linux tarball) on machines that have
never seen a build tool: the clean-VM half of grafida/grafida#86. Each is installed unattended by
its distribution's own installer, updated, and snapshotted as `clean` before anything of ours
touches it. `scripts/dev/linuxvm.sh` does all of it.

| VM | Installer | Desktop | SSH |
|---|---|---|---|
| `ubuntu-22.04` | live-server ISO, subiquity autoinstall, `ubuntu-desktop-minimal` | GNOME (Wayland), GDM | `localhost:22221` |
| `fedora-43` | Everything netinstall, kickstart, `@^workstation-product-environment` | GNOME (Wayland), GDM | `localhost:22222` |
| `arch` | archiso, its `script=` boot hook, `plasma-meta` | KDE Plasma (Wayland), SDDM | `localhost:22223` |

Ubuntu 22.04 is the floor (glibc 2.35, the oldest WebKitGTK 4.1 we support); Fedora and Arch show
what the newest WebKitGTK does. Every VM has the user `tester` (password `tester`, sudo without a
password), logs in automatically, and never locks or blanks the screen, so there is always a desktop
session to run the app in. The SSH key is `~/.ssh/grafida-linuxvm_ed25519`, created on the first
install; `~/.ssh/config` is not touched.

## 1. The ISOs

The VMs live in `~/VMs` (`LINUXVM_DIR`), beside the Windows one.

```sh
cd ~/VMs
quickget ubuntu-server 22.04      # checks the SHA-256 itself
quickget archlinux latest
mkdir -p fedora-43 && cd fedora-43
B=https://download.fedoraproject.org/pub/fedora/linux/releases/43/Everything/x86_64/iso
curl -fLO "$B/Fedora-Everything-43-1.6-x86_64-CHECKSUM"
curl -fLO "$B/Fedora-Everything-netinst-x86_64-43-1.6.iso"
grep 'SHA256 (Fedora-Everything-netinst' Fedora-Everything-43-1.6-x86_64-CHECKSUM \
  | sed -E 's/SHA256 \((.*)\) = (.*)/\2  \1/' | sha256sum -c -
```

Fedora's Workstation **live** ISO (what `quickget fedora 43 Workstation` fetches) cannot be
installed unattended: its installer takes no kickstart. The Everything netinstall with the
Workstation package environment installs the same package set.

## 2. Install, update, snapshot

```sh
scripts/dev/linuxvm.sh install ubuntu-22.04     # 15–40 min; ~/VMs/ubuntu-22.04/install.log
scripts/dev/linuxvm.sh start ubuntu-22.04 --headless
scripts/dev/linuxvm.sh prepare ubuntu-22.04     # update; no screen lock; powers off
scripts/dev/linuxvm.sh snapshot ubuntu-22.04 create clean
```

The install boots the ISO's kernel directly with QEMU (`-kernel`/`-append`), which is how the
unattended switches reach each installer without remastering the ISO. The install logs its serial
console to `install.log` and powers off when done. Afterwards the VM runs under quickemu
(`~/VMs/<distro>.conf`).

Gotchas:

- **Ubuntu** asks "Continue with autoinstall?" unless `autoinstall` is on the kernel command line.
  The direct kernel boot puts it there. The user-data comes from a `cidata` volume (NoCloud).
- **Fedora** reads the kickstart from a volume labelled `OEMDRV` (`inst.ks=hd:LABEL=OEMDRV:/ks.cfg`)
  and needs `inst.stage2` pointing at the ISO's own label.
- **Arch** downloads the `script=` URL in the live system, so `linuxvm.sh` serves it on the
  host's loopback (port 18800), which QEMU's user network presents to the guest as `10.0.2.2`.
- GNOME's first-login tour is skipped (`~/.config/gnome-initial-setup-done`). Without that, it
  would sit over the session every time.

## 3. Testing a packaged app

Before every run: `snapshot DISTRO apply clean`. Then:

```sh
scripts/dev/linuxvm.sh start fedora-43                 # with a window, to watch and for the by-hand checks
scp -P 22222 -i ~/.ssh/grafida-linuxvm_ed25519 Grafida-*-linux-amd64-shell-preview.tar.gz tester@localhost:
scripts/dev/linuxvm.sh ssh fedora-43 'tar xzf Grafida-*.tar.gz && grafida/install.sh'
scripts/dev/linuxvm.sh session fedora-43 'grafida/grafida'   # starts it in the desktop session
```

`session` runs a command with the logged-in desktop's `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR` and
D-Bus address, which is what a GUI program over SSH needs. For the automated parity checks, copy
the shell repository's `scripts/` and `tests/` and the release workflow's
`debug-automation-linux-x86_64` artifact over, and run
`GRAFIDA_PACKAGED=~/grafida SHELL_BIN=~/grafida-shell-debug scripts/dev/parity-linux.sh` through
`session`. It needs only `python3` on the VM. The release binary never has the automation
channel, so this is the only way in.
