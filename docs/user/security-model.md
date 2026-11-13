# Security model

This page says what the shell trusts, what it protects against, and what it leaves to you. Read it before you put
foreign content (an embedded video, an article's `<iframe>`) inside your app, or handle anything sensitive in it.

## Who is trusted

| Who | Trusted? | What the shell does about it |
|---|---|---|
| **You, the app's developer**: the manifest, the worker, the page | Yes | Nothing. The worker runs with the user's own rights and can do anything the user can. A user who installs your app trusts *you*, so the shell does not defend the user against your code. |
| **Your page**: scripts running at `<scheme>://app` | Yes | It may call every native endpoint and every route of your worker. |
| **Foreign content inside your page**: an `<iframe>` showing another site | No | It cannot call `/_native/*`, drive your worker's API, show your pages inside its own frame, or open the browser or a window without a click. See [Links and navigation](javascript.md#links-and-navigation). |
| **Other user accounts on the same computer** | No | The webview's data and the log live in per-user directories, and on Linux and macOS the log is readable by its owner only. |
| **The user's administrator, or other software running as the user** | Out of scope | It can read the user's files, attach a debugger or change the webview's settings. No desktop app can defend against that. |

The worker and the page are both your code, so the line the shell defends runs between them and *everything else*:
mainly foreign content embedded in the page.

## What this means for your code

* **A cross-site scripting (XSS) hole in your page gives the attacker everything your page can do.** That includes
  reading the clipboard at any moment without a click or a prompt (see [Clipboard](javascript.md#clipboard)),
  opening file dialogs, and calling every route of your worker. Escape whatever you put into HTML, especially content
  that came from the network or from a document the user opened.
* **Keep `GET` handlers free of side effects.** Change state with `POST` and a custom header. Not every webview tells
  the shell which frame sent a request, and a `GET` from a foreign frame may carry no `Origin`.
* **Never take a file path from the page.** Files reach your worker as tokens that the shell resolves itself (see
  [Files are tokens, not paths](javascript.md#files-are-tokens-not-paths)).
* **Your worker's stderr goes to the log.** Do not write passwords or keys there. The log is readable by the user's
  account only (Linux and macOS), but users send logs with bug reports.
* **A refused navigation may still have reached the network.** On Linux, a script-driven navigation of the window to
  a website is stopped after the request has gone out. See [Links and navigation](javascript.md#links-and-navigation).
  If you need to limit what your page may contact, send a `Content-Security-Policy` from your worker.

## The shell's releases

* **Ship only the release assets** (`grafida-shell-*` on the release page, with their `.sha256`), pinned by version and
  by a SHA-256 you computed yourself (see [Packaging](packaging.md#finding-the-urls-and-hashes-to-pin)).
* **Never ship a `debug-automation-*` build.** The shell's CI produces these debug builds for its own tests. They carry
  a remote-control channel: it is off unless the shell is started with `--automation`, it listens on `127.0.0.1` only,
  and every command needs a random token from a file only the user can read. They are never attached to a release,
  and nothing here tells you to download one.
* The release assets are built by GitHub Actions, on GitHub's own runners, from a tag that only the maintainer can push.
  Trusting them means trusting GitHub's build infrastructure and the crates the shell depends on. If that is not
  enough for you, build the shell yourself from the tag.
