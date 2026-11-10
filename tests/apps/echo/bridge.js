// The scheme bridge, exercised from inside the real webview against the fake echo worker.
// Run with: scripts/dev/automation.py eval-file tests/apps/echo/bridge.js (SHELL_APP_ID=test.shell.echo)
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
const out = { origin: location.origin, selfOrigin: self.origin, secure: isSecureContext, bodies: [], failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };

for (const size of [0, 1, 1023, 1024, 1025, 65536, 1048577, 10 << 20, 50 << 20]) {
    const body = new Uint8Array(size);
    for (let i = 0; i < size; i++) body[i] = (i * 31) % 251;
    const want = hex(await crypto.subtle.digest('SHA-256', body));
    const started = performance.now();
    const r = await fetch('/api/echo', { method: 'POST', body, cache: 'no-store' });
    const j = await r.json();
    const ms = Math.round(performance.now() - started);
    out.bodies.push({ size, ms, status: r.status, intact: j.bodyLength === size && j.bodySha256 === want });
    check(r.status === 200 && j.bodyLength === size && j.bodySha256 === want, `body ${size}`);
}

for (const method of ['GET', 'PATCH', 'DELETE', 'PUT']) {
    const r = await fetch('/api/m?q=%C3%A9', { method, cache: 'no-store' });
    const j = await r.json();
    check(j.method === method && j.url === 'boson://app/api/m?q=%C3%A9', `method ${method}: ${j.method} ${j.url}`);
}

let r = await fetch('/api/headers', { headers: { 'X-Custom': 'yes' }, cache: 'no-store' });
let j = await r.json();
out.multi = r.headers.get('x-multi');
check(out.multi === 'one, two', `repeated response header: ${out.multi}`);
check(JSON.stringify(j.headers['x-custom']) === '["yes"]', 'request header reaches the worker');
out.apiCacheControl = r.headers.get('cache-control');
check(out.apiCacheControl === 'no-store', `no-store under /api/: ${out.apiCacheControl}`);

r = await fetch('/static.txt', { cache: 'no-store' });
out.staticCacheControl = r.headers.get('cache-control');
check(out.staticCacheControl === null, `nothing added outside /api/: ${out.staticCacheControl}`);

r = await fetch('/api/echo', { method: 'POST', body: new Uint8Array((60 << 20) + 1), cache: 'no-store' });
out.overLimit = r.status;
check(r.status === 413 && (await r.json()).error === 'body_too_large', `over the limit: ${r.status}`);

r = await fetch('/api/x', { headers: { 'X-Shell-File-Path': '/etc/shadow' }, cache: 'no-store' });
j = await r.json();
check(!('x-shell-file-path' in j.headers), 'injected X-Shell-File-Path stripped');
r = await fetch('/api/x', { headers: { 'X-Shell-File': 'forged' }, cache: 'no-store' });
check(r.status === 400 && (await r.json()).error === 'unknown_file_token', 'forged token refused');

r = await fetch('/_native/info', { cache: 'no-store' });
out.info = await r.json();
check(out.info.protocol === 1 && /^WebKit(GTK)? /.test(out.info.webview), 'native info');

out.ok = out.failures.length === 0;
return out;
