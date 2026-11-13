# The PHP side

Your PHP code runs as the **worker**: a CLI process, started and supervised by the shell, that answers the page's
requests. The library [grafida/shell-php](https://github.com/grafida/shell-php) provides everything between the
shell's wire protocol and your code. You write a handler; the library does the rest.

```
composer require grafida/shell-php
```

(The package is not on Packagist. Add `{"type": "vcs", "url": "https://github.com/grafida/shell-php"}` to the
`repositories` section of your `composer.json`.)

## The life of a worker

1. The shell starts the worker: `dev.worker` in development, the packaged `worker.binary` in production.
2. `Worker::run()` sends a `ready` frame. Until the shell has seen it, the window does not load your first page. If
   the worker does not send it within `worker.readyTimeoutMs` (15 s by default), or exits first, the shell shows an
   error alert and quits. A worker that cannot start once is not retried.
3. The shell sends one request frame at a time; the worker answers each with a response frame. **Requests are handled
   strictly one at a time, in order.**
4. When the user closes the window, the shell closes the worker's stdin. `Worker::run()` sees the end of input and
   exits with status 0. A `shutdown` frame has the same effect.
5. If the worker dies mid-session, the page's in-flight request gets `502 {"error":"worker_restarted"}` and the shell
   starts a new worker. More than `worker.restart.max` crashes (3) within `worker.restart.windowSeconds` (60) is fatal
   and shows the user the last 40 lines of the worker's standard error.

Because the worker is long-lived, objects you create once at start-up (a database connection, a container) persist
across requests. Code is loaded once; to see PHP changes in development, restart the shell.

## The entry point

```php
use Grafida\Shell\{Handler, Worker, WorkerOptions};

require dirname(__DIR__) . '/vendor/autoload.php';

Worker::run($handler, new WorkerOptions(app: 'myapp', version: '1.0.0'));
```

`Worker::run(Handler $handler, WorkerOptions $options = new WorkerOptions()): never` is the last thing your entry point
does. It never returns; it calls `exit()` with the worker's exit status (0 normally; 65 after a protocol error; 74 if
standard output cannot be written, i.e. the shell has gone).

