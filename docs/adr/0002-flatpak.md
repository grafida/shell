# ADR 0002 — Flatpak: not now, and never with a bundled WebKitGTK

- **Status:** Accepted.
- **Context issue:** [grafida/grafida#87](https://github.com/grafida/grafida/issues/87)
- **Facts checked on:** 2026-11-15. Every fact below carries its source. What could not be confirmed is
  listed under *Unconfirmed* and is not relied on.

## Context

Linux packaging (issue #87) asks whether Grafida should also ship as a Flatpak. Building one is out of
scope here; this record decides whether and how. Two rules were taken before it:

- WebKitGTK is never bundled outside the system or runtime copy. A bundled engine gets no distribution
  security updates, and a web engine that renders article HTML fetched from sites is the component that
  most needs them.
- If no current runtime carries the WebKitGTK **4.1** API, building it as a module is the bundling
  problem again.

The shell is tied to that API (ADR 0001, decision 1, *Known risk accepted*): tao sits on GTK 3 and the
`webkit2gtk` crate at the 4.1 API (`Cargo.toml`: `webkit2gtk` 2.0.2, feature `v2_40`).

## Findings

### Which runtimes carry WebKitGTK 4.1, and for how long

- **GNOME is the only runtime family that ships WebKitGTK at all.** The GNOME SDK has had
  `webkit2gtk-4.1` since the 4.0 deprecation
  ([GNOME Discourse](https://discourse.gnome.org/t/webkit2gtk-4-1-added-to-gnome-sdk-webkit2gtk-4-0-deprecated/7053),
  checked 2026-11-15). In `gnome-build-meta`, the branches `gnome-47`, `gnome-48`, `gnome-49`, `gnome-50`
  and `gnome-51` all contain `elements/sdk/webkit2gtk-4.1.bst` next to `webkitgtk-6.0.bst`
  ([gnome-51 tree](https://gitlab.gnome.org/api/v4/projects/GNOME%2Fgnome-build-meta/repository/tree?path=elements/sdk&per_page=200&ref=gnome-51),
  checked 2026-11-15). The 4.1 element builds with `-DUSE_GTK4=OFF`
  ([gnome-51 element](https://gitlab.gnome.org/api/v4/projects/GNOME%2Fgnome-build-meta/repository/files/elements%2Fsdk%2Fwebkit2gtk-4.1.bst/raw?ref=gnome-51),
  checked 2026-11-15).
- **GNOME 52 will not carry it.** `master` (the next release) lost the element in commit
  [`e7643120`, "Remove webkit2gtk-4.1 from flatpak SDK"](https://gitlab.gnome.org/api/v4/projects/GNOME%2Fgnome-build-meta/repository/commits/e7643120)
  (committed 2026-09-28, MR !4734, checked 2026-11-15). Its message says Boxes was the only user, drops
  "webkit2gtk-4.1 (GTK 3) from the runtime", and tells applications to "port to webkitgtk-6.0 or stay on the
  GNOME 51 runtime, which is supported until September 2027". On `master`, `elements/sdk/` holds only
  `webkitgtk.bst`, built with `-DUSE_GTK4=ON`
  ([master element](https://gitlab.gnome.org/api/v4/projects/GNOME%2Fgnome-build-meta/repository/files/elements%2Fsdk%2Fwebkitgtk.bst/raw?ref=master),
  checked 2026-11-15).
- **So the usable window is GNOME 51, to about September 2027**, on the commit message's own statement.
  GNOME runtimes are supported for about a year and are end-of-lifed when a newer stable arrives
  ([Flatpak docs](https://docs.flatpak.org/en/latest/available-runtimes.html), checked 2026-11-15); GNOME 48
  ended on 2026-03-24
  ([Ubuntu Discourse](https://discourse.ubuntu.com/t/the-gnome-48-runtime-is-no-longer-supported-as-of-march-24-2026-ubuntu-24-04-4-lts-flatpak/80598),
  checked 2026-11-15).
- **Freedesktop and KDE runtimes do not carry it.** The Flatpak docs describe the Freedesktop runtime as the
  base and KDE's as Qt-based and mention no WebKitGTK
  ([Flatpak docs](https://docs.flatpak.org/en/latest/available-runtimes.html), checked 2026-11-15); the first
  100 entries of freedesktop-sdk's `elements/components` hold no `webkit` or `gtk` element
  ([listing](https://gitlab.com/api/v4/projects/freedesktop-sdk%2Ffreedesktop-sdk/repository/tree?path=elements/components&per_page=100&ref=master&page=1),
  checked 2026-11-15). This is a partial listing, so see *Unconfirmed*.
- **Upstream still ships the 4.1 API.** The only difference from 4.0 is libsoup 3; WebKitGTK 2.52.0
  (due March 2026) was to drop libsoup 2
  ([webkitgtk.org](https://webkitgtk.org/2025/10/07/webkitgtk-soup2-deprecation.html), checked 2026-11-15).
  The latest stable is 2.54.2, announced 2026-10-23
  ([webkitgtk.org](https://webkitgtk.org/), checked 2026-11-15). No removal date for the GTK 3 port was found.
  The risk in ADR 0001 therefore now arrives from the Flatpak side, earlier than from WebKitGTK itself.
- Tauri's Flatpak guide already targets `org.gnome.Platform` `51`
  ([Tauri](https://v2.tauri.app/distribute/flatpak/), checked 2026-11-15). It does not mention 4.1 and has no
  answer for GNOME 52.

### What building it as a module would cost

Once GNOME 52 is the newest runtime, a 4.1 build is a WebKitGTK built from source in our own manifest,
which is the bundling problem the rules rule out. The cost is not the compile alone:

- We, not a distribution, become the security-update channel for the engine. A release of WebKitGTK is
  needed for each advisory (the latest were WSA-2026-0004 to -0006, 2026-07-10 to 2026-09-29;
  [webkitgtk.org/security](https://webkitgtk.org/security.html), checked 2026-11-15), on a branch that
  upstream will not carry on in the same form for GTK 3 for ever.
- GNOME's own recipe is a CMake build with clang, libsoup, `-DUSE_GTK4=OFF` and several switched-off
  features, on both x86_64 and aarch64 (sources above). Its build time and dependency closure were not
  measured here.
- A module sits on top of a runtime (GNOME 52+) that has GTK 4 and no GTK 3 WebKit, so its
  libsoup 3 and GTK 3 pairing is ours to keep consistent.

The record does not accept this. The honest alternatives are the GNOME 51 runtime until it ends, or the
GTK 4 port.

### How it interacts with WebKit's own sandbox inside Flatpak's

- The shell turns the WebKitGTK sandbox on (`WebContext::set_sandbox_enabled`, ADR 0001). Inside a Flatpak,
  nested bubblewrap cannot work, because a sandboxed app may not create namespaces
  ([Flathub Discourse](https://discourse.flathub.org/t/help-with-running-bubblewrap-in-a-flatpak/3572),
  checked 2026-11-15, user-level source).
- WebKit's own answer is to launch its child processes through the Flatpak portal instead
  (`org.freedesktop.portal.Flatpak`, `Spawn`). Web processes get the `Sandbox` and `NoNetwork` flags, D-Bus
  limited to portals, and only the paths WebKit passes as file descriptors. If the portal is older than
  version 3, or cannot expose PIDs, the process runs without a sandbox
  ([FlatpakLauncher.cpp](https://raw.githubusercontent.com/WebKit/WebKit/main/Source/WebKit/UIProcess/Launcher/glib/FlatpakLauncher.cpp),
  checked 2026-11-15).
- A silent fallback to no sandbox when `flatpak-spawn` is unusable was reported (2024-08-23); a change to
  crash instead was committed on 2024-09-09 and then reverted under bug 279446
  ([WebKit bug 278578](https://bugs.webkit.org/show_bug.cgi?id=278578), checked 2026-11-15). Whether a
  current build can still fall back silently is unconfirmed.
- Consequence for us: the sandbox setting is kept on, and a Flatpak build would have to check that the
  web process really is sub-sandboxed (the portal path) rather than trust the flag.

### Portal-only file access and the token flow

- The shell's file dialogs are rfd 0.18; its default features include `xdg-portal`
  ([docs.rs](https://docs.rs/crate/rfd/latest/features), checked 2026-11-15), and the shell's own
  `docs/user/packaging.md` already says Linux dialogs go through the XDG Desktop Portal.
- The FileChooser portal "allows sandboxed applications to ask the user for access to files outside the
  sandbox"; chosen files "will be made accessible to the application (which may involve adding it to the
  Documents portal)"
  ([FileChooser](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html),
  checked 2026-11-15). The Documents portal shows each file at `/run/user/$UID/doc/$DOC_ID/filename`
  ([Documents](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Documents.html),
  checked 2026-11-15).
- That fits the token flow. The page only ever holds an opaque per-session token; the shell resolves it to a
  path and sends it to the worker as `X-Shell-File-Path`. The worker is a child of the shell, so it runs in
  the same sandbox and sees the same document-portal path. No `--filesystem=` permission would be needed
  for user-picked files, and nothing in the page-facing contract changes.
- Not covered by this: anything the worker reads without a dialog (its data and config directories fall
  under the app's own `~/.var/app/<id>` locations, which would move existing users' data).

## Decision

1. **No Flatpak is built now.** There is no runtime that will carry WebKitGTK 4.1 after GNOME 51.
2. **Never a WebKitGTK module.** If a runtime lacks the 4.1 API, there is no Flatpak, whatever the demand.
3. **A Flatpak is possible, once, on `org.gnome.Platform//51`, until about September 2027**, with the
   portal-only file access above. It is not worth building for a window of that size alone. Revisit it only
   if the user demand justifies a build that ends by itself; the packages in issue #87 (`.deb`, `.rpm`,
   AppImage on the system WebKitGTK) are the Linux distribution path.
4. **The durable route to a Flatpak is the GTK 4 port** (webkitgtk-6.0, which GNOME 52 and later ship). That
   is a windowing rewrite that ADR 0001 already named as the cost of losing the 4.1 API. Take it on its
   own merits, not for Flatpak alone.
5. Re-check this record when GNOME 52 is released, and before any Flatpak work starts: the runtime lists, the
   `gnome-build-meta` tree and WebKitGTK's GTK 3 status.

## Unconfirmed

- The end-of-life dates of GNOME 49 and 50 (the 2027-09 date for GNOME 51 is the commit message's, not a
  runtime metadata value), and the GNOME 52 release date (expected spring 2027 by the usual cadence).
- That freedesktop-sdk and the KDE runtime carry no WebKitGTK: only a partial listing was read.
- Whether the shell runs correctly in a Flatpak at all (nothing was built or run), including GTK 3 under
  Wayland and rfd's behaviour with the portal there.
- The WebKitGTK version in the GNOME 51 runtime, and the build time and dependency closure of the 4.1
  module.
- Whether WebKit still falls back to no sandbox silently inside Flatpak.
- Flathub's policy on submitting an app that depends on an end-of-life runtime.
