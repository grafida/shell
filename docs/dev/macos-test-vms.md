# Clean macOS test VMs (Apple Silicon host)

Two local VMs for testing a **packaged** app (a downloaded, notarised DMG) on Macs that have never
seen a build tool: the clean-machine half of grafida/grafida#88. Each is restored by
[tart](https://tart.run) from one of Apple's own IPSW restore images (not a preconfigured image),
taken through Setup Assistant by hand, updated, and kept as a `<name>-clean` VM that nothing of ours
has touched. Every run works on a fresh clone of it. `scripts/dev/macvm.sh` does everything except
Setup Assistant.

| VM | Restored from | Updated to | Why |
|---|---|---|---|
| `macos-12` | 12.6 (21G115), the newest macOS 12 VM restore image | the newest 12.x full installer (12.7.6), then Safari | the floor (ADR 0001) |
| `macos-27` | 27.2 (26B123) | its pending updates | the current release |

Guests can only be arm64 macOS (Virtualization.framework runs no Intel guest and no macOS older than
12), and Apple's licence allows **two macOS guests at a time**.

Every VM has the user `tester`, password `grafida-vm`, passwordless sudo, Remote Login (SSH) on,
automatic login, and no sleep, screen saver or automatic updates, so there is always a desktop
session to run the app in. The SSH key is `~/.ssh/grafida-macvm_ed25519`, created by the first
`prepare`; `~/.ssh/config` is not touched. SSH being on is the one way these guests differ from a
user's Mac; it is needed to drive them.

## 0. Setup

```sh
cp scripts/dev/macvm.conf.sample scripts/dev/macvm.conf   # git-ignored: TART_HOME, IPSW_DIR
```

tart is installed from its GitHub release (the `cirruslabs/cli` Homebrew tap's formula stopped
loading in current Homebrew): check `tart.tar.gz` against the release's checksums file and
`spctl -a -vv tart.app` ("Notarized Developer ID", Cirrus Labs), put `tart.app` in `/Applications`
and link `tart.app/Contents/MacOS/tart` onto the PATH. Put `TART_HOME` on a volume with room: each
IPSW is 14–26 GB and each VM up to 80 GB, twice over (the VM and its `-clean` twin; on APFS, clones
share blocks until they diverge).

## 1. Restore, Setup Assistant, prepare, update, seal

```sh
scripts/dev/macvm.sh create macos-12     # fetch + check the pinned IPSW, then restore (~10 min)
scripts/dev/macvm.sh run macos-12        # a window
```

Setup Assistant, **by hand**, in the VM window:

1. Any country and keyboard. Migration Assistant: **Not Now**. Apple ID: **Set Up Later → Skip**.
2. Account: full name and account name `tester`, password **`grafida-vm`**.
3. Location Services, analytics, Screen Time, Siri: off or later. **FileVault: off** (it blocks
   automatic login).
4. At the desktop, System Preferences (System Settings on 13+): **Sharing → Remote Login** on, and
   **Users & Groups → Login Options → Automatic login: tester**.
5. Install no updates there: `update` does it, and keeps to the guest's major version.

Then:

```sh
scripts/dev/macvm.sh prepare macos-12    # SSH key, passwordless sudo, no sleep/screen saver/auto-updates
scripts/dev/macvm.sh update macos-12     # 12.6 → 12.7.6 through the full installer; the guest restarts
scripts/dev/macvm.sh wait macos-12
scripts/dev/macvm.sh update macos-12     # again: the recommended updates (Safari 17.6)
scripts/dev/macvm.sh stop macos-12
scripts/dev/macvm.sh seal macos-12       # → macos-12-clean
```

Gotchas:

- **macOS refuses a password that contains the account name**, so it is not `tester`.
- **The host's own `~/.ssh/config` may allow only public keys** (`PreferredAuthentications
  publickey` under `Host *` does). `prepare` therefore asks for password authentication explicitly,
  and takes the password from an `SSH_ASKPASS` helper (`SSH_ASKPASS_REQUIRE=force`), since there is
  no terminal to type it in. `expect` did not work here.
- **Software Update offers a macOS 12 VM restored from 12.6 no 12.7.x at all**, only Safari. The
  12.7.x releases are there as full installers (`softwareupdate --list-full-installers`), which is
  what `update` uses: `--fetch-full-installer`, then `startosinstall --stdinpass` (on Apple Silicon
  an OS install needs the volume owner's password).
- **A Safari update replaces the system WebKit** that every WKWebView uses, so the Safari version
  is part of what a run is measured on. Record it (`/Applications/Safari.app`'s
  `CFBundleShortVersionString`).
- A GUI program started over SSH is outside the desktop session and cannot reach the WindowServer.
  Start it inside the logged-in user's session: `sudo launchctl asuser $(id -u) sudo -u tester CMD`.
- macOS 12 runs WebKit without a separate GPU process (WebContent + Networking only); newer
  releases add `com.apple.WebKit.GPU`. WebKit's helpers are XPC services started by launchd, not
  children of the app, which matters when measuring the CPU of "the app".

## 2. Testing

Before every run: `scripts/dev/macvm.sh fresh macos-12` (a new clone of `macos-12-clean`), then
`run` and `wait`. Copy files in with `scp -i ~/.ssh/grafida-macvm_ed25519 … tester@$(tart ip macos-12):`.
