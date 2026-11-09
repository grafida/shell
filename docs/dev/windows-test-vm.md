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

## Facts about this guest worth knowing

- Windows 11 Pro build 26300 came with WebView2 Evergreen 157 preinstalled. Read the version with
  `(Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}').pv`.
- `msedgewebview2.exe` processes belonging to `SearchHost.exe` (Windows Search) are always running.
  Check a process's `--webview-exe-name=` argument before blaming the shell for a stray one.
