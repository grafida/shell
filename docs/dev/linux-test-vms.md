# Clean Linux test VMs (Linux host)

Four local VMs for testing a **packaged** app (an extracted Linux tarball) on machines that have
never seen a build tool: the clean-VM half of grafida/grafida#86. Each is set up unattended from
its distribution's own image, updated, and snapshotted as `clean` before anything of ours touches
it. `scripts/dev/linuxvm.sh` does all of it.

| VM | Installer | Desktop | SSH |
|---|---|---|---|
| `ubuntu-22.04` | live-server ISO, subiquity autoinstall, `ubuntu-desktop-minimal` | GNOME (Wayland), GDM | `localhost:22221` |
| `fedora-43` | Everything netinstall, kickstart, `@^workstation-product-environment` | GNOME (Wayland), GDM | `localhost:22222` |
| `arch` | archiso, its `script=` boot hook, `plasma-meta` | KDE Plasma (Wayland), SDDM | `localhost:22223` |
| `ubuntu-22.04-arm64` | Ubuntu's signed arm64 cloud image, cloud-init (NoCloud); **emulated** (TCG) | none, always headless | `localhost:22225` |

Ubuntu 22.04 is the floor (glibc 2.35, the oldest WebKitGTK 4.1 we support); Fedora and Arch show
what the newest WebKitGTK does. The first three are x86_64 under KVM. Each logs in automatically and
never locks or blanks the screen, so there is always a desktop session to run the app in.
`ubuntu-22.04-arm64` is the floor for the arm64 tarball, for the checks that need no desktop
(section 5). Every VM has the user `tester` (password `tester`, sudo without a password). The SSH
key is `~/.ssh/grafida-linuxvm_ed25519`, created on the first install; `~/.ssh/config` is not
touched.

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

`ubuntu-22.04-arm64` needs no ISO: its `install` fetches the cloud image itself (section 5).

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
- **quickemu adds an SSH forward of its own**, on the first free port from 22220 up. Beside a running
  Windows guest (22220) that was 22221, which `ubuntu-22.04.conf` already forwards, and QEMU refused
  to start ("Could not set up host forwarding rule"). `start` therefore names quickemu's port
  itself: 22231–22233, unused; the VMs are still reached on 22221–22223.

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

## 4. Real input: pointer, keys and screendumps from the host

The automation channel's `eval` and `click` run inside the page. A scripted `el.click()` or a
synthetic event never goes through the compositor, the toolkit or WebKit's input handling, so it
cannot show what a person's input does: where the **keyboard focus** goes, `:hover`, **drags**,
modifier clicks, context menus, and anything the compositor decides (focus on map, the Overview).
For those, drive the VM's own input devices from the host. The release binary needs no automation
channel for it; use `eval` only to read state afterwards.

```sh
scripts/dev/linuxvm.sh key fedora-43 esc                     # leave the GNOME Overview (below)
scripts/dev/linuxvm.sh screen fedora-43                      # 1280x800
scripts/dev/linuxvm.sh screendump fedora-43 /tmp/screen.png  # .png, or .ppm
scripts/dev/linuxvm.sh move fedora-43 110 412
scripts/dev/linuxvm.sh click fedora-43 110 412               # left; or `right`, `middle`
scripts/dev/linuxvm.sh click fedora-43 700 669 right ctrl-shift
scripts/dev/linuxvm.sh drag fedora-43 700 669 900 719
```

- **Every `start` opens a QMP socket**, `~/VMs/<distro>/<distro>-qmp.socket`, beside the HMP monitor
  socket quickemu makes. `key` goes through HMP `sendkey`; `move`, `click`, `drag`, `screen` and
  `screendump` go through QMP (`scripts/dev/linuxvm/qmp.py`). A VM started before this existed has no
  QMP socket: stop it and start it again.
- **The pointer is absolute.** `move` sends QMP `input-send-event` absolute axes, which reach the guest
  as quickemu's USB tablet and land on the exact pixel (checked against the page's `clientX`/`clientY`
  on all three VMs). HMP `mouse_move` is the trap: it sends *relative* motion, which arrives as the PS/2
  mouse and goes through the desktop's pointer acceleration, so it ends up near the target, not on it.
