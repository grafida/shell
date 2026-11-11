# Windows 11 test VM (Linux host)

A local Windows 11 VM for running and testing the shell on WebView2, driven from the host
(including from Claude Code) over SSH. This is the procedure that produced the Windows results in
[ADR 0001](../adr/0001-shell-stack.md). Every gotcha below was actually hit.

The end state:

- a quickemu/KVM VM in `~/VMs/windows-11`, with an admin account `Quickemu` / `quickemu` that
  logs on automatically, so there is always an interactive desktop;
- `ssh winvm` from the host, with a dedicated key, Windows PowerShell as the remote shell;
- no sleep and no lock screen;
- a disk snapshot `clean` to return to before every installer test.

## 1. Host prerequisites

An x86_64 Linux host with KVM (`/dev/kvm`). Install QEMU, swtpm and quickemu with your package
manager; for example, on Arch:

```sh
sudo pacman -S --needed qemu-desktop swtpm
yay -S quickemu
```

Plus `mkisofs` (cdrtools) and, to screenshot the guest before its GPU driver loads, `socat` and
ImageMagick.

> Claude Code cannot do this step itself: `sudo` needs an interactive terminal.

## 2. Create the VM and fetch the ISOs

```sh
mkdir -p ~/VMs && cd ~/VMs
quickget windows 11
```

⚠️ **`quickget` exits 0 even when its downloads failed.** In our run, both failed:

- Microsoft blocked the automated Windows ISO request "based on your IP address". The log says so,
  but nothing else does: no `windows-11.iso` is written.
