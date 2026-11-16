# Packaging

[grafida/shell-packager](https://github.com/grafida/shell-packager) turns your app into something users can install:
for Linux a tarball, `.deb` and `.rpm` packages, an AppImage and an Arch Linux `PKGBUILD`; a disk image for macOS; an
installer for Windows. Which of them you can build depends on the machine you build on: see
[Which machine builds what](#which-machine-builds-what).

## Which machine builds what

**To build for macOS you need a Mac.** The bundle is sealed with `codesign` and the disk image is made with `hdiutil`,
and both exist only on macOS. Even an unsigned disk image (`--no-sign`) is made with `hdiutil`. Nothing replaces them
on another operating system.

| Build machine | Linux tarball, `PKGBUILD` | `.deb`, `.rpm` | AppImage | Windows installer | macOS disk image |
|---|---|---|---|---|---|
| **A Mac** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Linux** | ✅ | ✅ | ✅ | ✅ | ❌ |
| **Windows** | ✅ | ❌ | ✅ with Docker (untested) | ✅ | ❌ |

So with a Mac you can produce every deliverable, for both CPU architectures of each platform. With Linux you can
produce the Linux and Windows ones, and for macOS you need a Mac, or a macOS runner in your CI.

* The Linux tarball and the `PKGBUILD` are written by the packager itself, in PHP, so they need no extra tool on any
  host.
* The `.deb` and `.rpm` are made by [nfpm](https://nfpm.goreleaser.com), which the packager downloads and verifies
  itself. It pins nfpm for macOS and Linux build machines only.
* The AppImage needs `mksquashfs`: either installed on the build machine, or run in a Docker container that the packager
  builds itself. On Windows, Docker is the only way. That route was built for Windows, but so far it has only been run
  on a Mac. See [AppImage](#appimage).
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
PHAR, with no PHP installation on the machine. Where each file goes on each platform is shown with that platform's
package, under [Building](#building).

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
        "grafida/shell-packager": "^0.6"
    }
}
```

You need, on the build machine:

* PHP 8.4 with `curl`, `json`, `mbstring`, `phar` and `zlib`, run with `phar.readonly` off: invoke the packager as
  `php -d phar.readonly=0 vendor/bin/shell-packager …`.
* `composer` on `PATH`, and a committed **`composer.lock`**. The packager installs your runtime dependencies from the
  lock with `--no-dev --no-scripts` into a staging directory; your own `vendor/` is never used, so development
  dependencies cannot leak into the package. Composer `scripts` do not run during that install.
* For the AppImage: `mksquashfs` (Homebrew `squashfs`, apt `squashfs-tools`), or Docker. See [AppImage](#appimage).
* For macOS packages (on a Mac only): Xcode's command line tools (`codesign`, `hdiutil`, and for notarisation `notarytool`).
* For Windows packages: `makensis` 3.08 or later (the installer), and for signing `jsign` and `osslsigncode`.

The tarball, the `.deb`/`.rpm` and the `PKGBUILD` need nothing else: the packager downloads and verifies nfpm itself.

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
      // shell.js, which the page loads. Do not leave this out (see below).
      {"directory": "vendor/grafida/shell-php/resources"}
    ],
    "ini": {"memory_limit": "512M", "display_errors": "stderr", "opcache.enable": false}
  },
  "shell":    {"version": "^0.4"},   // the shell and launcher, by version: see below
  "runtimes": { /* the PHP and AppImage runtimes, pinned: see below */ },
  "linux":   { /* … */ },
  "macos":   { /* … */ },
  "windows": { /* … */ }
}
```

> [!IMPORTANT]
> **Do not forget the last `include` rule above.** `shell.js`, the helper your page loads to reach native features, is
> not a PHP file, and it lives inside `vendor/`, so the `vendor` rule (which only takes `*.php`) skips it. Without the
> `{"directory": "vendor/grafida/shell-php/resources"}` rule, the packaged app works in `--dev` but answers `/shell.js`
> with `500`, and every native feature (file dialogs, the clipboard, drag and drop) fails. Details below.

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

### `shell`: the shell and launcher, by version

The packager downloads the **shell executable** for each platform and the **Linux launcher** from a release of
[grafida/shell](https://github.com/grafida/shell/releases). You choose the release with a version constraint (packager
0.6.0 or later):

```jsonc
"shell": {
  "version": "^0.4"
}
```

* `version` (required) is a [Composer version constraint](https://getcomposer.org/doc/articles/versions.md), with
  Composer's meaning: `^0.4` is any 0.4.x, `^1.2` is 1.2 or later below 2.0, `~1.2.3` is 1.2.3 or later below 1.3,
  `>=1.0 <2.0` and `*` are what they say, and `0.4.2` is exactly that release. Every grafida/shell release is a tag
  `X.Y.Z` that follows semantic versioning, and the packager takes the **highest** one the constraint allows.
* Only stable releases are used, as in Composer, unless the constraint asks for less: a stability flag (`^1.0@beta`,
  `^1.0@RC`, `@alpha`, `@dev`), or a pre-release written out (`1.0.0-rc1`). The order is dev, alpha, beta, RC, stable.
  The version decides, not GitHub's "pre-release" checkbox. Draft releases are never used.
* `repository` (optional, default `grafida/shell`) is the GitHub repository whose releases are used, as `owner/name`,
  for a fork of the shell. **Public repositories only.**
* `assets` (optional) renames the files, for a fork that publishes them under other names. The defaults are what
  grafida/shell publishes:

  ```jsonc
  "assets": {
    "linux":   {"shell": "grafida-shell-linux-{arch}", "launcher": "grafida-launcher-linux-{arch}"},
    "macos":   {"shell": {"aarch64": "grafida-shell-macos-arm64", "x86_64": "grafida-shell-macos-x86_64"}},
    "windows": {"shell": "grafida-shell-windows-{arch}.exe"}
  }
  ```

  A name is a string in which `{arch}` stands for `x86_64` or `aarch64`, or an object with one name per architecture
  (the macOS shell says `arm64`, hence the object). You give only what differs: each platform's `shell` or `launcher`
  replaces that one default. There are exactly seven files (Linux shell and launcher on x86_64 and aarch64, macOS shell
  on aarch64 and x86_64, Windows shell on x86_64); naming any other is an error.
* `local` (optional) points at files on disk instead, to test a shell you have not released; see
  [below](#testing-an-unreleased-shell).

#### The lock file: `shell-package.lock`

`shell-packager update --config=shell-package.json` resolves the constraint and writes what it found to
`shell-package.lock`, beside the configuration file (the name follows the configuration's: `foo.json` gives
`foo.lock`). **Commit the lock file.** It records the release (version and tag) and, for each of the seven files, its
download URL and its SHA-256, which is GitHub's own record of it (the asset's `digest`). `update`:

1. lists the repository's releases, through the [GitHub CLI](https://cli.github.com) when `gh` is installed and logged
   in, and otherwise straight from GitHub's API (with `GH_TOKEN` or `GITHUB_TOKEN`, when set, against the rate limit
   for anonymous requests);
2. picks the highest version the constraint allows;
3. downloads all seven files and checks each against its SHA-256;
4. only then writes the lock, and prints the old and new version and every file's hash.

A release that lacks one of the seven files, or a file GitHub has no SHA-256 for, makes `update` fail.

A build reads the lock and **makes no GitHub API call**: it downloads the files it needs from the URLs in the lock and
checks each against the lock's SHA-256. Files are cached by repository and version in `SHELL_PACKAGER_CACHE` (default
`$XDG_CACHE_HOME/grafida-shell-packager`, under `shell/<owner>/<repo>/<version>/`), and a cached file is checked again
every time it is used.

* **No lock yet:** the first build does what `update` does, writes the lock, says `commit it`, and carries on.
* **The lock is stale** (you changed `version`, `repository` or `assets` since it was written): the build fails and
  tells you to run `update`. It never resolves on its own.
* **The lock does not match the configuration** (another repository, a version the constraint does not allow, or a URL
  that is not that release's file on GitHub): it is refused. The lock is written by `update` only; do not edit it.

**Upgrading the shell** is `shell-packager update`: it moves to the newest release your constraint allows. To move
past the constraint, change `version` first. With an exact version (`"0.4.2"`) every build uses the same files, lock or
not.

#### Testing an unreleased shell

To try a shell you built yourself with your app, without publishing a release, name the files in `local`, in the same
shape as `assets`, with paths relative to the configuration file:

```jsonc
"shell": {
  "version": "^0.4",
  "local": {
    "macos": {"shell": {"aarch64": "../shell/target/release/grafida-shell"}},
    "linux": {"shell": "../shell/dist/grafida-shell-linux-{arch}"}
  }
}
```

You can name any of the seven files. Each local file is used as it is and announced as **UNPINNED**, with its path and
SHA-256, every time; never ship a build made with one. The files you do not name still come from the lock. A build
that needs only local files needs no lock and no network. `local` never makes the lock stale. `--local shell=PATH`
(or `launcher=PATH`) on the command line does the same for one build and wins over `local`.

#### Moving from packager 0.5

Packager 0.5 pinned the shell and launcher as `runtimes.<platform>.shell` and `runtimes.linux.launcher`, with a URL and
hashes each. Those entries are now refused, with a message saying what to do: delete them all, add the `shell` block,
and run `shell-packager update`. `{"latest": true}` no longer applies to the shell or launcher; `"version": "*"` and
`update` do the same, and record what they found.

#### Publishing a fork of the shell

What the packager expects of `repository`'s releases, which grafida/shell keeps:

* one tag per version, following semantic versioning (`X.Y.Z`, or `vX.Y.Z`; a tag that is not a version is ignored);
* every release carries all seven files, under the default names or under the ones your apps put in `assets`;
* the repository is public, and the files are GitHub release assets, which GitHub records a SHA-256 for. The packager
  uses that record. The `.sha256` files the shell publishes beside each asset are for people downloading by hand;
  the packager does not read them;
* a published release is never changed. Replacing an asset makes every lock that names it fail its check. Publish a
  new version instead.

**Linux only:** `launcher` is a small static program that is the app's entry point. It checks that the system has the
libraries the shell needs and either starts the shell or shows a helpful alert listing the packages to install.

### `runtimes`: the PHP runtime and the AppImage runtime

The packager also downloads the **phpmicro stub** for each platform, and for an AppImage the **AppImage runtime**. You
pin each by URL and SHA-256, so a build cannot silently change under you:

```jsonc
"runtimes": {
  "linux": {
    "sfx":      {"url": "https://github.com/grafida/phpmicro/releases/download/sfx-20261109.16/linux-{arch}.standard.sfx",
                 "sha256": {"x86_64": "…", "aarch64": "…"}},
    // Only for linux-appimage:
    "appimage-runtime": {"url": "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-{arch}",
                         "sha256": {"x86_64": "…", "aarch64": "…"}}
  },
  "macos": {
    "sfx":   {"url": {"aarch64": "https://github.com/grafida/phpmicro/releases/download/sfx-…/macos-aarch64.standard.sfx",
                      "x86_64": "https://github.com/grafida/phpmicro/releases/download/sfx-…/macos-x86_64.standard.sfx"},
              "sha256": {"aarch64": "…", "x86_64": "…"}}
  },
  "windows": {
    "sfx":   {"url": "https://github.com/grafida/phpmicro/releases/download/sfx-…/windows-x86_64.standard.sfx",
              "sha256": {"x86_64": "…"}}
  }
}
```

* `{arch}` in a URL is `x86_64` or `aarch64`. A `url` can instead be an object mapping each architecture to its own URL,
  for when they come from different releases.
* The `sfx` stubs come from [grafida/phpmicro releases](https://github.com/grafida/phpmicro/releases) (tags
  `sfx-<date>.<run>`, which are never rewritten). For releases, pin an immutable tag; `sfx-latest` is what
  [`latest`](#using-the-latest-release-instead-of-a-pin) uses.
* **Where the hashes come from:** download the asset yourself, compute its SHA-256 (`shasum -a 256 FILE`), and record it.
  A mismatch with the hash in your configuration, or a missing pin for an architecture, is fatal (unless the runtime is
  [`latest`](#using-the-latest-release-instead-of-a-pin)). Downloads are cached by hash in `SHELL_PACKAGER_CACHE`.
* `shell-packager fetch --config=shell-package.json --arch=x86_64 [--platform=linux|macos|windows]` downloads and verifies
  the shell files and the runtimes without building.
* `--local NAME=PATH` replaces one file (`shell`, `launcher`, `sfx`, `appimage-runtime`, or `nfpm` for
  `linux-deb`/`linux-rpm`) with a local file, to test an unreleased build. It is announced as UNPINNED every time; never
  use it for a release.

#### Finding the URLs and hashes to pin

**1. Choose the release.** On [grafida/phpmicro releases](https://github.com/grafida/phpmicro/releases), pick the newest
`sfx-<date>.<run>` tag (not `sfx-latest`, which moves). Each tag holds the stubs for every platform and, for Windows,
`vc-runtime-x86_64.zip`, which you do not pin.

**2. Build the URLs.** A URL is `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`. The asset names
are:

| Runtime | Asset |
|---|---|
| `sfx` | `<platform>-<arch>.standard.sfx`, e.g. `linux-x86_64.standard.sfx`, `macos-aarch64.standard.sfx`, `windows-x86_64.standard.sfx` |
| `appimage-runtime` (Linux only) | `runtime-x86_64`, `runtime-aarch64`, from [AppImage/type2-runtime releases](https://github.com/AppImage/type2-runtime/releases) (a dated tag such as `20251108`, not `continuous`, which moves) |

**3. Get the hashes.** Compute them yourself from the downloaded file; do not copy them from anywhere unchecked:

```sh
shasum -a 256 linux-x86_64.standard.sfx            # macOS, Linux
sha256sum linux-x86_64.standard.sfx                # Linux
Get-FileHash -Algorithm SHA256 windows-x86_64.standard.sfx   # Windows PowerShell
```

You can also read GitHub's own record of the hash, without downloading, with the [GitHub CLI](https://cli.github.com):

```sh
gh release view sfx-20261111.28 -R grafida/phpmicro --json assets \
  -q '.assets[] | "\(.digest | sub("^sha256:"; ""))  \(.name)"'
```

**4. Let the packager do it for you.** To avoid copying 64-character strings by hand, put the URLs in
`shell-package.json` with a placeholder hash of 64 zeros (the packager insists on 64 hex digits) and run
`shell-packager fetch`. It fails with `does not match its pin`, and the `got:` line is the actual SHA-256 of what was
downloaded. Treat that as **a value to verify, not to trust**: it is the hash of whatever the URL served. Compare it
with the value from `gh release view`, then paste it in. (Alternatively, make the runtime
[`latest`](#using-the-latest-release-instead-of-a-pin) once: the warning prints the SHA-256 it downloaded, to compare in
the same way.)

Pin every architecture you build. Hashes are per file, so each platform and architecture needs its own entry, and the
packager fails when the one it needs is missing. Re-do the steps whenever you upgrade; the URL and its hash always
change together.

#### Using the latest release instead of a pin

Instead of a URL and hashes, a runtime can be just `{"latest": true}`:

```jsonc
"runtimes": {
  "linux": {
    "sfx": {"latest": true}
  }
}
```

Each runtime is either pinned or `latest`; giving both is an error. The packager picks the right asset for the platform
and architecture:

| Runtime | Resolves to |
|---|---|
| `sfx` | The `sfx-latest` release of [grafida/phpmicro](https://github.com/grafida/phpmicro/releases/tag/sfx-latest), which is rebuilt on every push to its `sibling-phar` branch. |
| `appimage-runtime` | The newest release of [AppImage/type2-runtime](https://github.com/AppImage/type2-runtime/releases/latest), which is its rolling `continuous` build. Needs packager 0.6.0 or later. |

Every use prints a `WARNING: … is LATEST, not pinned` line with the URL and the SHA-256 of what was downloaded, and the
file is cached under that hash.

> [!WARNING]
> **"Latest" changes your build under you.** Nothing is checked against a hash you chose, so two builds of the same
> commit, made on different days, can ship different PHP runtimes: a new PHP patch version, a changed extension list, a
> different minimum operating system. A release can contain incompatible changes, and you will find out when your
> packaged app misbehaves, not when you build it. **Do not use `latest` for a release you ship to users.** It is for
> trying an upcoming runtime during development, and for CI jobs that exist to catch incompatibilities early. For
> releases, pin: take the same asset from a specific release, check that its SHA-256 is the one in the warning, and the
> build stays the same until you change it.

`latest` is rejected for a runtime that does not exist on a platform (there is no macOS or Windows AppImage runtime).
It needs packager 0.5.0 or later; earlier versions require a pin for everything.

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

Always run the packager as `php -d phar.readonly=0 vendor/bin/shell-packager …`. Every command except `windows-sign`
takes `--config=shell-package.json`; every command that builds a package also takes `--app-version=1.2.3`.

| Command | Result |
|---|---|
| `phar --output=build/worker.phar` | Just the worker payload. Useful to inspect what ships. |
| `linux-tarball --arch=x86_64` (or `aarch64`) | A `.tar.gz` that users unpack and run, with an `install.sh` |
| `linux-deb --arch=…`, `linux-rpm --arch=…` | A `.deb` or `.rpm` package that installs under `/opt` |
| `linux-appimage --arch=…` | An AppImage: a single executable file |
| `linux-pkgbuild [--arch=…]…` | An Arch Linux `PKGBUILD` for tarballs you have already built |
| `macos-app [--arch=…]` | A signed `.app` bundle (default `aarch64`; `--arch=x86_64` for Intel) |
| `macos-dmg [--arch=…] [--no-style] [--app=BUNDLE]` | A disk image holding the bundle and an `/Applications` link. `--app` images a bundle you already built instead of building one. |
| `windows-tree` | An install tree (x86_64 only) |
| `windows-installer` | A Windows installer `.exe` |
| `windows-sign --file=FILE [--overlay]` | Signs one executable. Your installer script runs it for the uninstaller. |
| `fetch --arch=… [--platform=linux\|macos\|windows] [--nfpm]` | Downloads and verifies the shell files and the runtimes (and with `--nfpm` the pinned nfpm) without building anything |
| `update` | Resolves `shell.version` against the shell's releases, downloads and checks the seven shell files, and writes `shell-package.lock` (see [the lock file](#the-lock-file-shell-packagelock)) |
| `version`, `help` | The packager's version; every command and option |

`--arch` takes `x86_64` or `aarch64`, or their Debian names `amd64` and `arm64`. `--output=` overrides the output path,
`--no-check` skips the worker test below, and `--no-sign` builds a macOS or Windows package with nothing signed (see
[Unsigned builds](#unsigned-builds)). Output paths in the configuration may use `{name}`, `{version}`, `{arch}`
(`x86_64`/`aarch64`) and `{debarch}` (`amd64`/`arm64`). Every package gets a `<file>.sha256` beside it, in `shasum`
format, ready to publish.

**Reproducibility.** With `SOURCE_DATE_EPOCH` set, the PHAR, the Linux tarball and the AppImage are byte-identical for
identical inputs. For the AppImage, `mksquashfs` and its zstd library must be the same versions, wherever they run. The
macOS bundle and image are not reproducible: `codesign` and `hdiutil` are not deterministic.

### The packaged worker is tested before it is written

On a machine of the target's operating system and CPU, the packager runs the packaged worker and checks that:

* the `ini` settings read back exactly (phpmicro silently ignores a malformed block); and
* it sends the protocol's `ready` frame and exits when its standard input closes. Every shell worker speaks the
  protocol, so this works for any app.

Both run with every per-user directory pointed at a temporary directory. That proves the PHAR, your autoloader and your
bootstrap load from `phar://`.

* **On macOS** the handshake runs on the **signed** worker stub inside the finished bundle, so it also proves the stub
  finds its payload in `Contents/Resources` and runs under the hardened runtime with the entitlements you gave it. An
  Apple Silicon Mac with Rosetta 2 also tests an x86_64 bundle.
* **On Windows** it runs on the signed stub in the finished tree, after its resources were rewritten and its signature
  appended.
* **When you cross-build** (the Windows package on a Mac, a Linux package anywhere but Linux, an x86_64 bundle on an
  Apple Silicon Mac without Rosetta 2), the test is skipped with a note, never faked. Test a cross-built package on the
  target before you publish it.

### Linux

```jsonc
"linux": {
  "name": "myapp",
  "desktop": "assets/myapp.desktop",
  "icons": {"48": "assets/icons/48.png", "512": "assets/icons/512.png"},
  "files": {"LICENSE.txt": "LICENSE.txt"},
  "replaces": [],
  "output": "build/dist/MyApp-{version}-linux-{debarch}.tar.gz",
  "package":  { /* .deb and .rpm: see below */ },
  "appimage": { /* the AppImage: see below */ },
  "pkgbuild": { /* the PKGBUILD: see below */ }
}
```

These members are shared by every Linux package:

* `name` is the launcher's file name and the package name. By default it is your title in lower case, with every run
  of other characters turned into `-` (`My App` becomes `my-app`). For a `.deb` or `.rpm` it must be a valid package
  name: lower-case letters, digits, `.`, `+` and `-`.
* `desktop` is a freedesktop `.desktop` file you provide. The packager forces its `Exec`, `Icon` and `StartupWMClass`
  to agree with the package. The desktop entry is named `<id>.desktop` after your manifest's `id`, and so are the icons,
  because the shell's window reports the `id` as its Wayland `app_id`.
* `icons` maps pixel sizes to PNG files. `files` adds extra files beside the shell.
* `replaces` lists the desktop ids of an earlier build of your app installed under another id (`["myapp"]` for an app
  whose old packaging used `myapp.desktop`); see each package below for what is cleaned up.
* `output` is the tarball's path. The default is `build/dist/{name}-{version}-linux-{arch}.tar.gz`.

WebKitGTK and GTK are **never bundled** in any Linux package: the shell uses the system's. The launcher checks for them
and shows an alert listing the packages to install when they are missing.

> [!WARNING]
> A relative `logDir` in your manifest is resolved beside the shell. Users cannot write under `/opt`, where the `.deb`,
> `.rpm` and `PKGBUILD` install it, so use `"auto"` (or an absolute path) for an app you package that way.

#### The tarball

`linux-tarball` writes:

```
myapp/
  myapp                  the launcher (static): checks the system libraries, alerts, starts the shell
  grafida-shell          the shell
  myapp-worker           the worker stub
  myapp-worker.phar      the worker payload
  shell.json
  <id>.desktop
  icons/hicolor/<n>x<n>/apps/<id>.png
  install.sh
  …                      linux.files
```

Users can run the launcher directly, or run `./install.sh` to add a desktop entry and icons for themselves
(`./install.sh --uninstall` removes them). It needs no root. With `replaces`, `install.sh` also removes each old id's
`~/.local/share/applications/<id>.desktop` and its icons, but only when that entry's `Exec` (field codes dropped, quotes
removed) ends in `/<name>`. An entry under that id that starts anything else is left alone, and `install.sh` says so.

#### `.deb` and `.rpm`

`linux-deb` and `linux-rpm` write packages with the same files, installed system-wide:

```
/opt/myapp/                       myapp, grafida-shell, myapp-worker, myapp-worker.phar, shell.json, linux.files
/usr/bin/myapp                    a symbolic link to /opt/myapp/myapp
/usr/share/applications/<id>.desktop        Exec=myapp, Icon and StartupWMClass = the id
/usr/share/icons/hicolor/<n>x<n>/apps/<id>.png
```

The package metadata is yours, under `linux.package`:

```jsonc
"package": {
  "maintainer": "Jane Doe <jane@example.com>",     // required
  "license": "GPL-3.0-or-later",                   // required
  "homepage": "https://example.com",
  "description": "One paragraph.",                  // default: the manifest's title
  "section": "utils",                               // .deb only; default utils
  "depends": {"deb": ["xdg-utils"], "rpm": ["xdg-utils"]},
  "output": {"deb": "build/dist/{name}-{version}-linux-{debarch}.deb",
             "rpm": "build/dist/{name}-{version}-linux-{arch}.rpm"}
}
```

* The packages **depend on WebKitGTK 4.1 and GTK 3**, so the package manager installs them: `libwebkit2gtk-4.1-0` and
  `libgtk-3-0` for the `.deb`, `webkit2gtk4.1` and `gtk3` for the `.rpm`. These are the shell's own needs, so the
  packager adds them for you; `depends` only adds to them.
* Architectures are `amd64`/`arm64` for the `.deb` and `x86_64`/`aarch64` for the `.rpm`. A hyphen in `--app-version`
  becomes `~`, so `1.0-rc1` is packaged as `1.0~rc1`, which sorts before `1.0`.
* No field may contain a `$`, because nfpm expands environment variables in its configuration.
* After installing or removing, the packages refresh the desktop-entry and icon caches. On a final removal (not an
  upgrade) they delete `/opt/<name>`, with anything the app wrote there. They never touch a home directory: user data is
  left alone, and so is a per-user entry from the tarball's `install.sh`. With `replaces`, installing removes a
  system-wide `/usr/share/applications/<old id>.desktop` and its icons when its `Exec` is `<name>` or ends in `/<name>`.
* The packages are **not signed**. Installing the `.rpm` with `rpm` needs `--nosignature`, and with `dnf` it needs
  `--nogpgcheck` wherever dnf checks the signatures of local packages. A plain `dnf install ./myapp-….rpm` worked on
  Fedora 43.
* nfpm 2.49.0 is pinned inside the packager, for macOS and Linux build machines (x86_64 and aarch64), and verified by
  SHA-256 like every runtime. `fetch --nfpm` downloads it ahead of time.

#### AppImage

`linux-appimage` writes a single executable file. Users download it, mark it executable and run it; nothing is
installed. Inside, it holds:

```
AppRun                 the launcher; the AppImage runtime starts it
grafida-shell          the shell
myapp-worker, .phar    the worker stub and its payload
shell.json
<id>.desktop           the desktop entry, at the root as the AppImage format requires
<id>.png               your largest icon, likewise
.DirIcon -> <id>.png
usr/share/icons/hicolor/<n>x<n>/apps/<id>.png
…                      linux.files
```

An AppImage is a small program, the **AppImage runtime**, followed by a compressed (squashfs) image of those files. The
packager uses the static runtime from [AppImage/type2-runtime](https://github.com/AppImage/type2-runtime), which you pin
as `runtimes.linux.appimage-runtime` (see [`runtimes`](#runtimes-the-php-runtime-and-the-appimage-runtime)). The static runtime does not need
`libfuse2`, which current distributions no longer install. It is not self-contained, though: see
[AppImage requirements](#appimage-requirements) for what your users need.

```jsonc
"appimage": {
  "output": "build/dist/{name}-{version}-linux-{arch}.AppImage",
  "updateInformation": "gh-releases-zsync|myorg|myapp|latest|myapp-*-linux-{debarch}.AppImage.zsync",
  "squashfs": "auto",
  "docker": {"image": "example.com/squashfs@sha256:…"}
}
```

All four are optional.

* `output` defaults to `build/dist/{name}-{version}-linux-{arch}.AppImage`.
* `updateInformation` is written into the AppImage for tools such as AppImageUpdate to read. Its formats are
  `zsync|<URL>` and `gh-releases-zsync|<user>|<repo>|<tag>|<file>`; `{name}`, `{arch}` and `{debarch}` are expanded. The
  packager only embeds it: it does not make the `.zsync` file, and no updater is installed.
* `squashfs` says where the image is made, because the packager needs `mksquashfs` (from squashfs-tools) for it:

  | `squashfs` | Uses |
  |---|---|
  | `"auto"` (default) | `mksquashfs` if it is on the `PATH`; otherwise Docker, if the `docker` command is there and Docker is running; otherwise it stops with an error that names both. |
  | `"host"` | The `mksquashfs` on the `PATH`: `brew install squashfs` on macOS, `apt install squashfs-tools` on Debian and Ubuntu. |
  | `"docker"` | `mksquashfs` in a Docker container. Docker Desktop must be running on macOS and Windows. |

  The choice is made before anything is downloaded, so a missing tool fails fast.
* **Docker**: the packager builds its own small image the first time, from a Dockerfile it ships (Alpine Linux with
  squashfs-tools, all pinned), and tags it `grafida-shell-packager-squashfs:<hash>`. Building it needs network access
  once per machine. It then runs it for each AppImage with no network access, and nothing on your disk is shared with
  the container: the files go in on its standard input and the image comes back on its standard output. To use your own
  image instead, set `docker.image`. It **must be pinned by digest** (`name@sha256:…`; a tag such as `:latest` is
  refused), and it must contain `sh`, `mktemp`, `cat` and `mksquashfs` 4.6 or later.
* **The same AppImage from anywhere.** The image is zstd-compressed, owned by root, without extended attributes, and
  every timestamp is the build time. The packager feeds `mksquashfs` an archive with fixed permissions, so the build
  machine's file system makes no difference. With the same `SOURCE_DATE_EPOCH`, a Mac, a Linux machine and the Docker
  image give byte-identical AppImages, as long as their `mksquashfs` and zstd versions match. The Docker image's
  versions are the ones Homebrew had when the packager was released. `mksquashfs` older than 4.6 (Ubuntu 22.04 has 4.5)
  also works, but only on a Linux build machine.
* The [worker test](#the-packaged-worker-is-tested-before-it-is-written) runs as for the tarball.

#### Arch Linux `PKGBUILD`

`linux-pkgbuild --app-version=V [--arch=ARCH]… [--output=FILE]` writes a `PKGBUILD` for tarballs that `linux-tarball`
already built. Without `--arch` it uses whichever of the two architectures exist; with it, each one must exist.

```jsonc
"pkgbuild": {
  "source": "https://example.com/dl/{name}-{version}-linux-{debarch}.tar.gz",   // where you publish the tarball
  "license": ["GPL-3.0-or-later"],                 // required; SPDX identifiers
  "description": "Edit your notes",
  "url": "https://example.com",
  "maintainer": "Jane Doe <jane@example.com>",
  "depends": ["libsecret"],
  "optdepends": ["wl-clipboard: paste on Wayland"],
  "pkgrel": 1,
  "output": "build/dist/PKGBUILD"
}
```

* Each `source_<arch>` is your `source` URL, and each `sha256sums_<arch>` is computed from the tarball file. Publish the
  tarballs at those URLs before anyone builds the package.
* The package depends on `webkit2gtk-4.1`, `gtk3`, `hicolor-icon-theme` and `dbus`, plus your `depends`. It installs the
  tarball under `/opt/<name>` (without `install.sh` and the icons folder), links `/usr/bin/<name>` to the launcher, puts
  the desktop entry in `/usr/share/applications` and the icons in the hicolor theme.
* It sets `!strip`, so `makepkg` leaves the prebuilt binaries alone. A version like `1.0.0-rc1` becomes `1.0.0_rc1`,
  because pacman does not allow `-`.
* Nothing is published to the AUR.
* A licence that is not one of Arch's common ones needs its text in `/usr/share/licenses/<name>/`. Add it to
  `linux.files` and extend the file by hand, or use a common identifier.

### macOS

```jsonc
"macos": {
  "name": "MyApp",
  "icon": "assets/MyApp.icns",
  "infoPlist": {"NSHumanReadableCopyright": "…", "LSApplicationCategoryType": "public.app-category.productivity"},
  "entitlements": {"worker": "build/macos/worker.entitlements"},
  "resources": {"LICENSE.txt": "LICENSE.txt"},
  "dmg": {"background": "assets/dmg-background.tiff", "window": [640, 400], "iconSize": 128, "textSize": 13,
          "positions": {"app": [160, 210], "applications": [480, 210]}, "volumeIcon": "assets/MyApp.icns",
          "volumeName": "MyApp"},
  "app": "build/shell-macos/{name}.app",
  "output": "build/dist/{name}-{version}-macos-{debarch}.dmg"
}
```

The bundle:

```
MyApp.app/Contents/
  Info.plist                    generated (below)
  MacOS/MyApp                   the shell, the bundle's executable
  MacOS/myapp-worker            the worker stub
  Resources/myapp-worker.phar   the worker payload
  Resources/shell.json
  Resources/MyApp.icns          macos.icon
  Resources/…                   macos.resources
```

Code sits in `MacOS` and data in `Resources`, as `codesign` requires.

* `name` is the bundle's name, without `.app` (default: your title). `app` is where the bundle is written, and `output`
  is the disk image; the defaults are shown above. `dmg.volumeName` (default: the name) is the mounted image's name,
  and `dmg.textSize` its Finder label size.
* **One architecture per bundle.** Build twice for both Mac families; there is no universal binary. Both executables
  must be thin and must not require a macOS newer than the bundle's `LSMinimumSystemVersion` (12.0). Put `{arch}` or
  `{debarch}` in `app` and `output` if you build both, or the second build overwrites the first.
* **The worker stub must be bare.** A stub with its PHAR still appended is refused, because signing it would corrupt
  the PHAR.
* **`Info.plist` is generated** from the manifest and `--app-version` (one to three integers). Add your own keys with
  `infoPlist`. Keys the packager computes cannot be overridden.
* **Signing comes from the environment, never the configuration**: `MACOS_SIGN_IDENTITY` (a "Developer ID Application:
  …" identity name). Without it the build is signed *ad hoc*, which runs on the machine that built it but is blocked by
  Gatekeeper elsewhere. To sign nothing at all, see [Unsigned builds](#unsigned-builds). Everything is signed inside out (stub, shell, bundle) with the **hardened runtime**, so an
  ad-hoc build behaves like the final one. Each signature is then verified (`codesign --verify --strict`) and read
  back: the hardened runtime on each binary, and exactly the entitlements you configured.
* **Entitlements** are optional per binary (`entitlements.shell`, `entitlements.worker`), applied at signing and
  verified afterwards. The hardened runtime blocks things PHP may need. Grafida's worker, for example, needs only
  `com.apple.security.cs.allow-jit`, so that PCRE's JIT can allocate memory; without it PCRE falls back to the
  interpreter and logs a warning on every start. Start with none and add one only when you see a failure.
* **Notarisation** runs when `MACOS_NOTARY_PROFILE` names a `notarytool` keychain profile (it needs the identity too):
  submit, wait, staple, validate. Create the profile once with `xcrun notarytool store-credentials`.
* The disk image is laid out by Finder through AppleScript, best effort: it needs Finder's Automation permission, runs
  under a time limit, and only styles a volume mounted at exactly `/Volumes/<volumeName>`. Otherwise the image stays
  plain, with a warning. Use `--no-style` on a CI runner without Finder. The image is compressed (UDZO) and signed with
  your identity; with `MACOS_NOTARY_PROFILE` it is also notarised, stapled and checked with `spctl`.

### Windows

```jsonc
"windows": {
  "name": "MyApp",
  "icon": "assets/MyApp.ico",
  "version": {"companyName": "Example Ltd", "legalCopyright": "© Example Ltd", "productName": "MyApp",
              "shellDescription": "MyApp", "workerDescription": "MyApp worker"},
  "files": {"LICENSE.txt": "LICENSE.txt"},
  "installer": {"script": "build/windows/installer.nsi", "defines": {"SHORTCUTS": "yes"}},
  "tree": "build/shell-windows/{name}",
  "output": "build/dist/{name}-{version}-windows-{debarch}-Setup.exe"
}
```

The install tree:

```
MyApp/
  MyApp.exe              the shell, with your icon and a version resource
  myapp-worker.exe       the worker stub, likewise
  myapp-worker.phar      the worker payload
  shell.json
  MyApp.ico              windows.icon, for shortcuts and Add/Remove Programs
  …                      windows.files
```

* x86_64 only. `tree` is where the tree is written and `output` is the installer; the defaults are shown above.
* **Icon and version resources are written in PHP**, on any host. Task Manager names each process by its description:
  `version.shellDescription` (default: the name) and `version.workerDescription` (default: "<name> worker").
  `productName` defaults to the name; `version` accepts no other keys.
* **The four-part Windows version comes from `--app-version`**: major.minor.patch, then a fourth number that makes a
  pre-release sort below its release: 1000 for a release, 0–9 for alpha (or dev) N, 10–100 for beta N and 900–999 for
  rc N (`1.2.0-beta2` is `1.2.0.11`). Any other suffix is an error.
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

### Unsigned on macOS

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

### Unsigned on Windows

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
| Windows, Azure Trusted Signing | `WINDOWS_SIGN_ENDPOINT` | The account's endpoint host name, without `https://`, for example `weu.codesigning.azure.net`. |
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

The `.deb`, `.rpm` and `PKGBUILD` packages install WebKitGTK and GTK themselves, as dependencies. The tarball and the
AppImage do not: their users install them with the commands above, and the launcher's alert tells them which.

### AppImage requirements

An AppImage is not self-contained: it needs everything above, and also **FUSE**, because it mounts itself as a
read-only file system when it starts. That means kernel FUSE support and the `fusermount` helper, from the `fuse3`
package on Debian, Ubuntu, Fedora and Arch Linux. The older `libfuse2` is **not** needed.

Without FUSE (in some containers, for example), the AppImage can unpack itself into a temporary directory and run from
there instead:

```sh
./MyApp-1.0-linux-x86_64.AppImage --appimage-extract-and-run
```

To start it from a desktop menu, users can integrate it with a tool such as Gear Lever or AppImageLauncher. The
AppImage carries its desktop entry and icon for them.

## Release checklist

1. Bump your version; commit `composer.lock`.
2. Check `composer.json`'s `ext-*` requirements against the [extension list](#the-php-runtime-inside-the-package).
3. Build each package with `php -d phar.readonly=0 vendor/bin/shell-packager …`.
4. Install each package on a clean machine (or VM) of that platform. Start it, use every feature that touches the
   disk, the database, dialogs and the clipboard, then quit.
5. Publish the packages together with their SHA-256 checksums: the `.sha256` file the packager wrote beside each one.
   For the `PKGBUILD`, publish the tarballs at the URLs in `linux.pkgbuild.source` first.
