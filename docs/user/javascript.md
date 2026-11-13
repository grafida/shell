# The JavaScript side

Your page is ordinary web content running in the system webview (WebView2 on Windows, WKWebView on macOS, WebKitGTK on
Linux). It talks to your PHP worker with `fetch()`, XHR or form posts against its own origin, `<scheme>://app`. That
needs nothing special.

What a webview cannot do is reach the desktop: pick a file, show a save dialog, read the clipboard, open a link in the
user's browser. For that the shell answers a small set of reserved URLs under `/_native/`, and the
`shell.js` helper from [grafida/shell-php](https://github.com/grafida/shell-php) wraps them in a `GrafidaShell` object.

## Loading `shell.js`

Serve the file from your own origin and load it before your code (see
[Serving `shell.js`](php-worker.md#serving-shelljs)):

```html
<script src="/shell.js"></script>
```

It is a classic script, not a module, and defines the global `GrafidaShell` (frozen). Every call returns a promise.

## Are we inside the shell?

```js
if (await GrafidaShell.available()) {
    // running in Grafida Shell
}
```

`available()` asks the shell once per page, caches the answer, and never rejects. If you also serve your app in a
browser, or from another host, use it to pick a web fallback. If your app only ever runs in the shell, you can skip it.

`await GrafidaShell.info()` returns `{shell, protocol, platform, arch, webview}`: the shell's version, the protocol
version, `"linux"` / `"macos"` / `"windows"`, the CPU architecture, and the webview's version.

## Handling errors

Two error classes, which mean different things:

| Class | Meaning | What to do |
|---|---|---|
| `GrafidaShell.ShellUnavailableError` | The page is **not** hosted by the shell (a plain browser, another host) | Use your web fallback, or tell the user the feature needs the desktop app |
| `GrafidaShell.ShellError` | The shell is there and **refused** the call. `error.code` says why; `error.status` is the HTTP status; `error.detail` may carry text | Show a message. Do not fall back to the web version. |

```js
try {
    await GrafidaShell.openUrl(url);
} catch (e) {
    if (e instanceof GrafidaShell.ShellError) {
        console.warn('Refused:', e.code);      // e.g. "unsupported_url"
    } else {
        // ShellUnavailableError: not in the shell
    }
}
```

Codes you can meet: `bad_request` (400, malformed arguments), `not_found` (404), `method_not_allowed` (405),
`dialog_busy` (409, a dialog is already open: only one at a time), `unsupported_url` (400), `unknown_file_token` and
`stale_file_token` (400), `open_failed` and `reveal_failed` (500, the operating system could not do it; `detail` has the
reason).

## Files are tokens, not paths

The page runs in a sandbox and must not learn real file paths. A file dialog therefore returns **tokens**: opaque
strings, valid for the current run of the shell and tied to the file the user picked. You pass the token to your PHP code,
and the shell turns it back into a path on the way.

```js
const result = await GrafidaShell.openFile({
    filters: [{name: 'Images', extensions: ['png', 'jpg']}],
    multiple: false,
});

if (!result.cancelled) {
    const file = result.files[0];             // {token, name, size}

    await fetch('/api/import', {
        method: 'POST',
        headers: GrafidaShell.fileHeaders(file.token),
    });
}
```

`fileHeaders(token)` is `{"X-Shell-File": token}`. Before the request reaches the worker the shell:

1. **drops any `X-Shell-File-Path` header the page sent**, so a script cannot forge a path;
2. looks the `X-Shell-File` token up and, if it is good, adds `X-Shell-File-Path: /the/real/path`;
3. removes the token header itself.

If the token is unknown or stale, the shell answers `400 unknown_file_token` or `400 stale_file_token` itself and your
worker never sees the request. Your PHP code reads the path from the header and does the real work:

```php
$path = $request->header('X-Shell-File-Path');   // null when no token was sent
```

A script injected into the page can therefore only use files the user chose in this session. Treat `X-Shell-File-Path`
as the only trustworthy source of a path, and never accept a path from the request body.

## The calls

### Dialogs

| Call | Returns |
|---|---|
| `openFile({filters, multiple})` | `{cancelled: true}` or `{files: [{token, name, size}, …]}` |
| `selectFolder()` | `{cancelled: true}` or `{folder: {token, name}}` |
| `saveFile({suggestedName, filters})` | `{cancelled: true}` or `{file: {token, name}}` |

`filters` is a list of `{name: 'Images', extensions: ['png', 'jpg']}`; a leading dot in an extension is ignored. Leave it
out for no filter. The dialog is the operating system's own.

`saveFile` only chooses a destination. It does not create or write the file (the file may not exist yet). Send the
token to your worker, which writes to the path it receives in `X-Shell-File-Path`:

```js
const target = await GrafidaShell.saveFile({suggestedName: 'report.csv'});

if (!target.cancelled) {
    await fetch('/api/export', {method: 'POST', headers: GrafidaShell.fileHeaders(target.file.token)});
}
```

On Linux, dialogs go through the XDG Desktop Portal (falling back to `zenity`); see the
[system requirements](packaging.md#system-requirements).

### Dropped files

Files dropped on the window are taken by the shell (the web process cannot read them). Register a handler:

```js
const stop = GrafidaShell.onDrop(({files, x, y}) => {
    for (const f of files) {
        // f is {token, name, size}; x, y are viewport coordinates of the drop
        fetch('/api/import', {method: 'POST', headers: GrafidaShell.fileHeaders(f.token)});
    }
});
// later: stop();
```

Internally this is a `shell:drop` event on `window` whose `detail` is `{files, x, y}`. It never fires outside the shell.

### Clipboard

```js
const response = await fetch('/api/paste', {method: 'POST', headers: GrafidaShell.clipboardHeaders()});
```

`clipboardHeaders()` is `{"X-Shell-Clipboard": "read"}`. The shell replaces the request **body** with the clipboard's
text (empty if it holds none), replaces the header with `X-Shell-Clipboard: text`, and forwards it. Read the text from
`$request->body`. There is no prompt and no helper tool.

### Opening and revealing

| Call | What it does |
|---|---|
| `openUrl(url)` | Opens an `http://` or `https://` URL in the user's default browser. Any other scheme, or a URL containing whitespace, is refused with `unsupported_url`. |
| `reveal(token)` | Shows a picked file or folder in the system file manager. |

### The window

| Call | What it does |
|---|---|
| `setWindowTitle(title)` | Sets the title bar text. |
| `setWindowTheme('light' \| 'dark' \| 'system')` | Sets the title bar and window decoration theme. The page's own colours are your CSS. |
| `quit()` | Closes the window. The worker sees end of input and exits. |

The initial title and theme come from the [manifest](manifest.md).

## Links and navigation

The page may navigate within `<scheme>://app`. If the user clicks a link, submits a form or calls `window.open` to
somewhere else:

* an **`http`/`https`** destination is cancelled and opened in the user's browser;
* anything else (`file:`, `mailto:`, another scheme) is refused.

Embedded `<iframe>`s load normally, but a request carrying a foreign `Origin` header, or `Sec-Fetch-Site: cross-site`,
is answered `403 cross_origin` by the shell. Not every webview sends `Sec-Fetch-Site` (WKWebView on macOS does not), and
browsers send no `Origin` on a plain GET, so keep your GET handlers side-effect free: change state with `POST` plus a
custom header, which forces a CORS preflight that the shell refuses from a foreign page. On macOS the shell also
refuses a foreign frame's navigation to `<scheme>://app`.

Do your outbound HTTP (calling a remote service) from PHP, where you control the client and the credentials, not from
page JavaScript.

## Keyboard shortcuts and menus

On Linux and Windows the shell has no menu bar, and on macOS it has the standard application, Edit and Window menus
(`menu.preset`, see the [manifest](manifest.md#menu)). Menu accelerators fire **before** the webview sees a key. If your
page implements a shortcut itself, list its chord in `menu.reservedChords` so the shell never binds it.

> [!NOTE]
> **Custom application menus are not supported.** You cannot add, change or remove menu items, and a menu click cannot
> call into your page or your PHP code. If your app needs a menu, build it in HTML and CSS inside the page.

## Raw endpoints

You do not need the helper to call the native endpoints. Each is `<scheme>://app/_native/<endpoint>`, JSON in and out,
never cached, and does not reach your worker:

| Method | Endpoint | Body | Success |
|---|---|---|---|
| GET | `info` | | `{shell, protocol, platform, arch, webview}` |
| POST | `dialog/open-file` | `{filters?, multiple?}` | `{cancelled:true}` or `{files:[…]}` |
| POST | `dialog/select-folder` | | `{cancelled:true}` or `{folder:{…}}` |
| POST | `dialog/save-file` | `{suggestedName?, filters?}` | `{cancelled:true}` or `{file:{…}}` |
| POST | `open-url` | `{url}` | `{"ok":true}` |
| POST | `reveal` | `{token}` | `{"ok":true}` |
| POST | `window/theme` | `{theme}` | `{"ok":true}` |
| POST | `window/title` | `{title}` | `{"ok":true}` |
| POST | `quit` | | `{"ok":true}` |

Errors are `{"error": "<code>"}` with the HTTP status noted above. An unknown endpoint is `404 not_found`; the wrong
method is `405 method_not_allowed`. The path `/_native/` is reserved: do not use it for routes of your own.