- fedorapeople.org answered the VirtIO driver request with an anti-bot ("Making sure you're not a
  bot!") HTML page, which quickget saved as `virtio-win.iso`, about 4 KB.

Check before going on:

```sh
file ~/VMs/windows-11/*.iso    # each must say "ISO 9660 CD-ROM filesystem data"
```

If either is missing or is HTML, download it in a browser, and save it into `~/VMs/windows-11/`
(not `~/VMs/`) under the name `windows-11.conf` expects:

| File | Source |
|---|---|
| `windows-11.iso` | <https://www.microsoft.com/en-us/software-download/windows11>, "Download Windows 11 Disk Image (ISO) for x64 devices" |
| `virtio-win.iso` | <https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso> |

quickget will already have made the VM config (`~/VMs/windows-11.conf`) and the unattended-install
ISO.

## 3. Add SSH to the unattended install (before the first boot)

Make a key used only for this VM. It has no passphrase, which is acceptable for a local throwaway
VM and keeps any hardware-backed personal key out of it:

```sh
ssh-keygen -t ed25519 -N "" -C "grafida-winvm (local test VM)" -f ~/.ssh/grafida-winvm_ed25519
```

Patch quickemu's answer file and rebuild `unattended.iso`:

```sh
scripts/dev/winvm-unattend.py ~/VMs/windows-11 ~/.ssh/grafida-winvm_ed25519.pub
```

It appends first-logon commands that:

- install and start OpenSSH Server;
- widen its firewall rule to every network profile;
- authorise the key for the Administrators group;
- make Windows PowerShell the SSH shell;
- turn off sleep and the lock screen.

The original answer file is kept as `autounattend.xml.orig`, and running the script twice is a
no-op.

Why each command exists:

- ⚠️ **Firewall:** OpenSSH's rule `OpenSSH-Server-In-TCP` applies to the **Private** profile only,
  but Windows 11 puts the VM's network in **Public**, ignoring the answer file's
  `NetworkLocation=Home`. Without the fix, sshd runs and `ssh` hangs with "Connection timed out
  during banner exchange": QEMU accepts the host side, and the guest silently drops the packets.
- **Keys:** for a member of Administrators, sshd reads
  `C:\ProgramData\ssh\administrators_authorized_keys`, not `~\.ssh\authorized_keys`. It ignores the
  file unless only Administrators and SYSTEM can access it, hence the `icacls`.

Add the host entry to `~/.ssh/config`:

```
Host winvm
    HostName localhost
    Port 22220
    User Quickemu
    IdentityFile ~/.ssh/grafida-winvm_ed25519
    IdentitiesOnly yes
    StrictHostKeyChecking accept-new
    UserKnownHostsFile ~/.ssh/known_hosts.winvm
    LogLevel ERROR
```

A separate `known_hosts` file is used because rebuilding the VM changes its host key. `LogLevel
ERROR` hides the "not using a post-quantum key exchange" warning that Windows' bundled OpenSSH
triggers on every connection.

## 4. Install

```sh
cd ~/VMs && quickemu --vm windows-11.conf --ssh-port 22220
```

QEMU stays in the foreground (run it in the background, or from its own terminal). Setup stops
twice: quickemu's answer file has no Windows PE language section, so press **Next** on the
language page and on the keyboard page. Everything after that is unattended: edition, disk, the
account, and several reboots. Then the desktop appears, the first-logon commands run for a few
minutes (OpenSSH comes from Windows Update), and their console window closes.

Done when:

```sh
ssh winvm 'whoami; query user'    # quickemu, with an Active console session
```

**Seeing the guest from the host.** Until Windows loads its virtio GPU driver, the QEMU monitor
can screenshot and type:

```sh
M=~/VMs/windows-11/windows-11-monitor.socket
echo "screendump /tmp/vm.ppm" | socat -t3 - UNIX-CONNECT:$M && magick /tmp/vm.ppm /tmp/vm.png
echo "sendkey ret"            | socat -t3 - UNIX-CONNECT:$M
```

Without `-t3`, socat may close the socket before QEMU runs the command. Delete the old `.ppm`
first, or a failed dump silently leaves you looking at a stale image.

⚠️ After the GPU driver loads (from the desktop onward), `screendump` answers **"Error: no surface"**
(the display is `virtio-vga-gl`). There is no guest-agent channel either, so until SSH works the
host is blind, and someone has to look at the QEMU window.

## 5. Snapshot

Take the snapshot with the VM **powered off**: quickemu's snapshots are `qemu-img` snapshots of
the disk image.

```sh
ssh winvm 'Stop-Computer -Force'
# wait until the qemu process for windows-11 has exited, then:
cd ~/VMs && quickemu --vm windows-11.conf --snapshot create clean
qemu-img snapshot -l windows-11/disk.qcow2     # must list "clean"
```

Restore it before every installer test, which is what makes "works on a clean machine" a real claim:

```sh
cd ~/VMs && quickemu --vm windows-11.conf --snapshot apply clean
```

The snapshot covers the disk only, not the UEFI variables or the TPM state. That is enough for a
test VM.

## 6. Running GUI programs from the host

⚠️ **A process started over SSH runs in a non-interactive session and cannot show a window.**
Run it in the logged-on user's desktop session through an interactive scheduled task:

```sh
scp myapp.exe winvm:C:/spike/
ssh winvm 'schtasks /Create /TN Run1 /TR "C:\spike\myapp.exe" /SC ONCE /ST 23:59 /IT /RU $env:USERNAME /F; schtasks /Run /TN Run1'
ssh winvm 'schtasks /Delete /TN Run1 /F'      # afterwards
```

`/ST` only has to be syntactically valid, since the task is started with `/Run`. A start time in
the past just makes `schtasks` print a warning. To capture output or measure the program, run a
PowerShell script through the task instead. `spikes/windows/run.ps1` (report + idle CPU of the
process tree) and `spikes/windows/dialog.ps1` (run once, wait for exit) are working examples.

Build Windows binaries on the host and only *run* them in the VM, which keeps it close to a clean
machine:

```sh
RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release --target x86_64-pc-windows-msvc
```

## 7. Day to day: `scripts/dev/winvm.sh` (grafida/grafida#91)

Everything above is the one-off build. Using the VMs afterwards goes through `scripts/dev/winvm.sh
COMMAND VM`, with VM `windows-11` (port 22220, ssh host `winvm`) or `windows-10` (port 22224,
`winvm10`, the same `Host` entry as above with that port and its own `known_hosts.winvm10`):

| Command | What it does |
|---|---|
| `start` / `stop` | boot with quickemu (a window on this desktop) and wait for SSH; shut down and wait for QEMU to exit |
| `snapshot … create\|apply\|delete\|list NAME` | the disk snapshots; the VM must be off |
| `update` | Windows Update + Defender + Edge/WebView2, pass after pass (`winvm/update.ps1`), rebooting between passes, until nothing is left; then `versions` |
| `versions` | what a clean-machine record notes: Windows build, Edge, WebView2, Defender, Smart App Control, locale |
| `clean-up` | removes `C:\winvm` and our scheduled tasks, before a `clean` snapshot |
| `dev-tools` | PHP 8.4 (the SFX's extensions), Git for Windows and Python, each pinned by SHA-256 on the host and again in the guest (`winvm/dev-tools.ps1`) |
| `push SRC DEST` | copies a host directory into the guest (a tarball unpacked by the guest's own `tar`) |
| `run PROGRAM [ARG…]` | starts a program **in the desktop session** with `APPDATA`, `LOCALAPPDATA` and `TEMP` in a fresh `C:\winvm\run` (`WINVM_ENV='K=V …'` adds variables); output in `C:\winvm\run\std{out,err}.txt` |
| `stop-run` | kills that program and everything it started |
| `watch-start` / `watch-stop` | records every console window (conhost's, Windows Terminal's) shown on the desktop until stopped (`winvm/watch.ps1`, a WinEvent hook); `watch-stop` prints them, nothing if none |
| `ps SCRIPT` | runs one PowerShell script in the guest, as `-EncodedCommand` |
| `desktop SCRIPT [ARG…]` | runs a PowerShell script **in the desktop session** (through the headless console host) and prints its output; the helpers in `winvm/desk/`: `screen` (the whole desktop as a PNG), `dialog` (drives a native file dialog through UI Automation), `clipboard` (HTML + text, as another program leaves it), `clipboard-read` (its text, as JSON), `theme` (Windows' light/dark mode), `explorer` (File Explorer at a file, and where that file is on screen), `windows` (a class's windows, optionally closed), `cursor` (samples the pointer's shape), `close` (clicks a window's X). Arguments are positional |
| `get GUEST HOST` / `put HOST GUEST` | copies a file out of the guest, or into it |
| `tunnel [APP_ID]` | forwards the shell's automation channel to this host and prints a directory: `XDG_RUNTIME_DIR=<it> SHELL_AUTOMATION_REMOTE=1 scripts/dev/automation.py …` then drives the guest's window |

The snapshots: **`clean`** is updated Windows and nothing else (every installer test starts from
it); **`dev`** is `clean` plus `dev-tools` and the source trees in `C:\src`. Re-take `clean` with
`update`, `clean-up`, `stop`, `snapshot … delete clean`, `snapshot … create clean`.

The Windows 10 VM is built exactly as the Windows 11 one, from a Windows 10 22H2 ISO (ours is the
maintainer's Greek one, `CCCOMA_X64FRE_EL-GR`): copy quickget's answer file
(`windows-11/unattended/autounattend.xml.orig`, plus the two `.msi` files) into
`windows-10/unattended/`, hard-link `virtio-win.iso`, write `windows-10.conf` like the Windows 11 one,
and run `winvm-unattend.py` on it.

Gotchas, each one hit:

- ⚠️ **Windows Update refuses an SSH logon** (`E_ACCESSDENIED`): `update` runs each pass as SYSTEM
  through a scheduled task.
- ⚠️ **sshd keeps answering while Windows shuts down**, so "SSH is up" right after `Restart-Computer`
  can be the old boot: a pass started then is killed by the shutdown (`0x8007045B`). `reboot` waits
  for a new `LastBootUpTime` instead.
- ⚠️ **Windows 10's OpenSSH (7.7, the optional feature) strips the double quotes** out of the command
  it hands PowerShell; Windows 11's does not. Scripts therefore travel as `-EncodedCommand`
  (UTF-16LE, base64), never as text.
- ⚠️ **On the Windows 10 guest the key file was missing after the first logon** although Setup had
  queued the command and the next one ran; run again, the same command worked. If `ssh` says
  `Permission denied (publickey,…)`, log in with the account's password
  (`SSH_ASKPASS=<script that echoes it> SSH_ASKPASS_REQUIRE=force setsid ssh -o
  PreferredAuthentications=password -o PubkeyAuthentication=no winvm10 …`) and run that command.
- **Greek Setup** (and probably any non-English one) ignored Enter sent through the QEMU monitor on
  its language page; the button's access key (`sendkey alt-e`, "Επόμενο") worked. Its progress page
  then sat at "0%" while the disk filled: check `qemu-img info -U` before deciding it is stuck.
- **`python3` is the Microsoft Store's app-execution alias** (`…\WindowsApps\python3.exe`), which
  opens the Store; the installed interpreter is `python`. A manifest's `dev.worker` must say
  `python` in the guest.
- **Two WebView2 apps on one user data folder share one browser process**: kill the first one's
  process tree and the second one's webview dies with it. `run` therefore refuses while its previous
  program is alive.
- A program started through the interactive task can take 10 s and more to appear the first time
  (a cold PowerShell, and Defender scanning a new executable); `run` waits up to 60 s.
- **A process can only take the foreground from the one that has it** (the foreground lock): the
  automation channel's `key` shares the foreground window's input queue for the switch
  (`AttachThreadInput`) and, if Windows still refuses, sends nothing, rather than typing into whatever
  program is in front.
- **Real input is SendInput in the desktop session**, which is where the shell runs: the automation
  channel's `type` (one Unicode key press per character), `key`, `mouse` (a real left or right
  click, modifiers held) and `drag` (from a screen point, such as a file in Explorer, to the page).
  ⚠️ **Unicode injection does not wake the spell checker**: text typed as `VK_PACKET`s stays
  unchecked until a real key is pressed after it, so a spelling check types, then presses `space`.
  TinyMCE keeps a plain right click for its own menu; WebView2's (the spelling suggestions) is
  **Ctrl+right-click**.
- ⚠️ **Virtual keys go through the guest's keyboard layout**: `key t` types `τ` on the Greek guest.
  Text that must come out exactly is sent with `type`.
- ⚠️ **WebView2 spell-checks the user's primary language only** (the profile's
  `spellcheck.dictionaries` is `["el"]` on the Greek guest, with US English checking installed and in
  the Windows language list). ⚠️ **After a cold boot the first text typed into the editor can stay
  unmarked indefinitely** (twice in two cold boots on Windows 10, once in two on Windows 11; over 80 s),
  while text typed into an `<input>` first, or **any further typing**, gets everything checked within
  15–25 s, earlier words included. Once it has started it stays started for the boot, across app
  restarts and fresh profiles, which is why it never shows on a warm guest. The parity rows type
  misspellings in the guest's language and keep typing until they are marked.
- **File Explorer's large-icon view** (Windows 10's default here) starts a rubber-band selection when
  a drag begins on a tile's blank thumbnail; `explorer.ps1` reports a point on the file's name.
- ⚠️ **Windows hands a dead process's pid to a new process quickly** (seen on Windows 10): `run`
  and `stop-run` take the recorded pid as ours only if that process started before the pid file
  was written. A bare pid check refused a restart, and `stop-run` would have killed the stranger's tree.
- **A Greek guest's console output is code page 737/1253**: `winvm.sh` sets PowerShell's output to
  UTF-8 for every script, and logs are read with `-Encoding UTF8`.
- **HTTPS from the host**: `parity-windows.sh` makes a throwaway CA and a certificate for
  `10.0.2.2`, puts the CA in the guest's **machine** root store for the one row (the SSH session is
  elevated; the user store would show a confirmation dialog) and removes it afterwards.
