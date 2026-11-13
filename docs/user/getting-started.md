# Getting started

This page builds a minimal application and runs it in **development mode**: the shell opens a window and starts your
PHP worker straight from your source tree, with your system's PHP. No packaging is involved yet.

## What you need

* **PHP 8.4 or later** on your `PATH`, and [Composer](https://getcomposer.org).
* **The shell binary** for your platform and CPU, from the
  [grafida/shell releases](https://github.com/grafida/shell/releases):

  | Platform | Release asset |
  |---|---|
  | Linux | `grafida-shell-linux-x86_64`, `grafida-shell-linux-aarch64` |
  | macOS | `grafida-shell-macos-arm64`, `grafida-shell-macos-x86_64` |
  | Windows | `grafida-shell-windows-x86_64.exe` |

  Every asset has a `.sha256` file beside it. On Linux and macOS make it executable (`chmod +x`). If macOS refuses to
  open a binary you downloaded with a browser, remove the quarantine flag with
  `xattr -d com.apple.quarantine grafida-shell-macos-arm64`.
* **The webview** the shell uses: WebView2 on Windows (normally already there), WKWebView on macOS (part of the
  system), and GTK 3 with WebKitGTK 4.1 on Linux. See the
  [system requirements](packaging.md#system-requirements).

## 1. Create the project

```
myapp/
├── composer.json
├── shell.json            the manifest: tells the shell about your app
├── bin/worker.php        the worker's entry point
├── src/App.php           your request handler
└── public/index.html     your page
```

**`composer.json`**: `grafida/shell-php` is not on Packagist, so add its repository:

```json
{
    "name": "example/myapp",
    "type": "project",
    "repositories": [
        {"type": "vcs", "url": "https://github.com/grafida/shell-php"}
    ],
    "require": {
        "php": "^8.4",
        "grafida/shell-php": "^0.2"
    },
    "autoload": {
        "psr-4": {"Example\\MyApp\\": "src"}
    }
}
```

Run `composer install`.

**`shell.json`**: the manifest. Only `id`, `title`, `scheme`, `window` and `worker` are required. The `dev.worker`
command is what `--dev` runs. The full list is in the [manifest reference](manifest.md).

```json
{
    "$schema": "https://github.com/grafida/shell/raw/main/schema/shell.schema.json",
    "id": "com.example.myapp",
    "title": "My App",
    "scheme": "myapp",
    "window": {"width": 1024, "height": 720},
    "worker": {"binary": "myapp-worker"},
    "dev": {"worker": ["php", "bin/worker.php"]}
}
```

The `$schema` line gives you completion and validation in editors that understand JSON Schema. `worker.binary` is the
name the worker will have *once packaged*; in development it is not used.

**`bin/worker.php`**: the entry point. `Worker::run()` never returns: it announces itself to the shell, then answers
requests until the shell closes the pipe.

```php
<?php

declare(strict_types=1);

use Example\MyApp\App;
use Grafida\Shell\{StaticFiles, Worker, WorkerOptions};

require dirname(__DIR__) . '/vendor/autoload.php';

Worker::run(
    new App(new StaticFiles(dirname(__DIR__) . '/public')),
    new WorkerOptions(app: 'myapp', version: '1.0.0')
);
```

**`src/App.php`**: the handler. It receives a `Request` and returns a `Response`; this one serves `shell.js`, a tiny
JSON API, and the files in `public/`.

```php
<?php

declare(strict_types=1);

namespace Example\MyApp;

use Grafida\Shell\{Handler, Request, Response, ShellScript, StaticFiles};

final class App implements Handler
{
    public function __construct(private readonly StaticFiles $assets)
    {
    }

    public function __invoke(Request $request): Response
    {
        $path = $request->path();

        if ($path === '/shell.js') {
            return ShellScript::response();
        }

        if (str_starts_with($path, '/api/')) {
            return $this->api($request);
        }

        // A file from public/, or the single-page app's entry point.
        return $this->assets->find($request)
            ?? Response::file(__DIR__ . '/../public/index.html');
    }

    private function api(Request $request): Response
    {
        return match ($request->path()) {
            '/api/hello' => Response::json(['message' => 'Hello, ' . ($request->query('name') ?? 'world') . '!']),
            default      => Response::json(['error' => 'not_found'], 404),
        };
    }
}
```

**`public/index.html`**: the page. It loads `/shell.js` (served by the route above) to reach native features.

```html
<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>My App</title>
    <script src="/shell.js"></script>
</head>
<body>
    <h1>My App</h1>
    <button id="hello">Say hello</button>
    <button id="open">Open a file…</button>
    <pre id="out"></pre>
    <script>
        const out = document.getElementById('out');

        document.getElementById('hello').onclick = async () => {
            const r = await fetch('/api/hello?name=desktop');
            out.textContent = (await r.json()).message;
        };

        document.getElementById('open').onclick = async () => {
            if (!(await GrafidaShell.available())) {
                out.textContent = 'Not running inside the shell.';
                return;
            }
            const result = await GrafidaShell.openFile();
            out.textContent = result.cancelled ? 'Cancelled.' : 'Picked ' + result.files[0].name;
        };
    </script>
</body>
</html>
```

> [!IMPORTANT]
> `shell.js` lives in `vendor/`, outside your own code. When you [package](packaging.md#phar-the-worker-payload) the
> app, add `{"directory": "vendor/grafida/shell-php/resources"}` to `phar.include` in `shell-package.json`, or the
> packaged app cannot serve it. In `--dev` it just works, so this is easy to miss.

The page uses plain relative URLs such as `/api/hello`. They resolve against `myapp://app`, so they reach your PHP
code. There is no `http://` involved anywhere.

## 2. Check the worker without the shell

The worker is an ordinary PHP program that speaks a binary protocol on stdin/stdout, so you can test it without a window.
`Grafida\Shell\Testing\WorkerProcess` plays the shell's part:

```php
use Grafida\Shell\Request;
use Grafida\Shell\Testing\WorkerProcess;

$worker   = new WorkerProcess([PHP_BINARY, 'bin/worker.php'], __DIR__);
$response = $worker->request(new Request('GET', 'myapp://app/api/hello?name=Ann'));

echo $response->status, ' ', $response->body;   // 200 {"message":"Hello, Ann!"}

$worker->closeInput();                           // what the shell does when the window closes
echo $worker->wait();                            // 0
```

Never run `php bin/worker.php` in a terminal and expect to type at it. It waits for binary frames.
See [testing](php-worker.md#testing-your-worker).

## 3. Run it

From the project directory:

```sh
/path/to/grafida-shell --dev .
```

`--dev [APP_DIR]` reads `APP_DIR/shell.json` (default: the current directory), starts `dev.worker` with `APP_DIR` as its
working directory, and opens the window. Click the buttons. Close the window and the worker is told to exit.

Things to know in development mode:

* **Edit PHP, then restart the shell.** The worker is a long-lived process; it does not reload code. Static
  files served by `StaticFiles` are read from disk on every request, so changes to `public/` show on reload.
* **Anything your PHP prints goes to the log**, not to the page. See [Troubleshooting](troubleshooting.md).
* **To inspect the page** (the web inspector), set `webview.devtools` in the manifest. `"env:MYAPP_DEBUG"` enables it
  when that environment variable is set.
* If `dev.worker` is missing from the manifest, `--dev` stops with an error.

## Where to go next

* Understand the request cycle and what you can do in the handler: [The PHP side](php-worker.md).
* Open files, save files, use the clipboard: [The JavaScript side](javascript.md).
* Window size, menus, theme, the data directory: [the manifest](manifest.md).
* Ship it: [Packaging](packaging.md).