- **Coordinates are screen pixels, as a screendump shows them**, origin top left. The absolute range is
  0…32767 across the whole screen whatever its size, so every command first reads the screen's current
  size from a screendump (the size of QEMU's display surface, which is the area the tablet's range
  covers) and aims at the centre of the pixel. A resolution change in the guest (`kscreen-doctor
  output.Virtual-1.mode.13` on Plasma) is picked up by the next command.
- **From a point in the page to the screen:** screen = window's webview origin + CSS point ×
  `devicePixelRatio`. Wayland tells no client where its window is, so find the origin from a
  screendump. One way is to match the automation channel's `screenshot` of the page inside it, with
  tolerance: the webview's snapshot and the compositor anti-alias a few rounded edges differently.
  Then confirm it with one move: the page's `mousemove` `clientX`/`clientY` must be the point you
  aimed at.
- **`click` comes from two pixels away first**, so the client sees a motion onto the target and its
  hover state is there before the press, even when the pointer was already on it. Modifiers are
  QMP key events held around the button. They go through a different device from the tablet, and
  on Ubuntu 22.04 a press sent 30–60 ms after them sometimes arrived without them, so the command waits
  0.2 s after the modifiers before pressing. `drag` presses, moves in 10–60 small steps (a toolkit
  starts a drag only past its threshold, and a drop target only learns of the pointer from motion
  over it), and releases at the end.
- **GNOME logs in to the Overview** (Ubuntu 22.04 and Fedora 43). Until `key <distro> esc` closes it, a
  new window does not get the keyboard focus, so keys go nowhere and anything that needs a focused
  window fails. Plasma lands on the desktop.
- **A screendump shows the pointer only if the guest draws it into the framebuffer.** Fedora's GNOME
  does; Ubuntu 22.04's does not. So prove a move by its
  effect, a `:hover` style in the screendump or the page's own event, not by looking for the arrow.

## 5. The arm64 guest (`ubuntu-22.04-arm64`)