- **The guest reaches the host at `10.0.2.2`** (QEMU's user-mode network), which is how a test
  server on the host (`scripts/dev/macguest/test-server.py`, the local AI model) is reached.
- **Nothing in the SSH session can see the desktop's windows** (`MainWindowHandle` is 0 there, and
  `PostMessage` reaches nothing); anything that must touch a window runs in the desktop session.
- **`Set-Content -NoNewline <path> <value>` with positional arguments writes nothing** (and says so
  on stderr only); name them: `-Path … -Value …`.
- `pgrep -f`/`pkill -f` with a pattern that also appears in the calling command line match the
  caller; `stop-run` kills by the pid the guest recorded.

## Facts about this guest worth knowing

- Windows 11 Pro build 26300 came with WebView2 Evergreen 157 preinstalled. Read the version with
  `(Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}').pv`.
- **The `clean` snapshots of 2026-11-10** (`versions`, after `update`): `windows-11` is Windows 11
  Pro 26H2, build 26300.9711, en-GB, Smart App Control in evaluation mode, Defender signatures
  1.463.226.0; `windows-10` is Windows 10 Pro 22H2, build 19045.6456 (installed at 19045.2965),
  el-GR, no Smart App Control, signatures 1.463.227.0. Both have WebView2 and Edge 157.0.4341.42:
  the Windows 10 media from 2023 got the runtime during setup, so a missing runtime cannot be met
  naturally on either guest. The
  registry's `ProductName` says "Windows 10" on Windows 11 too; read `Win32_OperatingSystem.Caption`.
- `msedgewebview2.exe` processes belonging to `SearchHost.exe` (Windows Search) are always running.
  Check a process's `--webview-exe-name=` argument before blaming the shell for a stray one.