`WorkerOptions` holds `app` and `version` (names announced in `ready`, shown in the shell's log) and `maxBodyBytes`.
You should leave `maxBodyBytes` alone: the shell tells the worker its limit through the `SHELL_MAX_BODY_BYTES`
environment variable, set from `worker.maxBodyBytes` in the manifest (128 MiB by default), and that value wins.

Do your start-up work (building a container, opening a database) **before** `Worker::run()`. If it fails, write a
clear sentence to standard error and `exit(1)` (or any non-zero status) without sending `ready`: the shell shows the end
of standard error in its failure alert, so the user sees your message.

## Handlers, requests and responses

A handler is anything implementing `Grafida\Shell\Handler`:

```php
interface Handler
{
    public function __invoke(Request $request): Response;
}
```

### `Request`

Immutable. Properties: `method` (case preserved), `url` (the absolute URL exactly as sent, for example
`myapp://app/api/sites?x=1`), `headers`, `body` (raw string).

| Method | Returns |
|---|---|
| `path()` | The URL's path as sent, still percent-encoded; `/` if empty |
| `queryString()` | The raw query string, without `?` |
| `queryParams()` | All query parameters, decoded as `parse_str()` does (`a[]=1` gives an array) |
| `query($name, $default = null)` | One query parameter as a string |
| `header($name)` | The first value of a request header, or `null` (names are case-insensitive) |

The request body is never parsed for you. For a JSON API, decode it yourself:

```php
$data = json_decode($request->body, true, flags: JSON_THROW_ON_ERROR);
```

There is no `$_POST`, `$_FILES` or `$_SESSION`. A `multipart/form-data` body arrives as a raw string; if you need file
uploads, prefer the shell's file dialogs (see [the JavaScript side](javascript.md#files-are-tokens-not-paths)), which give
you real paths.

### `Response`

Immutable. Build one with a constructor or a factory, adjust with the `with*()` methods:

| | |
|---|---|
| `new Response($body = '', $status = 200, $headers = [])` | Status must be 100–599 |
| `Response::json($data, $status = 200, $headers = [])` | `application/json; charset=utf-8`, slashes and Unicode unescaped |
| `Response::html($html, …)` / `Response::text($text, …)` | The matching `Content-Type` |
| `Response::file($path, $contentType = null, $headers = [])` | A file's contents; type from the extension. Works for `phar://` paths. Throws `RuntimeException` if unreadable. |
| `->withHeader($name, $values)` | Replace a header |
| `->withAddedHeader($name, $value)` | Add another value (use for repeated headers such as `Set-Cookie`) |
| `->withStatus($s)` / `->withBody($b)` | |

You do not set `Content-Length`; the shell derives it. Bodies are strings held in memory, so there is no streaming:
the shell reads the whole body in a request or a response and enforces `worker.maxBodyBytes` both ways.
A request body above the limit is answered by the shell itself with `413 {"error":"body_too_large"}`.

### `Headers`

`Request::$headers` and `Response::$headers` are `Headers` objects: immutable, case-insensitive, and they keep **every**
value of a repeated header. `get($name)` is the first value, `all($name)` a list, `line($name)` the values joined by
`", "`, `has()`, `with()`, `withAdded()` and `without()`. Iterating yields one `name => value` pair per value, with
names in lower case.

## Routing

The library has no router; the handler is a function from request to response. Use whatever you like: a `match` on
the path as in the [getting-started example](getting-started.md), or a router component (FastRoute, Symfony Routing…)
fed `$request->method` and `$request->path()`. You can also wrap an existing framework's front controller, as long as
it can work from a request object rather than from the `$_SERVER` superglobals.

### Static files

`StaticFiles` serves files from one or more document roots: directories on disk, or `phar://` paths once packaged.

```php
$assets = new StaticFiles([__DIR__ . '/../public']);

$response = $assets->find($request);        // Response, or null on a miss
```

* Only `GET` and `HEAD` are served (`HEAD` returns no body).
* Roots are searched in order and the first match wins.
* A path containing `..`, a backslash or a NUL byte is refused. Directory listings are never served.
* Used directly as a `Handler`, a miss is `404 Not found`.

`find()` answers `null` on a miss, so you can fall through to your own routes or to a single-page app's `index.html`.

### Serving `shell.js`

The page-side helper lives in the library. Serve it from a route with `ShellScript::response()`, or copy the file
at build time (`ShellScript::path()` gives its location; inside a PHAR that is a `phar://` path).

```php
if ($request->path() === '/shell.js') {
    return ShellScript::response();
}
```

⚠️ `shell.js` is a non-PHP file inside `vendor/`. When you [package](packaging.md#phar-the-worker-payload) the app, add
`{"directory": "vendor/grafida/shell-php/resources"}` to `phar.include`, or the packaged app cannot serve it.

## Things that behave differently from a web request

### Standard output belongs to the shell

Standard output carries the binary frame stream. One stray byte there corrupts the conversation. The library therefore
protects it:

* `display_errors` is sent to standard error, and `html_errors` is switched off.
* Each handler call runs inside an output buffer. Anything your code `echo`es or `var_dump`s is diverted to standard
  error, one line at a time, tagged `[stray-output]`, and ends up in the [log](troubleshooting.md#logs). It never
  reaches the page.
* Output produced **before** `Worker::run()` is not covered unless you started an output buffer (the library flushes any
  buffer it finds to standard error, but PHP's CLI does not buffer by default). Start-up code must not print anything.
  Put `ini_set('display_errors', 'stderr');` at the top of the entry point, so that a warning during bootstrap cannot
  land ahead of the `ready` frame.

To debug, write to standard error (`fwrite(STDERR, …)`, `error_log()`) or use the log. Never write to `php://stdout`
or call `exit` / `die` with a message inside a handler.

### Errors

* An exception that escapes the handler is logged to standard error and answered with
  `500 {"error":"internal_error"}`. The worker keeps serving. Catch exceptions yourself if you want a better response.
* A body larger than the limit becomes `500 {"error":"response_too_large"}`.
* A response the protocol cannot carry (for example, an invalid header value) becomes `500 {"error":"bad_response"}`.
* A fatal PHP error or a segfault kills the worker. The in-flight request gets `502 {"error":"worker_restarted"}` and
  the shell restarts it.

### State, time and concurrency

* **One request at a time.** While your handler runs, no other request is processed. For slow work (a long download,
  a heavy computation), either keep it short and let the page poll, or run it in a child process
  (`proc_open`) and return immediately.
* **Memory leaks accumulate.** The worker may run for days. Release large objects, and set `memory_limit` sensibly (see
  [`phar.ini`](packaging.md#phar-the-worker-payload)).
* **No request-scoped globals.** Do not rely on `$_SERVER['REQUEST_URI']` or similar. Everything comes from `Request`.

### Files, working directory and user data

The worker's working directory is **not the same in development and in a package**:

| | Working directory | Your code lives in |
|---|---|---|
| `--dev` | The app directory | Real files on disk |
| Packaged | The directory holding `shell.json` (the install directory; `Contents/Resources` in a macOS bundle) | A read-only `phar://` archive |

So:

* **Locate your own files with `__DIR__`**, never with a relative path or `getcwd()`.
* **Never write beside your code.** The PHAR is read-only, and the install directory is often not writable by the user.
* **Store user data in a per-user directory** that you resolve yourself: `$XDG_DATA_HOME` (or `~/.local/share`) on
  Linux, `~/Library/Application Support` on macOS, `%APPDATA%` or `%LOCALAPPDATA%` on Windows. Create it on first use.
* Resources you ship but must hand to external tools (a binary, a script) have to be copied out of the PHAR to a real
  file first. `file_get_contents('phar://…')` works; `exec('phar://…')` does not.

### Extensions and PHP version

In development you run your system PHP, which usually has more extensions than the packaged runtime. A script that
works in `--dev` can therefore fail once packaged. The packaged runtime is PHP 8.4 with a fixed extension list; see
[Packaging](packaging.md#the-php-runtime-inside-the-package). Check the list before you depend on an extension, and
test the packaged build.

## Testing your worker

`Grafida\Shell\Testing\WorkerProcess` spawns *your* worker over real pipes and plays the shell's part. It is meant for
your test suite and works with PHPUnit or any other runner.

```php
use Grafida\Shell\Request;
use Grafida\Shell\Testing\WorkerProcess;

$worker = new WorkerProcess([PHP_BINARY, 'bin/worker.php'], cwd: __DIR__ . '/..');

$response = $worker->request(new Request('POST', 'myapp://app/api/save', ['content-type' => 'application/json'], '{"a":1}'));
self::assertSame(200, $response->status);

$worker->closeInput();
self::assertSame(0, $worker->wait());
self::assertStringNotContainsString('Fatal', $worker->stderr());
```

The constructor takes the command, a working directory, an optional environment (pass your own so tests never touch
real user data) and a ready timeout. `request()` sends a request and returns the matching response; `stderr()` returns
what the worker logged; `kill()`, `send()`, `receive()` and `wait()` are there for protocol-level tests. Every wait has
a deadline, so a hung worker fails a test instead of hanging the run.

Because a handler is just an object, you can also unit-test it by calling `$handler(new Request(…))` directly,
without any process. Use `WorkerProcess` for the things that only the real thing shows: start-up, `ready`, clean exit,
and stray output.

## The wire protocol

You normally never touch it. If you want to write a worker in something other than PHP, or understand what goes over
the pipe, read the [protocol specification](../../protocol/v1.md).
