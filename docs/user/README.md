# Building a desktop application with Grafida Shell

Grafida Shell lets you write a desktop application in **PHP and web technologies**. You write a web
application (HTML, CSS, JavaScript) and a PHP back-end that answers its requests; the shell puts the one in a native
window and runs the other as a child process. You ship one installable package per platform, with no PHP, web server
or browser required on the user's machine.

This documentation is for a PHP developer who has never used the shell. It explains how the pieces fit together, how to
start a project, and how to package it.

## The pieces

| Piece | What it is | You use it to… |
|---|---|---|
| [**grafida/shell**](https://github.com/grafida/shell) | A prebuilt native program (Rust) that opens a window with the operating system's own webview and supervises your PHP code | Run your app. You download it; you do not build it. |
| [**grafida/shell-php**](https://github.com/grafida/shell-php) | A Composer library: the PHP side of the shell | Turn your PHP code into a *worker* the shell can talk to, and give your page a JavaScript helper (`shell.js`) for native features |
| [**grafida/shell-packager**](https://github.com/grafida/shell-packager) | A Composer development tool | Build, sign and package your app for Linux, macOS and Windows |
| [**grafida/phpmicro**](https://github.com/grafida/phpmicro) | A self-contained PHP 8.4 runtime, built for the shell | Nothing directly. The packager bundles it, so your users need no PHP installation. |

Your application is two halves that never share memory:

* **The page**: your HTML, CSS and JavaScript, shown in the webview at `<scheme>://app` (you choose the scheme, for
  example `myapp://app`).
* **The worker**: a PHP CLI process that receives every request the page makes, as if it were a tiny web server, and
  answers it.

Anything that is not a request to your own application, such as opening a file dialog, is a request to a reserved
path (`/_native/*`) that the shell answers itself. The [architecture](architecture.md) page has the diagrams.

## What you should know before you start

* **Your PHP runs as a long-lived CLI process, not under a web server.** There are no `$_GET`, `$_SERVER` or
  sessions. You receive a `Request` object and return a `Response` object. State in static properties or memory
  survives between requests, because the process stays alive for as long as the window is open.
* **The worker handles one request at a time.** A slow handler delays every other request to your app (native
  requests excepted).
* **The packaged PHP is a fixed build** (PHP 8.4, a specific list of extensions; see
  [Packaging](packaging.md#the-php-runtime-inside-the-package)). In development you run your system's PHP. Test the
  packaged build before you release.
* **The origin never changes.** Your page's origin is `<scheme>://app`. Browser storage (localStorage, IndexedDB,
  cookies) is tied to it and to the manifest's `id` and `dataDir`. Choose them once.

## Reading order

1. [Getting started](getting-started.md): build and run a minimal app in development mode.
2. [Architecture](architecture.md): how the shell, the webview and the worker fit together (diagrams).
3. [The PHP side](php-worker.md): the worker, requests and responses, static files, testing.
4. [The JavaScript side](javascript.md): `shell.js` and the native endpoints (dialogs, clipboard, drag and drop,
   window control).
5. [The `shell.json` manifest](manifest.md): every setting, with defaults.
6. [Security model](security-model.md): what the shell trusts, what it protects against, and what it leaves to you.
7. [Packaging](packaging.md): build installable packages and sign them.
8. [Getting signing certificates](code-signing.md): Apple Developer ID and notarisation, Azure Artifact Signing.
9. [Troubleshooting](troubleshooting.md): logs, alerts, and what the error codes mean.

Reference material for people who want to go deeper: the [wire protocol v1](../../protocol/v1.md) between the shell and
the worker, and the [architecture decision record](../adr/0001-shell-stack.md) explaining why the shell is built the
way it is.

## Who uses it

Grafida Shell was written for [Akeeba Grafida](https://github.com/grafida/grafida), a real application built on it.
Its `shell.json`, `shell-package.json` and `bin/worker.php` are a good complete example.

## Licence

The shell, shell-php and shell-packager are MIT-licensed. phpmicro is Apache-2.0.
