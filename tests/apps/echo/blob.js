// Request bodies WKWebView cannot deliver (grafida/grafida#88, ADR 0001 "WKWebView drops file
// bodies"). On macOS a document-start script makes fetch() reject, and XMLHttpRequest.send() and
// navigator.sendBeacon() throw, a TypeError for a Blob/File body or a FormData with a file part sent
// to the app's own origin. Elsewhere such bodies arrive intact. Every other body arrives intact
// everywhere.
// Run with: scripts/dev/automation.py eval-file tests/apps/echo/blob.js (SHELL_APP_ID=test.shell.echo)
const out = { failures: [], refused: {}, delivered: {} };
const check = (ok, what) => { if (!ok) out.failures.push(what); };
const info = await (await fetch('/_native/info', { cache: 'no-store' })).json();
out.platform = info.platform;
const guarded = info.platform === 'macos';

const bytes = new Uint8Array(4096).map((_, i) => i % 251);
const blob = () => new Blob([bytes], { type: 'application/octet-stream' });
const file = () => new File([bytes], 'a.bin', { type: 'application/octet-stream' });
const formWithFile = () => { const f = new FormData(); f.append('text', 'kept'); f.append('file', blob(), 'a.bin'); return f; };
const formWithout = () => { const f = new FormData(); f.append('text', 'kept'); return f; };

const fetchOutcome = async (make) => {
    try {
        const r = await make();
        const j = await r.json();
        return { sent: true, status: r.status, bodyLength: j.bodyLength };
    } catch (e) {
        return { sent: false, type: e.constructor.name, message: String(e.message) };
    }
};
const fileCases = {
    'fetch Blob': () => fetch('/api/echo', { method: 'POST', body: blob(), cache: 'no-store' }),
    'fetch File': () => fetch('/api/echo', { method: 'POST', body: file(), cache: 'no-store' }),
    'fetch FormData with a file': () => fetch('/api/echo', { method: 'POST', body: formWithFile(), cache: 'no-store' }),
    'fetch Request with a Blob': () => fetch(new Request('/api/echo', { method: 'POST', body: blob(), cache: 'no-store' })),
    'fetch Request cloned from one with a Blob': () => fetch(new Request(new Request('/api/echo', { method: 'POST', body: blob() }))),
    'fetch absolute URL, Blob': () => fetch(location.origin + '/api/echo', { method: 'POST', body: blob(), cache: 'no-store' }),
};
for (const [name, make] of Object.entries(fileCases)) {
    const o = await fetchOutcome(make);
    if (guarded) {
        out.refused[name] = o;
        check(!o.sent && o.type === 'TypeError' && /ArrayBuffer/.test(o.message), `${name}: refused with a TypeError`);
    } else {
        out.delivered[name] = o;
        check(o.sent && o.status === 200 && o.bodyLength > 0, `${name}: delivered`);
    }
}

const xhr = (body) => new Promise((resolve) => {
    const x = new XMLHttpRequest();
    x.open('POST', '/api/echo');
    x.onload = () => resolve({ sent: true, status: x.status, bodyLength: JSON.parse(x.responseText).bodyLength });
    x.onerror = () => resolve({ sent: true, status: 0 });
    try { x.send(body); } catch (e) { resolve({ sent: false, type: e.constructor.name, message: String(e.message) }); }
});
let o = await xhr(blob());
if (guarded) {
    out.refused['XMLHttpRequest Blob'] = o;
    check(!o.sent && o.type === 'TypeError', 'XMLHttpRequest Blob: send() throws a TypeError');
} else {
    check(o.sent && o.bodyLength === bytes.length, 'XMLHttpRequest Blob: delivered');
}
if (guarded) {
    try { navigator.sendBeacon('/api/echo', blob()); out.refused['sendBeacon Blob'] = { sent: true }; check(false, 'sendBeacon Blob: throws a TypeError'); }
    catch (e) { out.refused['sendBeacon Blob'] = { sent: false, type: e.constructor.name }; check(e instanceof TypeError, 'sendBeacon Blob: throws a TypeError'); }
}

// Everything else is untouched, guard or no guard.
const text = 'x'.repeat(1000);
const okCases = {
    'string': [() => fetch('/api/echo', { method: 'POST', body: text, cache: 'no-store' }), 1000],
    'ArrayBuffer': [() => fetch('/api/echo', { method: 'POST', body: bytes.buffer, cache: 'no-store' }), bytes.length],
    'typed array': [() => fetch('/api/echo', { method: 'POST', body: bytes, cache: 'no-store' }), bytes.length],
    'URLSearchParams': [() => fetch('/api/echo', { method: 'POST', body: new URLSearchParams({ a: '1' }), cache: 'no-store' }), 3],
    'Request with a string': [() => fetch(new Request('/api/echo', { method: 'POST', body: text, cache: 'no-store' })), 1000],
};
for (const [name, [make, length]] of Object.entries(okCases)) {
    const r = await fetchOutcome(make);
    out.delivered[name] = r;
    check(r.sent && r.status === 200 && r.bodyLength === length, `${name}: delivered intact`);
}
const form = await fetchOutcome(() => fetch('/api/echo', { method: 'POST', body: formWithout(), cache: 'no-store' }));
out.delivered['FormData without a file'] = form;
check(form.sent && form.status === 200 && form.bodyLength > 0, 'FormData without a file: delivered');
o = await xhr('abc');
check(o.sent && o.bodyLength === 3, 'XMLHttpRequest string: delivered');
check(Request.name === 'Request' && new Request('/x') instanceof Request, 'Request still behaves as Request');
return out;
