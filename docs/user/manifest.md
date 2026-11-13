# The `shell.json` manifest

`shell.json` is everything the shell needs to know about your app. The shell contains nothing specific to any
application: name, window, scheme, worker command and behaviour all come from here.

* **The contract is the JSON Schema**, [`schema/shell.schema.json`](../../schema/shell.schema.json), compiled into the shell.
  At start-up the shell validates the manifest against it and **refuses to run on any error**, naming the offending member.
* **Every object rejects unknown members.** A typo is an error, never a silently ignored setting.
* Add the schema to your editor for completion and validation:

  ```json
  {"$schema": "https://github.com/grafida/shell/raw/main/schema/shell.schema.json"}
  ```

  (`$schema` is the one member allowed beyond those documented below.)

## Where the shell looks for it

| Mode | Manifest | Worker | Worker's working directory |
|---|---|---|---|
| `grafida-shell --dev [APP_DIR]` | `APP_DIR/shell.json` (default: the current directory) | `dev.worker` | `APP_DIR` |
| `--manifest PATH` | That file | As above | As above |
| Packaged | `shell.json` beside the shell binary, or in `../Resources` (macOS bundle) | `worker.binary`, beside the shell binary | The manifest's directory |

In packaged mode paths are resolved from the shell's *real* path (symlinks followed), so a symlink to the executable
works.

## A complete example

Every setting at its default is omitted here except where shown for illustration:

```json
{
    "$schema": "https://github.com/grafida/shell/raw/main/schema/shell.schema.json",
    "id": "com.example.myapp",
    "title": "My App",
    "scheme": "myapp",
    "dataDir": "myapp",
    "startPath": "/",
    "noStorePaths": ["/api/"],
    "window": {"width": 1280, "height": 860, "minWidth": 900, "minHeight": 600, "theme": "system"},
    "worker": {
        "binary": "myapp-worker",
        "args": [],
        "readyTimeoutMs": 15000,
        "maxBodyBytes": 134217728,
        "restart": {"max": 3, "windowSeconds": 60}
    },
    "dev": {"worker": ["php", "bin/worker.php"]},
    "webview": {"contextMenu": true, "devtools": "env:MYAPP_DEBUG", "spellCheck": true},
    "menu": {"preset": "standard", "reservedChords": ["CmdOrCtrl+Shift+V"]},
    "logDir": "auto"
}
```

## Top level

