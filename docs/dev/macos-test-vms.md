# Clean macOS test VMs (Apple Silicon host)

Two local VMs for testing a **packaged** app (a downloaded, notarised DMG) on Macs that have never
seen a build tool: the clean-machine half of grafida/grafida#88. Each is restored by
[tart](https://tart.run) from one of Apple's own IPSW restore images (not a preconfigured image),
taken through Setup Assistant by hand, updated, and kept as a `<name>-clean` VM that nothing of ours
has touched. Every run works on a fresh clone of it. `scripts/dev/macvm.sh` does everything except
Setup Assistant.

| VM | Restored from | Updated to | Why |
|---|---|---|---|
| `macos-12` | 12.6 (21G115), the newest macOS 12 VM restore image | 12.7.6 (21H1320) + Safari 17.6 | the floor (ADR 0001) |
| `macos-27` | 27.2 (26B123) | 27.2.1 (26B131), Safari 27.2 | the current release |

Guests can only be arm64 macOS (Virtualization.framework runs no Intel guest and no macOS older than
12), and Apple's licence allows **two macOS guests at a time**. An x86_64 app is therefore tested
under Rosetta 2, in a Rosetta clone of each clean guest (section 3; grafida/grafida#92).

Every VM has the user `tester`, password `grafida-vm`, passwordless sudo, Remote Login (SSH) on,
automatic login, and no sleep, screen saver, screen lock or automatic updates, so there is always an
unlocked desktop session to run the app in. The SSH key is `~/.ssh/grafida-macvm_ed25519`, created by the first
`prepare`; `~/.ssh/config` is not touched. What sets these guests apart from a user's Mac, all of it
needed to drive them: SSH, passwordless sudo, no screen lock. Two leftovers of the 2026-11-09 build are
recorded in grafida/grafida#88: `macos-12-clean` carries one Screen Recording grant (made for
`screencapture` over SSH, not for any app under test), and a 1.9 GB downloaded, unprepared macOS 27
update asset that SIP keeps from being deleted (see the `--recommended` gotcha).

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
scripts/dev/macvm.sh prepare macos-12    # SSH key, passwordless sudo; no sleep, screen saver, lock, auto-updates
scripts/dev/macvm.sh update macos-12     # 12.6 → 12.7.6 through the full installer; the guest restarts
scripts/dev/macvm.sh wait macos-12
scripts/dev/macvm.sh update macos-12     # again: the non-macOS updates by label (Safari 17.6)
scripts/dev/macvm.sh prepare macos-12    # again, harmlessly: an OS update can bring the lock back
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
- **Never `softwareupdate --recommended` on an old major version.** On 12.7.6 it includes the
  upgrade to the current major release, and downloads *and prepares* it: the sealed system volume
  is mounted at `/System/Volumes/Update/mnt1` with the new OS applied, waiting for a boot blessing.
  It was not installed only because an OS install on Apple Silicon needs the owner's credentials.
  A restart without them discards the prepared system and its snapshots; the downloaded asset stays
  (SIP-restricted, purged by macOS in its own time). `update` therefore installs the non-macOS
  updates by label, and an OS update only for the guest's own major version.
- **An OS update on Apple Silicon needs the volume owner's credentials** whether it comes through
  `startosinstall` or `softwareupdate --install` (`--user … --stdinpass`). Without them
  `softwareupdate` downloads it, says nothing more, and the guest stays where it was.
- **Prefer an ordinary OS update to a full installer.** 27.2.1 was offered both ways; the ~17 GB full
  installer left the guest 8 GB short of the space it needs.
- **A locked screen hides every window, and WebKit then clamps the page's timers to 1 s.** macOS 27
  locked its screen after an update's restart; a timing check then measured the lock (9 ticks in 3 s).
  `prepare` runs `sysadminctl -screenLock off` (macOS 12 has it too).
- **`screencapture` over SSH captures only the wallpaper and the menu bar** until the guest grants
  Screen Recording, which raises a prompt in the guest. Take screenshots of the app with the
  automation channel (`takeSnapshot`), or of the VM window from the host. An `osascript` that talks
  to System Events over SSH likewise raises an Automation prompt; click Don't Allow.
- **A Safari update replaces the system WebKit** that every WKWebView uses, so the Safari version
  is part of what a run is measured on. Record it (`/Applications/Safari.app`'s
  `CFBundleShortVersionString`).
- **A clean guest has no Python.** `/usr/bin/python3` is a stub that asks to install the Command Line
  Tools (and raises that dialog in the guest). `perl`, `plutil` and `nc` are there. The parity script
  needs Python (its client and the fake worker), so a packaged run installs the Command Line Tools in
  the clone **after** the DMG install and first launch have been checked on the untouched guest
  (decided in grafida/grafida#88); everything from then on ran on a guest with build tools.
- A GUI program started over SSH is outside the desktop session and cannot reach the WindowServer.
  Start it inside the logged-in user's session: `sudo launchctl asuser $(id -u) sudo -u tester CMD`.
- macOS 12 runs WebKit without a separate GPU process (WebContent + Networking only); newer
  releases add `com.apple.WebKit.GPU`. WebKit's helpers are XPC services started by launchd, not
  children of the app, which matters when measuring the CPU of "the app".

## 2. Testing

Before every run: `scripts/dev/macvm.sh fresh macos-12` (a new clone of `macos-12-clean`), then
`run` and `wait`. Copy files in with `scp -i ~/.ssh/grafida-macvm_ed25519 … tester@$(tart ip macos-12):`.

### A packaged app on a clean guest (grafida/grafida#88)

The order matters, because some steps change the guest:

1. **Serve the image from the host**: `scripts/dev/macguest/test-server.py 8765 <the .dmg>`. The
   guests reach the host at their gateway, `192.168.64.1` (the host's firewall must not prompt).
2. **Install as a user would**, by hand in the VM window: Safari to
   `http://192.168.64.1:8765/<the .dmg>` (Safari 17.6 on 12 asks to allow downloads from the
   address; 27.2 did not), open the image, drag the app to Applications, double-click it.
   **Note every dialog's wording before clicking**: the Gatekeeper dialog appears once per
   quarantined copy, and a new download brings it back. Then, over SSH: the image's
   `com.apple.quarantine` (`0183;…;Safari`) and the app's (`01c3`: the 0x40 bit is the user's
   approval), `spctl -a -vv -t open --context context:primary-signature` on the image, `spctl -a
   -vv` and `codesign --verify --deep --strict` on the app, its entitlements and Info.plist.
3. **The floor**: a copy with `LSMinimumSystemVersion` 99 (`plutil -replace`, re-signed ad hoc) must
   be refused: `open` fails with -10825 (`kLSIncompatibleSystemVersionErr`), and from Finder it
   says "You can't use this version of the application …".
4. **Only now the Command Line Tools** (Python for the scripts, `stapler`):
   `touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress`, `softwareupdate -l`,
   `sudo softwareupdate -i "<the Command Line Tools label>" --agree-to-license`. From here on the
   guest has build tools; everything above ran on an untouched one. (Calling `xcrun` before this
   raises the install dialog in the guest.)
5. **The automated checks**, in the desktop session (`sudo launchctl asuser $(id -u) sudo -u tester
   …`) with `TMPDIR=$(getconf DARWIN_USER_TEMP_DIR)`: `scripts/dev/parity-macos.sh` with
   `GRAFIDA_PACKAGED=/Applications/<App>.app SHELL_BIN=<the release workflow's
   debug-automation-macos-arm64 artifact; x86_64: section 3>`, then `scripts/dev/macguest/ats-matrix.sh`. Copy in the
   shell repo's `scripts/dev` and `tests/apps`, `tests/workers` (`git archive`). Keep off the VM:
   the key rows activate the app.
6. **By hand**, then **`scripts/dev/macguest/idle-cpu.sh`** on the release app, with the user's
   hands off and the screen the checklist names (a focused editor's caret alone costs ~1.5 %; a
   real click on the sidebar takes the focus out of it).

Gotchas found on the way:

- **The guest's screen can freeze** while the system runs on: the pointer stops moving in the VM
  window, `WindowServer` sits in state `U` (an uninterruptible wait in the kernel, likely the
  virtual graphics device), and `screencapture` hangs. SSH still answers. Only a restart helps
  (`sudo shutdown -r now`); nothing on disk is lost. Seen once on macOS 12, with nothing of ours
  running.
- **Scripts run in the guests under bash 3.2**: no associative arrays, `${a[@]+"${a[@]}"}` for a
  possibly empty array under `set -u`, and no `case` inside `$( … )` (move it into a function).
- **A `.local` name may resolve to IPv6 only** over the virtual network (macOS 27 publishes the
  guest's and the host's that way), and a test server listening on IPv4 alone then never answers
  it; `test-server.py` listens on both. Grafida's PHP resolver sees no address for such a name
  (`gethostbynamel()` is IPv4, `dns_get_record()` never asks Bonjour), and the webview's own
  fetch to it hung on 27 where `curl` succeeded: the `.local` row of the ATS matrix is not
  testable in this setup.

## 3. x86_64 under Rosetta 2 (grafida/grafida#92)

Intel Macs are tested nowhere else: there is no Intel hardware and no Intel guest. Each clean guest
gets one Rosetta 2 clone, made once and kept like the others:

```sh
scripts/dev/macvm.sh rosetta macos-12 --agree-to-license   # macos-12-clean + Rosetta 2 → macos-12-rosetta-clean
scripts/dev/macvm.sh rosetta macos-27 --agree-to-license
```

`rosetta` clones `NAME-clean`, boots the clone headless, checks that it cannot run x86_64 code yet,
runs `softwareupdate --install-rosetta --agree-to-license`, proves `arch -x86_64` works, shuts it down
and seals it as `NAME-rosetta-clean`. Nothing else is added, so everything section 1 says about the
clean guest holds for its clone. ⚠️ **Installing Rosetta 2 accepts Apple's licence for it**: the flag
is required, and the operator is asked before it is passed. The base `-clean` guest never gets Rosetta,
so the arm64 runs stay on a Mac that cannot run Intel code by accident.

Made on 2026-11-11: `macos-12-rosetta-clean` (12.7.6 21H1320, Rosetta 2 package
`1.0.0.0.1.1721215246`) and `macos-27-rosetta-clean` (27.2.1 26B131, `1.0.0.0.1.1793943009`). On 12,
`softwareupdate` prints "Package Authoring Error: … RosettaUpdateAuto is missing installKBytes
attribute" before installing it: Apple's catalogue, harmless.

From then on `macos-12-rosetta` is a guest like the others (`fresh`, `run`, `wait`, `ssh`, `stop`); it
counts towards the two-guest limit. A run follows section 2 with the x86_64 artefacts: the
`-macos-amd64` DMG, installed and first launched through Gatekeeper on the untouched clone, then
`parity-macos.sh` with `SHELL_BIN=<the debug-automation-macos-x86_64 artifact>`. The parity script
reads the architecture from `SHELL_BIN`, refuses an x86_64 one on a Mac without Rosetta 2, and its
first row proves what runs: `_native/info`'s `arch` (fixed at compile time, so `x86_64` from a shell
on an arm64 Mac is Rosetta 2) and every executable in the packaged bundle (shell and worker stub),
which must all be x86_64. Python, the parity client and the fake worker stay arm64; only the app is
translated.
