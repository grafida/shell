# Packaging

[grafida/shell-packager](https://github.com/grafida/shell-packager) turns your app into something users can install:
a tarball for Linux, a disk image for macOS, an installer for Windows. Which of them you can build depends on the
machine you build on: see [Which machine builds what](#which-machine-builds-what).

## Which machine builds what

**To build for macOS you need a Mac.** The bundle is sealed with `codesign` and the disk image is made with `hdiutil`,
and both exist only on macOS. Even an unsigned disk image (`--no-sign`) is made with `hdiutil`. Nothing replaces them
on another operating system.

| Build machine | Linux tarball | Windows installer | macOS disk image |
|---|---|---|---|
| **A Mac** | ✅ | ✅ | ✅ |
| **Linux** | ✅ | ✅ | ❌ |
| **Windows** | ✅ | ✅ | ❌ |

So with a Mac you can produce every deliverable, for both CPU architectures of each platform. With Linux or Windows you
can produce the Linux and Windows ones, and for macOS you need a Mac, or a macOS runner in your CI.

* The Linux tarball is written by the packager itself, in PHP, so it needs no extra tool on any host.
* The Windows installer needs `makensis`, and signing it needs `jsign`; checking the signature needs `osslsigncode`. All
  three are available for macOS and Linux (Homebrew, apt). See [Install the packager](#install-the-packager).
* **A cross-built package is not run before it is written.** The packager's
  [worker test](#the-packaged-worker-is-tested-before-it-is-written) only runs on a machine of the target's operating
  system and CPU. Test packages built for another platform on the platform itself before you publish them.

## What goes into a package

Every package holds the same three parts, built by the packager from your sources:

```
shell executable         the prebuilt grafida/shell binary (the window and the webview)
worker stub              phpmicro's "SFX": a self-contained PHP 8.4 interpreter, named after worker.binary
worker payload (.phar)   your app: a PHAR holding your PHP code, vendor/ and assets, plus php.ini settings
shell.json               your manifest
```

The stub and the payload are **two files, side by side**, never one. A stub with its payload appended cannot be
code-signed, because the signature would land past the PHAR and corrupt it.

At run time the shell starts the stub; the stub finds `<name>.phar` next to it and runs your entry point from inside the
PHAR, with no PHP installation on the machine. Platform specifics of the layout are in
[the packager's README](https://github.com/grafida/shell-packager#readme).

## Install the packager

It is a development tool. Like `shell-php`, it is not on Packagist:

```json
{
    "repositories": [
        {"type": "vcs", "url": "https://github.com/grafida/shell-php"},
        {"type": "vcs", "url": "https://github.com/grafida/shell-packager"}
    ],
    "require": {
        "php": "^8.4",
        "grafida/shell-php": "^0.2"
    },
    "require-dev": {
        "grafida/shell-packager": "^0.4"
    }
}
```

You need, on the build machine:

* PHP 8.4 with `curl`, `json`, `mbstring`, `phar` and `zlib`, run with `phar.readonly` off: invoke the packager as
  `php -d phar.readonly=0 vendor/bin/shell-packager …`.
* `composer` on `PATH`, and a committed **`composer.lock`**. The packager installs your runtime dependencies from the
  lock with `--no-dev --no-scripts` into a staging directory; your own `vendor/` is never used, so development
  dependencies cannot leak into the package. Composer `scripts` do not run during that install.
* For macOS packages (on a Mac only): Xcode's command line tools (`codesign`, `hdiutil`, and for notarisation `notarytool`).
* For Windows packages: `makensis` 3.08 or later (the installer), and for signing `jsign` and `osslsigncode`.

## The configuration file: `shell-package.json`

A JSON file in your project root (conventionally `shell-package.json`). Paths in it are relative to the file. The
names of things in the package come from your manifest, so the shell and the package cannot disagree: `id` names the
desktop entry and icons, `title` is used in alerts, `worker.binary` names the stub and its `.phar`.

```jsonc
{
  "manifest": "shell.json",
  "phar": {
    "entrypoint": "bin/worker.php",
    "include": [
      {"directory": "bin", "name": "worker.php"},
      {"directory": "src", "name": "*.php"},
      {"directory": "public"},
      {"directory": "vendor", "name": "*.php", "notName": ["*Test.php"]},
      {"directory": "vendor/grafida/shell-php/resources"}
    ],
    "ini": {"memory_limit": "512M", "display_errors": "stderr", "opcache.enable": false}
  },
  "runtimes": { /* pinned downloads: see below */ },
  "linux":   { /* … */ },
  "macos":   { /* … */ },
  "windows": { /* … */ }
}
```

### `phar`: the worker payload

| Member | Meaning |
|---|---|
| `entrypoint` | The PHP file the stub runs, relative to the project root. It must be among the included files. |
| `include` | A list of rules. Each names a `directory` (relative to the project, or `vendor`) and includes **every file under it**, narrowed by `name` (glob patterns on the file name; any may match), `notName` (globs that exclude by file name) and `notPath` (path prefixes, relative to the directory, that exclude a whole subtree). A rule can also name a single file. |
| `ini` | PHP settings applied before any PHP code runs, as phpmicro's extra-INI block at the front of the payload. Values may be strings, numbers or booleans. Use it for what `ini_set()` cannot change. |

Things to get right:

* **Include what you read at run time.** Anything not matched by an `include` rule does not exist in the package:
  templates, language files, SQL migrations, JSON data, documentation shown in the app, the `public/` directory your
  `StaticFiles` serves. Missing files fail only when that code path runs, so test the packaged build.
* Files under `vendor/` come from the staged `--no-dev` install. Use `name` and `notName` to leave out tests and
  other noise. A `*.php` filter drops every non-PHP file inside packages, so add a rule (without `name`) for any
  vendor directory whose data files you read. **You need one for `shell.js`**: `ShellScript::response()` reads
  `vendor/grafida/shell-php/resources/shell.js`, so without the
  `{"directory": "vendor/grafida/shell-php/resources"}` rule above the packaged app answers `/shell.js` with a 500 and
  every native call fails. (The alternative is to copy `shell.js` into your own assets at build time.)
* Path repositories in your `composer.json` are installed as copies, with the path made absolute.
* The PHAR is uncompressed and deterministic: with `SOURCE_DATE_EPOCH` set, the same input gives the same bytes.
* The packager checks that `phar.ini` really took effect (phpmicro silently ignores a malformed block) before it writes a package.

### `runtimes`: pinned downloads

The packager downloads the **shell executable**, the **Linux launcher** and the **phpmicro stub** for each platform.
You pin each by URL and SHA-256, so a build cannot silently change under you:

```jsonc
"runtimes": {
  "linux": {
    "shell":    {"url": "https://github.com/grafida/shell/releases/download/0.4.1/grafida-shell-linux-{arch}",
                 "sha256": {"x86_64": "…", "aarch64": "…"}},
    "launcher": {"url": "https://github.com/grafida/shell/releases/download/0.4.1/grafida-launcher-linux-{arch}",
                 "sha256": {"x86_64": "…", "aarch64": "…"}},
    "sfx":      {"url": "https://github.com/grafida/phpmicro/releases/download/sfx-20261109.16/linux-{arch}.standard.sfx",
                 "sha256": {"x86_64": "…", "aarch64": "…"}}
  },
  "macos": {
    "shell": {"url": {"aarch64": "…/grafida-shell-macos-arm64", "x86_64": "…/grafida-shell-macos-x86_64"},
              "sha256": {"aarch64": "…", "x86_64": "…"}},
    "sfx":   {"url": "https://github.com/grafida/phpmicro/releases/download/sfx-…/macos-{arch}.standard.sfx",
              "sha256": {"aarch64": "…", "x86_64": "…"}}
  },
  "windows": {
    "shell": {"url": "https://github.com/grafida/shell/releases/download/0.4.1/grafida-shell-windows-x86_64.exe",
              "sha256": {"x86_64": "…"}},
    "sfx":   {"url": "https://github.com/grafida/phpmicro/releases/download/sfx-…/windows-x86_64.standard.sfx",
              "sha256": {"x86_64": "…"}}
  }
}
```

* `{arch}` in a URL is `x86_64` or `aarch64`. A `url` can instead be an object mapping each architecture to its own URL,
  for when they come from different releases.
* Shell and launcher assets come from [grafida/shell releases](https://github.com/grafida/shell/releases); the
  `sfx` stubs from [grafida/phpmicro releases](https://github.com/grafida/phpmicro/releases) (tags `sfx-<date>.<run>`,
  which are never rewritten). For releases, pin an immutable tag; `sfx-latest` is what [`latest`](#using-the-latest-release-instead-of-a-pin) uses.
* **Where the hashes come from:** download the asset yourself, compute its SHA-256 (`shasum -a 256 FILE`), and record it. Each
  release asset also has a `.sha256` file beside it; compare the two once, over HTTPS, when you first pin. **The
  packager never reads those `.sha256` files**: a mismatch with the hash in your configuration, or a missing pin
  for an architecture, is fatal (unless the runtime is [`latest`](#using-the-latest-release-instead-of-a-pin)). Downloads are cached by hash in `SHELL_PACKAGER_CACHE` (default
  `$XDG_CACHE_HOME/grafida-shell-packager`).
* `shell-packager fetch --config=shell-package.json --arch=x86_64 [--platform=linux|macos|windows]` downloads and verifies
  the runtimes without building.
* `--local NAME=PATH` replaces one runtime (`shell`, `launcher`, `sfx`) with a local file, to test an unreleased build.
  It is announced as UNPINNED every time; never use it for a release. It works per runtime and mixes with pinned and
  `latest` runtimes.
* Upgrading the shell is: change the URL and hashes, rebuild.

#### Finding the URLs and hashes to pin

**1. Choose the releases.** Open the release pages and note the tag you want from each:

* [grafida/shell releases](https://github.com/grafida/shell/releases): tags like `0.4.1`. One release holds the shell
  and the Linux launcher for every platform.
* [grafida/phpmicro releases](https://github.com/grafida/phpmicro/releases): tags like `sfx-20261111.28`. Pick the newest
  `sfx-<date>.<run>` tag (not `sfx-latest`, which moves). Each tag holds the stubs for every platform and, for Windows,
  `vc-runtime-x86_64.zip`, which you do not pin.

**2. Build the URLs.** A URL is `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`. The asset names
are:

| Runtime | Asset |
|---|---|
| `shell`, Linux | `grafida-shell-linux-x86_64`, `grafida-shell-linux-aarch64` |
| `shell`, macOS | `grafida-shell-macos-x86_64`, `grafida-shell-macos-arm64` |
| `shell`, Windows | `grafida-shell-windows-x86_64.exe` |
| `launcher` (Linux only) | `grafida-launcher-linux-x86_64`, `grafida-launcher-linux-aarch64` |
| `sfx` | `<platform>-<arch>.standard.sfx`, e.g. `linux-x86_64.standard.sfx`, `macos-aarch64.standard.sfx`, `windows-x86_64.standard.sfx` |

Mind the naming: the macOS **shell** asset says `arm64`, but the macOS **sfx** and the `sha256`/`url` keys in
`shell-package.json` say `aarch64`. That is why the macOS shell in the example above uses an object for `url`, with one
entry per architecture.

**3. Get the hashes.** Compute them yourself from the downloaded file; do not copy them from anywhere unchecked:

```sh
shasum -a 256 grafida-shell-linux-x86_64          # macOS, Linux
sha256sum grafida-shell-linux-x86_64              # Linux
Get-FileHash -Algorithm SHA256 grafida-shell-windows-x86_64.exe   # Windows PowerShell
```

You can also read GitHub's own record of the hash, without downloading, with the [GitHub CLI](https://cli.github.com):

```sh
gh release view 0.4.1 -R grafida/shell --json assets \
  -q '.assets[] | "\(.digest | sub("^sha256:"; ""))  \(.name)"'
```

The same command with `-R grafida/phpmicro` and an `sfx-…` tag lists the stubs. Each asset also has a `.sha256` file
in the same format as `shasum` output (`<hash>  <name>`); comparing it to the hash you computed is a cheap
cross-check, but it is only a cross-check, since it comes from the same place as the file.

**4. Let the packager do it for you.** To avoid copying 64-character strings by hand, put the URLs in
`shell-package.json` with a placeholder hash of 64 zeros (the packager insists on 64 hex digits) and run
`shell-packager fetch`. It fails with `does not match its pin`, and the `got:` line is the actual SHA-256 of what was downloaded. Treat that as **a value to verify, not to trust**: it is the hash of
whatever the URL served. Compare it with the value from `gh release view` or the `.sha256` file, then paste it in.
(Alternatively, make the runtime [`latest`](#using-the-latest-release-instead-of-a-pin) once: the warning prints the
SHA-256 it downloaded, to compare in the same way.)

Pin every architecture you build. Hashes are per file, so each platform and architecture needs its own entry, and the
packager fails when the one it needs is missing. Re-do the steps whenever you upgrade; the
URL and its hash always change together.

#### Using the latest release instead of a pin

Instead of a URL and hashes, a runtime can be just `{"latest": true}`:

```jsonc
"runtimes": {
  "linux": {
    "shell":    {"latest": true},
    "launcher": {"latest": true},
    "sfx":      {"latest": true}
  }
}
```

Each runtime is either pinned or `latest`; giving both is an error. You can mix them: pin the shell and take the newest
`sfx`, for instance. The packager picks the right asset for the platform and architecture:

| Runtime | Resolves to |
|---|---|
| `shell`, `launcher` | The asset of the [newest grafida/shell release](https://github.com/grafida/shell/releases/latest) (`…/releases/latest/download/…`). `launcher` exists for Linux only. |
| `sfx` | The `sfx-latest` release of [grafida/phpmicro](https://github.com/grafida/phpmicro/releases/tag/sfx-latest), which is rebuilt on every push to its `sibling-phar` branch. |

Every use prints a `WARNING: … is LATEST, not pinned` line with the URL and the SHA-256 of what was downloaded, and the
file is cached under that hash.

> ⚠️ **"Latest" changes your build under you.** Nothing is checked against a hash you chose, so two builds of the same
> commit, made on different days, can ship different shells and different PHP runtimes: a new shell release, a new PHP
> patch version, a changed extension list, a different minimum operating system. A release can contain incompatible
> changes, and you will find out when your packaged app misbehaves, not when you build it. **Do not use `latest` for a
> release you ship to users.** It is for trying an upcoming shell or runtime during development, and for CI jobs that
> exist to catch incompatibilities early. For releases, pin: take the same asset from a specific release, check that its
> SHA-256 is the one in the warning, and the build stays the same until you change it.

`latest` is rejected for a runtime that does not exist on a platform (there is no macOS or Windows launcher). It needs
packager 0.5.0 (unreleased at the time of writing); earlier versions require a pin for everything.

**Linux only:** `launcher` is a small static program that is the app's entry point. It checks that the system has the
libraries the shell needs and either starts the shell or shows a helpful alert listing the packages to install.

### The PHP runtime inside the package

The phpmicro stub is **PHP 8.4** (the newest patch release at the time its tag was built), built for the shell (fully static on macOS and Windows; on Linux it links the
system's glibc, 2.35 or later) with this extension set:

`ctype`, `curl`, `dom`, `ffi`, `filter`, `iconv`, `libxml`, `mbstring`, `opcache`, `openssl`, `pdo`, `pdo_sqlite`,
`phar`, `shmop`, `sockets`, `sodium`, `sqlite3`, `xml`, `zlib`

(plus PHP's built-ins such as `json`, `pcre`, `hash`, `spl`, `random` and `standard`). The list has no MySQL or PostgreSQL
driver, and no `gd`, `intl`, `bcmath`, `zip` or `pcntl`. SQLite is the natural database. A dependency that needs
anything else will work in `--dev` on your machine and fail in the package. Check `composer.json`'s `ext-*` entries
against this list, and run the packaged build before every release. The extension set can change between stub
releases; see the [phpmicro workflow](https://github.com/grafida/phpmicro/blob/sibling-phar/.github/workflows/build-sfx.yml)
(`EXTENSIONS`).

## Building

All commands take `--config=shell-package.json` and `--app-version=1.2.3`. Always add `-d phar.readonly=0`.

| Command | Result |
|---|---|
| `shell-packager phar --output=build/worker.phar` | Just the worker payload. Useful to inspect what ships. |
| `shell-packager linux-tarball --arch=x86_64` (or `aarch64`) | A `.tar.gz` |
| `shell-packager macos-app [--arch=…]` | A signed `.app` bundle (default `aarch64`; `--arch=x86_64` for Intel) |
| `shell-packager macos-dmg [--arch=…] [--no-style]` | A disk image holding the bundle and an `/Applications` link |
| `shell-packager windows-tree` | An install tree (x86_64 only) |
| `shell-packager windows-installer` | A Windows installer `.exe` |
| `shell-packager windows-sign --file=FILE` | Signs one executable (used by installer scripts) |

`--output=` overrides the output path; `--no-check` skips the worker test below. Output paths in the configuration may
use `{name}`, `{version}`, `{arch}` (`x86_64`/`aarch64`) and `{debarch}` (`amd64`/`arm64`).

**Reproducibility.** With `SOURCE_DATE_EPOCH` set, the PHAR and the Linux tarball are byte-identical for identical inputs. The macOS bundle and image are not (`codesign` and `hdiutil` are not deterministic).

### The packaged worker is tested before it is written

On a machine of the target's operating system and CPU, the packager runs the packaged worker and checks that:

* the `ini` settings read back exactly; and
* it sends the protocol's `ready` frame and exits when its standard input closes.

That proves the PHAR, your autoloader and your bootstrap load from `phar://`. It cannot run when you cross-build
(for example the Windows package on a Mac); then it is skipped with a note. A cross-built package should be tested on
the target before you publish it.

### Linux

```jsonc
"linux": {
  "name": "myapp",
  "desktop": "assets/myapp.desktop",
  "icons": {"48": "assets/icons/48.png", "512": "assets/icons/512.png"},
  "files": {"LICENSE.txt": "LICENSE.txt"},
  "replaces": [],
  "output": "build/dist/MyApp-{version}-linux-{debarch}.tar.gz"
}
```

* `name` is the launcher's file name (default: your title in lower case). `desktop` is a freedesktop `.desktop` file you
  provide; the packager forces its `Exec`, `Icon` and `StartupWMClass` to agree with the package. `icons` maps pixel
  sizes to PNG files. `files` adds extra files to the tarball. `replaces` lists desktop ids of an earlier build under
  another id that `install.sh` should clean up.
* The tarball unpacks to a folder with the launcher (named `name`), the shell, the worker stub and payload,
  `shell.json`, the desktop entry, the icons, and `install.sh`. Users can run the launcher directly, or run
  `./install.sh` to add a desktop entry and icons for themselves (`./install.sh --uninstall` removes them). It needs no root.

### macOS

```jsonc
"macos": {
  "name": "MyApp",
  "icon": "assets/MyApp.icns",
  "infoPlist": {"NSHumanReadableCopyright": "…", "LSApplicationCategoryType": "public.app-category.productivity"},
  "entitlements": {"worker": "build/macos/worker.entitlements"},
  "resources": {"LICENSE.txt": "LICENSE.txt"},
  "dmg": {"background": "assets/dmg-background.tiff", "window": [640, 400], "iconSize": 128,
          "positions": {"app": [160, 210], "applications": [480, 210]}, "volumeIcon": "assets/MyApp.icns"}
}
```

The bundle: the shell is `Contents/MacOS/<name>` (the bundle's executable), the worker stub is beside it, and the
payload, `shell.json`, the icon and your extra resources are in `Contents/Resources`. Code sits in `MacOS`, data in
`Resources`, as `codesign` requires.

* **One architecture per bundle.** Build twice for both Mac families; there is no universal binary. Both executables
  must be thin and must not require a macOS newer than the bundle's `LSMinimumSystemVersion` (12.0).
* **`Info.plist` is generated** from the manifest and `--app-version` (one to three integers). Add your own keys with
  `infoPlist`. Keys the packager computes cannot be overridden.
* **Signing comes from the environment, never the configuration**: `MACOS_SIGN_IDENTITY` (a "Developer ID Application:
  …" identity name). Without it the build is signed *ad hoc*, which runs on the machine that built it but is blocked by
  Gatekeeper elsewhere. To sign nothing at all, see [Unsigned builds](#unsigned-builds). Everything is signed inside out (stub, shell, bundle) with the **hardened runtime**, so an
  ad-hoc build behaves like the final one.
* **Entitlements** are optional per binary (`entitlements.shell`, `entitlements.worker`), applied at signing and
  verified afterwards. The hardened runtime blocks things PHP may need. Grafida's worker, for example, needs only
  `com.apple.security.cs.allow-jit`, so that PCRE's JIT can allocate memory; without it PCRE falls back to the
  interpreter and logs a warning on every start. Start with none and add one only when you see a failure.
* **Notarisation** runs when `MACOS_NOTARY_PROFILE` names a `notarytool` keychain profile (it needs the identity too):
  submit, wait, staple, validate. Create the profile once with `xcrun notarytool store-credentials`.
* The disk image is laid out by Finder through AppleScript, best effort: it needs Finder's Automation permission.
  Use `--no-style` on a CI runner without Finder. The image is compressed (UDZO) and signed with your identity.

### Windows

```jsonc
"windows": {
  "name": "MyApp",
  "icon": "assets/MyApp.ico",
  "version": {"companyName": "Example Ltd", "legalCopyright": "© Example Ltd", "workerDescription": "MyApp worker"},
  "files": {"LICENSE.txt": "LICENSE.txt"},
  "installer": {"script": "build/windows/installer.nsi", "defines": {"SHORTCUTS": "yes"}}
}
```

* x86_64 only. The tree holds `<name>.exe` (the shell), `<worker>.exe` (the stub), `<worker>.phar`, `shell.json`, the
  icon and your extra `files`. Icon and version resources are written in PHP, on any host.
* **Installers are your own NSIS script** (`installer.script`), compiled with `makensis`. The packager hands it the
  tree and these defines: `SRCDIR`, `OUTFILE`, `APPNAME`, `APPID`, `APPEXE`, `WORKEREXE`, `WORKERPHAR`, `ICONFILE`,
  `APPVERSION`, `VIVERSION` and `PUBLISHER` (taken from `windows.version.companyName`), plus anything of your own in
  `installer.defines`, such as `SHORTCUTS` above (you may not redefine the packager's).
  A complete working script, installing per user without elevation and checking for WebView2, is
  [`tests/fixtures/app/installer.nsi`](https://github.com/grafida/shell-packager/blob/main/tests/fixtures/app/installer.nsi).
  Start from it.
* **Signing comes from the environment** and uses Jsign: Azure Trusted Signing (`WINDOWS_SIGN_ENDPOINT`,
  `WINDOWS_SIGN_PROFILE` as `account/profile`, `WINDOWS_SIGN_TOKEN`), or a keystore for testing
  (`WINDOWS_SIGN_PKCS12`, `WINDOWS_SIGN_PKCS12_PASSWORD`). With neither, everything is left unsigned (see [Unsigned builds](#unsigned-builds)). The packager checks
  each signature with `osslsigncode`.
* When signing, your NSIS script receives `SHELL_PACKAGER_SIGN` and **must** sign its uninstaller with
  `!uninstfinalize '${SHELL_PACKAGER_SIGN}' = 0`. The uninstaller is written inside the installer, so nothing can sign it later.
  A script that does not is refused.

## Unsigned builds

Signing needs paid certificates and credentials. For development, CI smoke tests, internal tools and hobby projects you
can build without it. Add `--no-sign` to `macos-app`, `macos-dmg`, `windows-tree` or `windows-installer`:

```sh
php -d phar.readonly=0 vendor/bin/shell-packager macos-app --config=shell-package.json --app-version=1.2.3 --no-sign
php -d phar.readonly=0 vendor/bin/shell-packager windows-installer --config=shell-package.json --app-version=1.2.3 --no-sign
```

`--no-sign` wins over any signing credentials in the environment. Linux packages are never signed, so they need nothing.
On Windows, a build with no credentials in the environment is already unsigned; `--no-sign` just makes it explicit.

**What the package is not.** An unsigned package works, but the operating systems treat it as software of unknown
origin. Tell your users what to expect, because the first thing they see is a warning, not your app.

### macOS

* **Gatekeeper rejects the app.** The bundle is not sealed, so `codesign --verify` fails ("code has no resources but
  signature indicates they must be present") and `spctl --assess` rejects it. If the user downloaded it (a browser, a
  chat app and AirDrop mark files as quarantined), macOS refuses to open it and may say the app is "damaged" or from an
  unidentified developer. To open it anyway, on recent macOS the user must go to **System Settings → Privacy & Security**,
  find the message about the app, and choose **Open Anyway**; older versions allow Control-click → **Open**. From a
  terminal, `xattr -dr com.apple.quarantine /Applications/MyApp.app` clears the quarantine flag. A copy that never
  went through a browser (built locally, or copied with `scp`) carries no quarantine flag.
* **It cannot be notarised.** Notarisation needs a Developer ID signature. Setting `MACOS_NOTARY_PROFILE` together with
  `--no-sign` is an error.
* **"Unsigned" is not quite nothing.** The shell and the worker stub were released with an ad-hoc *linker* signature,
  which Apple Silicon requires of every program it runs. `--no-sign` leaves it exactly as it came, so the binaries still
  run. It adds nothing: no seal over `Info.plist` and `Resources`, no hardened runtime, no entitlements (the `entitlements`
  settings are ignored). The packaged worker was run inside such a bundle and answered `ready`.
* **Every update asks again.** The warning returns for each new download.
* The ad-hoc build you get without `--no-sign` (no `MACOS_SIGN_IDENTITY`) is the middle ground: it is sealed and
  runs under the hardened runtime, which makes it a good test of your entitlements, but Gatekeeper rejects it elsewhere
  just the same.

### Windows

These are behaviours of Windows rather than of the packager; they have not been verified on a test machine for this guide.

* **SmartScreen warns.** A downloaded unsigned installer or program (it carries the "mark of the web") shows "Windows
  protected your PC". The user must choose **More info → Run anyway**. An unsigned file has no publisher, so it never
  builds a reputation; a signed one eventually can.
* **The publisher is "Unknown publisher"** in the installer and in the file's properties.
* **Smart App Control** (Windows 11, when it is switched on) blocks unsigned programs outright, with no "Run anyway".
  The user has to turn Smart App Control off, which cannot be undone without reinstalling Windows. This is the biggest
  obstacle to distributing unsigned Windows software to ordinary users.
* **Antivirus programs** are more suspicious of unsigned executables, and a program that unpacks a PHP runtime and starts
  child processes looks suspicious to some of them. Expect the occasional false positive.
* **Managed computers** often only allow signed software through policies such as AppLocker or Windows Defender Application Control. For these users an unsigned build does not work at all.
* **Updating is no different:** each new unsigned build gets the same treatment.

### When not to ship unsigned

For anything you hand to people outside your team, sign and (on macOS) notarise. Unsigned builds are for you and for
people who understand the warnings.

## Providing signing credentials

Need the certificates first? [Getting signing certificates](code-signing.md) walks through Apple Developer ID and
notarisation, and Azure Artifact Signing.

Signing credentials come from **environment variables**, never from `shell-package.json`. The configuration is meant to
be committed; credentials must not be. Set the variables in whatever way suits you: your CI's secret store, your shell
profile, or a git-ignored file that you load before building (`set -a; . ./sign.env; set +a`). The packager does not care
how they got there. If a variable is absent, that kind of signing is simply off (see [Unsigned builds](#unsigned-builds)).

| Platform | Variable | Meaning |
|---|---|---|
| macOS | `MACOS_SIGN_IDENTITY` | The name of a "Developer ID Application: …" certificate in a keychain the build can use. Unset: signed ad hoc. |
| macOS | `MACOS_NOTARY_PROFILE` | A `notarytool` keychain profile, created once with `xcrun notarytool store-credentials`. Needs the identity. Unset: not notarised. |
| Windows, certificate file | `WINDOWS_SIGN_PKCS12` | Path to a PKCS#12 (`.p12` / `.pfx`) keystore. |
| Windows, certificate file | `WINDOWS_SIGN_PKCS12_PASSWORD` | Its password. |
| Windows, Azure Trusted Signing | `WINDOWS_SIGN_ENDPOINT` | The signing endpoint's URL. |
| Windows, Azure Trusted Signing | `WINDOWS_SIGN_PROFILE` | `<account>/<profile>`. |
| Windows, Azure Trusted Signing | `WINDOWS_SIGN_TOKEN` | An access token for `https://codesigning.azure.net`. You obtain it (for example with the Azure CLI or a CI step) and export it; the packager does not fetch it. Tokens are short-lived, so get one just before building. |
| Windows, optional | `WINDOWS_SIGN_TSAURL` | Overrides the timestamp server. |
| Windows, optional | `WINDOWS_SIGN_CAFILE` | Root certificates to verify the finished signature's chain against. Without it only the signed digest is checked. |

Notes:

* **Windows: use one method.** Trusted Signing needs all three of its variables, and a keystore needs its password;
  setting only some of them, or both methods together, is an error. With none of the variables set, the build is
  unsigned.
* The Windows secrets reach the signing tool as `env:` references, never as command-line arguments, so they do not show
  up in process listings or build logs.
* The macOS certificate and notary credentials live in the keychain, so no secret passes through the environment for them
  at all; the variables only name them. A CI machine has to import them into a keychain first.
* Keep `WINDOWS_SIGN_PKCS12_PASSWORD` and `WINDOWS_SIGN_TOKEN` out of version control, shell history and build logs.

## System requirements

Your users need the following. The shell's **launcher** on Linux asks users to read *your* documentation when the
shell cannot start, and it cannot be customised, so copy this section into your own documentation and add anything your
app needs on top.

### Windows requirements

- Windows 10 22H2 or later, Windows 11
- x86_64 only; there is no Arm build
- Microsoft Edge **WebView2 Runtime** (Evergreen)

The WebView2 Runtime ships with Windows 11. It is installed on Windows 10 through Windows Update, so it is almost always
already present. If it is missing, the shell shows an alert when it starts. It can be installed directly
[from Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/).

### macOS requirements

- macOS 12 Monterey or later
- Apple Silicon (arm64) or Intel (x86_64)

On an older macOS the system refuses to open the app. There is a separate build per architecture; there is no universal
binary. Support for macOS on x86 is deprecated: it is supported only up to macOS 26 (the last version with x86 support
and the last with Rosetta 2 on Apple Silicon) and will be removed when macOS 28 is released.

WKWebView is part of macOS. There is nothing to install.

### Linux requirements

- Linux on x86_64 or arm64 (aarch64)
- GNU C Library (glibc) **2.35** or later
- **GTK 3** and **WebKitGTK 4.1**, version **2.40** or later

Ubuntu 22.04 and later, Debian 12 and later (both with their updates), and current releases of Fedora and Arch Linux
meet the requirements. Install the libraries with:

| Distribution | Command |
|---|---|
| Debian, Ubuntu and their derivatives | `sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0` |
| Fedora | `sudo dnf install webkit2gtk4.1 gtk3` |
| Arch Linux | `sudo pacman -S webkit2gtk-4.1 gtk3` |

An Ubuntu 22.04 installation does not have WebKitGTK 4.1 by default.

Also used, and normally installed with the desktop:

- **File dialogs** go through the XDG Desktop Portal, so they need a portal backend for the desktop:
  `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde` or `xdg-desktop-portal-gtk`. Without one, the dialogs fall back to
  `zenity`. The wlroots backend (`xdg-desktop-portal-wlr`) does not offer file dialogs.
- **Spell checking** needs a Hunspell dictionary for each language the user writes in (`hunspell-en-us` on Debian and
  Ubuntu, `hunspell-en-US` on Fedora, `hunspell-en_us` on Arch Linux, for US English). Without one, spell check does nothing.
- **The launcher's alert** is shown with the first of `zenity`, `kdialog`, a desktop notification (`notify-send`) or
  `xmessage` that works. Its text is always written to stderr as well.

## Release checklist

1. Bump your version; commit `composer.lock`.
2. Check `composer.json`'s `ext-*` requirements against the [extension list](#the-php-runtime-inside-the-package).
3. Build each package with `php -d phar.readonly=0 vendor/bin/shell-packager …`.
4. Install each package on a clean machine (or VM) of that platform. Start it, use every feature that touches the
   disk, the database, dialogs and the clipboard, then quit.
5. Publish the packages together with their SHA-256 checksums.