| Member | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `id` | string | **yes** | | Reverse-DNS application id, for example `com.example.myapp`: letters, digits and hyphens in at least two dot-separated parts, up to 255 characters. It is the macOS bundle id, the Windows AppUserModelID, and it names the log and runtime files. The packager also uses it for the Linux desktop entry and icons. **Do not change it after release.** |
| `title` | string | **yes** | | The window title until the page sets another with `setWindowTitle()`. Also used in the shell's alert dialogs. 1–255 characters. |
| `scheme` | string | **yes** | | The custom URL scheme your app is served on. Lower case, starts with a letter, then letters, digits, `+`, `.` or `-`; up to 64 characters. The page's origin is `<scheme>://app` on every platform, byte for byte. The names `about`, `blob`, `data`, `file`, `ftp`, `http`, `https`, `javascript`, `mailto`, `resource`, `view-source`, `ws` and `wss` are not allowed. **Do not change it after release**: stored documents and browser storage embed the origin. |
| `window` | object | **yes** | | See [`window`](#window). |
| `worker` | object | **yes** | | See [`worker`](#worker). |
| `dataDir` | string | no | the `id` | Name of the per-user directory in which the webview keeps its data (cookies, localStorage, IndexedDB, caches): on Linux `$XDG_DATA_HOME/<dataDir>/webkit`. Letters, digits, `.`, `_` and `-`. Choose it once: changing it makes the user's stored page data disappear. |
| `startPath` | string | no | `/` | The path of the first page, loaded once the worker has sent `ready`. Must start with `/`. |
| `noStorePaths` | array of strings | no | `[]` | Path prefixes (each starting with `/`, for example `"/api/"`). A response under one of them gets `Cache-Control: no-store` **if the worker sent neither `Cache-Control` nor `Expires`**. A header you set yourself always wins. |
| `dev` | object | no | | See [`dev`](#dev). |
| `webview` | object | no | | See [`webview`](#webview). |
| `menu` | object | no | | See [`menu`](#menu). |
| `logDir` | string | no | `"auto"` | Where the log goes: `"auto"` for the platform's log directory, or an absolute path. See [Troubleshooting](troubleshooting.md#logs). |

## `window`

Sizes are logical pixels, integers from 1 to 32767.

| Member | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `width` | integer | **yes** | | Initial width |
| `height` | integer | **yes** | | Initial height |
| `minWidth` | integer | no | | The user cannot resize below this |
| `minHeight` | integer | no | | |
| `theme` | `"light"`, `"dark"` or `"system"` | no | `"system"` | The window decoration (title bar) theme at start. The page can change it with `setWindowTheme()`. |

## `worker`

| Member | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `binary` | string | **yes** | | The worker executable's name. In a package the shell looks for it **beside the shell executable** (`Contents/MacOS` in a macOS bundle). On Windows `.exe` is added if the name has no extension. It is not used in `--dev`, but it names the packaged worker, so choose it now. If the file is missing the shell shows an alert. |
| `args` | array of strings | no | `[]` | Arguments passed to the packaged worker. |
| `readyTimeoutMs` | integer | no | `15000` | How long the worker has to send its `ready` frame. 1000–600000. A worker that is not ready in time is a fatal start-up failure. |
| `maxBodyBytes` | integer | no | `134217728` (128 MiB) | The largest body, per direction. 1048576 to 2^53 − 1. The shell passes it to the worker as the `SHELL_MAX_BODY_BYTES` environment variable, which `Worker::run()` honours. A larger request is answered `413 body_too_large` by the shell. |
| `restart.max` | integer | no | `3` | Worker restarts allowed within `restart.windowSeconds`. 0–100. One crash more is fatal: the shell shows an alert with the tail of the worker's stderr and quits. A failure *before the first `ready`* is always fatal. |
| `restart.windowSeconds` | integer | no | `60` | 1–86400. |

## `dev`

Only used with `--dev`.

| Member | Type | Meaning |
|---|---|---|
| `worker` | array of strings (at least one) | The worker command, run with the app directory as its working directory, for example `["php", "bin/worker.php"]`. Required for `--dev`. |

## `webview`

| Member | Type | Default | Meaning |
|---|---|---|---|
| `contextMenu` | boolean | `true` | Whether the webview's native right-click menu is available. It carries the spelling suggestions, so turn it off only if you provide your own. |
| `devtools` | boolean or `"env:NAME"` | `false` | The web inspector. `true` always, `false` never, `"env:NAME"` only when the environment variable `NAME` is set to something non-empty other than `0`. The `env:` form lets you keep it off for users and turn it on for yourself without rebuilding. |
| `spellCheck` | boolean | `true` | Turns on the webview's spell checker. Your page still decides per element with the `spellcheck` attribute. |
| `frameGuard` | boolean | `true` | Only your app may show its pages in a frame: a foreign page your app embeds (an article's `<iframe>`) cannot frame them to overlay them. `false` lets any page frame your pages and, on macOS, lets a foreign frame load and request your app's URLs. Turn it off only when your app has controls of its own. See [The PHP worker](php-worker.md#who-may-frame-your-pages). |
| `dmabufRenderer` | `"auto"`, `"on"` or `"off"` | `"auto"` | **Linux only.** WebKitGTK's DMA-BUF renderer. `"auto"` turns it off when NVIDIA's proprietary driver is loaded, where it can leave the window blank. The user's own `WEBKIT_DISABLE_DMABUF_RENDERER` environment variable always wins. |

## `menu`

| Member | Type | Default | Meaning |
|---|---|---|---|
| `preset` | `"standard"` or `"none"` | `"standard"` | `"standard"`: on macOS the application, Edit and Window menus; on Linux and Windows no menu bar. `"none"`: no menus anywhere. |
| `reservedChords` | array of strings | `[]` | Key chords your page owns. Menu accelerators fire *before* the webview sees a key, so the shell never binds these. Format: zero or more modifiers from `CmdOrCtrl`, `Cmd`, `Ctrl`, `Alt`, `Shift`, `Super`, then a key, joined by `+`: `"CmdOrCtrl+Shift+V"`. No duplicates. |

> [!NOTE]
> **Custom application menus are not supported.** The only choice is between the standard menus and none. To give your
> users a menu, build it in the page.

## Rules of thumb

* **Decide `id`, `scheme` and `dataDir` before the first release.** They are the app's identity to the operating system
  and to the webview's storage.
* The manifest ships inside the package, as a plain file the user can read (and edit).
  Never put secrets in it.
* The packager reads the same file: `id`, `title` and `worker.binary` name the package contents, so the shell and the
  package cannot disagree. See [Packaging](packaging.md).
* Do not add your own members. To configure your app, use your own config file.
