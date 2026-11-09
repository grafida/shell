// The file-token flow. Run after `automation.py pick <file>` (and `pick --cancel` for the second).
const out = { failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };
const post = (endpoint, body, headers = {}) =>
    fetch(endpoint, { method: 'POST', cache: 'no-store', headers, body: JSON.stringify(body) });

let picked = await (await post('/_native/dialog/open-file', { filters: [{ name: 'Images', extensions: ['png'] }] })).json();
out.picked = picked;
const token = picked.files && picked.files[0].token;
check(typeof token === 'string' && !JSON.stringify(picked).includes('/'), 'a token, never a path');

let r = await post('/api/read', {}, { 'X-Shell-File': token, 'X-Shell-File-Path': '/etc/shadow' });
let j = await r.json();
out.forwarded = j.headers['x-shell-file-path'];
check(out.forwarded && out.forwarded.length === 1 && out.forwarded[0] !== '/etc/shadow', 'resolved path forwarded, injected one gone');
check(!('x-shell-file' in j.headers), 'the token itself stays in the shell');

const cancelled = await (await post('/_native/dialog/open-file', {})).json();
check(cancelled.cancelled === true, 'cancel → {cancelled:true}');

r = await post('/api/read', {}, { 'X-Shell-File': out.staleToken || 'deadbeefdeadbeef-' + '0'.repeat(32) });
check(r.status === 400 && ['stale_file_token', 'unknown_file_token'].includes((await r.json()).error), 'other-session token refused');
return out;
