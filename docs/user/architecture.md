# Architecture

How Grafida Shell fits into a desktop application built on it, and how the application talks to it.

## Where the shell sits

At run time the shell is the parent process. It owns the window, borrows the operating system's webview to show your UI, and starts your PHP code as a child process. The shell contains nothing specific to your application: `shell.json` supplies the name, scheme, window size and worker command.

```mermaid
flowchart TB
    launcher["Launcher<br/>(Linux only)"]
    manifest["shell.json<br/>(app manifest)"]

    subgraph shell["grafida-shell: one Rust process"]
        direction LR
        subgraph wv["System webview (provided by the OS)"]
            ui["<b>App UI</b><br/>HTML, JS, CSS<br/>origin shell://app"]
        end
        subgraph core["Shell"]
            direction TB
            bridge["Scheme bridge<br/>serves shell://app"]
            native["Native endpoints<br/>/_native/*, answered in Rust"]
            sup["Worker supervisor<br/>spawns, restarts, forwards"]
            win["Window, menus, alerts"]
        end
    end

    subgraph worker["PHP worker: child process"]
        direction TB
        runtime["shell-php runtime<br/>frame loop, requests"]
        app["<b>Your app code</b><br/>routes, API, logic"]
        micro["phpmicro<br/>PHP in one executable"]
    end

    os["Operating system<br/>GTK + WebKitGTK, Cocoa + WKWebView, Win32 + WebView2<br/>file dialogs, processes"]

    launcher -- "starts the shell if its libraries load" --> shell
    manifest -- configures --> shell
    ui -- "request" --> bridge
    bridge -- "response" --> ui
    sup -- "stdin: request frames" --> runtime
    runtime -- "stdout: response frames" --> sup
    shell -- "windows, dialogs, process spawn" --> os

    classDef shellc fill:#dcefee,stroke:#0d7d78,color:#1b2227
    classDef appc fill:#f8e9d2,stroke:#a8620a,color:#1b2227
    classDef osc fill:#eceff1,stroke:#8a969e,stroke-dasharray:5 4,color:#1b2227
    class bridge,native,sup,win,runtime,manifest shellc
    class ui,app appc
    class os,launcher,micro osc
```

Teal is the shell (generic), amber is your application, dashed grey is the operating system or a bundled runtime.

## How the application talks to the shell

The page has a single door: every request goes to `shell://app` (or whatever scheme `shell.json` names).

```mermaid
flowchart LR
    subgraph page["Page (webview)"]
        f1["fetch('/api/...')<br/>application data"]
        f2["fetch('/_native/...')<br/>desktop features"]
    end

    subgraph shell["grafida-shell"]
        policy["Policy: origin, headers, caching"]
        sup["Supervisor<br/>everything else goes to the worker"]
        native["Native endpoints<br/>/_native/* never reaches PHP"]
        policy --> sup
        policy --> native
    end

    worker["PHP worker<br/>handles the request,<br/>returns status, headers, body"]
    os["Operating system<br/>open and save dialogs<br/>open URL, reveal file<br/>title, theme, quit"]

    f1 -- "shell://app/api/..." --> policy
    f2 -- "POST JSON" --> policy
    sup -- "request frame (stdin)" --> worker
    worker -- "response frame (stdout)" --> sup
    native -- "native call" --> os
```

- Requests for `/_native/*` end at the shell and become operating system calls such as file dialogs. The result goes back to the page as JSON.
- All other requests become length-prefixed frames on the worker's stdin. The worker's reply frame comes back on stdout and the shell hands it to the page as an ordinary HTTP response. The page never learns that the worker is a separate process.
- The UI thread never waits on the worker. Answers are posted back to it and completed there.

The frame format is specified in the [wire protocol v1](../../protocol/v1.md); the reasoning behind the stack is in [ADR 0001](../adr/0001-shell-stack.md).
