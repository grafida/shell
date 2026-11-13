// Who may frame the app's pages (security.md L1), in the real webview against the fake echo worker.
// Run with: scripts/dev/automation.py eval-file tests/apps/echo/framing.js, in the echo app
// (frameGuard on, SHELL_APP_ID=test.shell.echo) and in tests/apps/unguarded (frameGuard off,
// SHELL_APP_ID=test.shell.unguarded); the guard's state is read from the shell's own header.
// `/framed.html?<tag>` posts `{framed: '?<tag>'}` to the top window once it renders.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { rendered: [], failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };
addEventListener('message', (e) => { if (e.data && e.data.framed) out.rendered.push(e.data.framed); });

out.webview = (await (await fetch('/_native/info', { cache: 'no-store' })).json()).webview;
const probe = await fetch('/framed.html?probe', { cache: 'no-store' });
out.csp = probe.headers.get('content-security-policy');
out.guard = out.csp !== null;
check(out.csp === null || out.csp === "frame-ancestors 'self'", `content-security-policy: ${out.csp}`);

const frames = [];
const frame = (attrs) => {
    const f = document.createElement('iframe');
    Object.assign(f, attrs);
    document.body.append(f);
    frames.push(f);
};
const page = (tag, csp) => `${location.origin}/framed.html?${tag}` + (csp ? `&csp=${encodeURIComponent(csp)}` : '');
// A sandboxed srcdoc frame has an opaque origin: a foreign page as far as the engine is concerned.
const foreign = (src) => frame({ sandbox: 'allow-scripts', srcdoc: `<iframe src="${src}"></iframe>` });

frame({ src: page('own') });
// The app's own srcdoc frame (as TinyMCE's content frame), and an app page inside it.
frame({ srcdoc: `<script>top.postMessage({framed: '?own-srcdoc'}, '*')</script><iframe src="${page('own-nested')}"></iframe>` });
// The worker's `'none'` stops even the app's own frame where the engine enforces frame-ancestors on a
// scheme handler's response (reported as `headerEnforced`). WKWebView does not (nor X-Frame-Options;
// it does enforce the rest of the CSP): measured on macOS 27.2.
frame({ src: page('own-none', "frame-ancestors 'none'") });
foreign(page('foreign'));
// The worker's own frame-ancestors replaces the shell's header, but not the frame-buster script:
// with the guard on, only the script can stop this one (and on Windows it shows that the script
// reaches child frames).
foreign(page('foreign-allowed', 'frame-ancestors *'));
await sleep(3000);

const has = (tag) => out.rendered.some((t) => t === `?${tag}` || t.startsWith(`?${tag}&`));
out.own = has('own');
out.ownSrcdoc = has('own-srcdoc');
out.ownNested = has('own-nested');
out.ownNone = has('own-none');
out.foreign = has('foreign');
out.foreignAllowed = has('foreign-allowed');
check(out.own, "the app's own frame rendered");
check(out.ownSrcdoc, "the app's own srcdoc frame rendered");
check(out.ownNested, "an app page inside the app's own srcdoc frame rendered");
// Reported, not checked: the guard does not depend on the header (the injected script holds).
out.headerEnforced = !out.ownNone;
if (out.guard) {
    check(!out.foreign, 'a foreign frame could not render the app');
    check(!out.foreignAllowed, "a foreign frame could not render the app, whatever the worker's header said");
} else {
    // The control: with the guard off, the foreign frame does reach the app's page and render it,
    // so the guarded run's refusals are the guard's doing.
    check(out.foreign, 'frameGuard off: a foreign frame rendered the app');
    check(out.foreignAllowed, 'frameGuard off: a foreign frame rendered the app (worker: frame-ancestors *)');
}

frames.forEach((f) => f.remove());
out.ok = out.failures.length === 0;
return out;
