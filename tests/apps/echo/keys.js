// macOS only: the key reporter's message handler is out of reach of page script (security.md L4).
// Run with: scripts/dev/automation.py eval-file tests/apps/echo/keys.js, in the echo app. The
// automation channel's eval runs in the page world, as the app's and any embed's own script does.
// The reporter itself lives in a content world of its own; that it still reports is what the
// parity script's "Edit shortcuts reach the page first" row proves (its 0.6 s waits are inside the
// 1 s fallback).
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { reachable: {}, failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };
const probe = 'typeof window.webkit?.messageHandlers?.shellKey?.postMessage === "function"';
addEventListener('message', (e) => {
    if (e.data && typeof e.data.where === 'string') out.reachable[e.data.where] = e.data.reachable;
});

// The same expression as `probe`, in this frame.
out.reachable.main = typeof window.webkit?.messageHandlers?.shellKey?.postMessage === 'function';
const frames = [];
const frame = (where, attrs) => {
    const f = document.createElement('iframe');
    Object.assign(f, attrs, { srcdoc: `<script>top.postMessage({where: '${where}', reachable: ${probe}}, '*')</script>` });
    document.body.append(f);
    frames.push(f);
};
// The app's own srcdoc frame (as TinyMCE's), and a sandboxed one: an opaque origin, i.e. a foreign
// page as far as the engine is concerned.
frame('own', {});
frame('foreign', { sandbox: 'allow-scripts' });
await sleep(1500);

for (const where of ['main', 'own', 'foreign']) {
    check(where in out.reachable, `${where}: the frame reported`);
    check(out.reachable[where] === false, `${where}: page script cannot post to shellKey`);
}
frames.forEach((f) => f.remove());
out.ok = out.failures.length === 0;
return out;