No arm64 machine is at hand, so the arm64 tarball is checked in an **emulated** Ubuntu 22.04 arm64
guest (grafida/grafida#93). It replaces the CI job that did this on GitHub's `ubuntu-22.04-arm`
runners (Grafida, `48f6290^:.github/workflows/shell-linux.yml`). It covers what needs no desktop:
the worker, a headless launch, `install.sh` and the launcher. It does **not** stand in for arm64
hardware: everything runs many times slower than on a real CPU, and the guest has no desktop for
the launcher's graphical alert or any interactive check.

Host packages (Arch): `qemu-system-aarch64`, `edk2-aarch64` (the UEFI firmware), `gnupg`,
`cdrtools` (`genisoimage`). Debian/Ubuntu: `qemu-system-arm`, `qemu-efi-aarch64`, `gnupg`,
`genisoimage`.

```sh
scripts/dev/linuxvm.sh install ubuntu-22.04-arm64   # ~1 min: fetch, verify, disk, seed
scripts/dev/linuxvm.sh start ubuntu-22.04-arm64     # always headless
scripts/dev/linuxvm.sh wait ubuntu-22.04-arm64      # the first boot (cloud-init) takes minutes
scripts/dev/linuxvm.sh prepare ubuntu-22.04-arm64   # update; powers off and waits for QEMU to exit
scripts/dev/linuxvm.sh snapshot ubuntu-22.04-arm64 create clean
```

- **The image is Ubuntu's own**, `ubuntu-22.04-server-cloudimg-arm64.img` from
  `cloud-images.ubuntu.com/releases/22.04/release/`. `install` checks it against that directory's
  `SHA256SUMS`, whose signature must be by `D2EB 4462 6FDD C30B 513D 5BB7 1A5D 6C4C 7DB8 7C81`
  ("UEC Image Automatic Signing Key"), the key Ubuntu's `ubuntu-keyring` package ships as
  `ubuntu-cloudimage-keyring.gpg`. The key is fetched by its full fingerprint into a throwaway
  keyring. The image is kept in `~/VMs/ubuntu-22.04-arm64/download/` and fetched again only when
  it no longer matches the signed `SHA256SUMS`.
- **cloud-init** (a NoCloud `cidata` seed, `scripts/dev/linuxvm/ubuntu-22.04-arm64.user-data`)
  creates `tester` on the first boot, then disables itself, so later boots and snapshots keep that
  configuration. Its network configuration matches the NIC by MAC, so `start` always gives the
  guest the same one.
- **QEMU runs directly, not under quickemu**: `-machine virt,gic-version=3 -accel tcg,thread=multi
  -cpu max,pauth-impdef=on -smp 8 -m 8G`, UEFI, `-display none`, serial console to `console.log`,
  pid in `qemu.pid`. `pauth-impdef=on` matters: QEMU's default pointer-authentication algorithm
  makes the guest several times slower. The emulation takes up to 8 host threads; `renice -n 19 -p
  "$(cat ~/VMs/ubuntu-22.04-arm64/qemu.pid)"` keeps it behind everything else.
- **`stop` waits for QEMU to exit**, and `snapshot` refuses while the guest runs. The UEFI variables
  (`efivars.fd`, the boot entries) are outside the disk, so each snapshot keeps its own copy
  (`efivars.fd.<name>`), restored by `snapshot … apply`.
- `session`, `key`, `move`, `click`, `drag`, `screen` and `screendump` need a desktop and refuse
  this guest.

### The tarball check, as the CI job ran it

Each from `clean` (`snapshot ubuntu-22.04-arm64 apply clean`, `start`, `wait`), with the tarball
copied in (`scp -P 22225 -i ~/.ssh/grafida-linuxvm_ed25519 … tester@localhost:`) and extracted to
`~/pkg`:

1. **`tests/Pipe` against the packaged worker, with PHP 8.4.** The job's `shivammathur/setup-php`
   takes PHP on Ubuntu from `ppa:ondrej/php`, so the guest does too: `sudo add-apt-repository -y
   ppa:ondrej/php`, then `php8.4-cli php8.4-curl php8.4-xml php8.4-mbstring php8.4-sqlite3` (the
   job's extension list). Copy Grafida's source in with its `vendor/` (no `.git`, no
   `node_modules`) and run `GRAFIDA_PIPE_WORKER=$HOME/pkg/grafida/grafida-worker vendor/bin/phpunit
   --testsuite=pipe`. About 2 minutes under emulation.
2. **The headless launch.** `sudo apt-get install -y libwebkit2gtk-4.1-0 libgtk-3-0 xvfb dbus`;
   point `HOME` and every `XDG_*` at a temp directory; `dbus-run-session -- xvfb-run -a
   <pkg>/grafida/grafida`; wait for `worker ready` in `$XDG_STATE_HOME/grafida/logs/shell.log`,
   with no `giving up` in it; then `pkill -TERM -x grafida-shell` must leave no `grafida-worker`.
   ⚠️ Set the package path **before** moving `HOME`: a `~/pkg/…` after it points into the empty
   temp home. Allow longer than the job's 60 s, since under emulation the worker is ready only
   after several seconds.
3. **`install.sh` and a launch with no libraries**, on a fresh `clean`: `./install.sh`, then run the
   `Exec=` of `~/.local/share/applications/app.grafida.desktop`. Over SSH there is no display and no
   alert program, so the launcher prints its missing-libraries message on stderr and exits 69.

`parity-linux.sh` also runs here, packaged, with the `debug-automation-linux-aarch64` artifact, in
`dbus-run-session -- xvfb-run -a -s "-screen 0 1280x800x24"` (about 4 minutes), but under TCG
four rows fail for timing alone (RC5, grafida/grafida#93):
- "A worker that never sends ready": the row's fixed `sleep 3` ends before the shell's 2 s `ready`
  timeout fires;
- "Ctrl+Shift+V": `parity/grafida-editor.js` calls `navigator.clipboard.writeText()` after
  TinyMCE's initialisation, which takes about 6 s here; by then the eval's user gesture (about 1 s)
  has expired, so `writeText()` fails with `NotAllowedError`;
- "drop on the editor": fails because the Ctrl+Shift+V eval threw and returned no drop
  coordinates;
- "Idle CPU": measures an emulated CPU, so it says nothing about real hardware.

Re-check the first three by hand, with a longer wait and with the clipboard written in an eval of
its own. Treat the idle-CPU row as not measured.
