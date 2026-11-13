// Grafida inside the shell: boot, origin, Help (bundled docs + external links), the dialog flows.
// Run after queueing: pick <png>, pick --cancel, pick <md>, pick <folder>.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };

check(location.origin === 'shell://app' && isSecureContext, 'origin shell://app, secure');
check(await underShell(), 'underShell()');

document.querySelector('#main-nav [data-screen="help"]').click();
for (let i = 0; i < 40 && !document.querySelector('#help-page h1, #help-page h2'); i++) await sleep(100);
check(!!document.querySelector('#help-page h1, #help-page h2'), 'Help screen renders a page');
const external = document.querySelector('#help-page a[data-help-external]');
out.helpExternal = external ? external.href : null;
if (external) external.click();

const img = await api.openFile('image');
check(img.mime === 'image/png' && img.name.endsWith('.png'), 'openFile(image)');
check((await api.openFile('image')).cancelled === true, 'openFile cancelled');
const md = await api.openFile('markdown');
check(md.mime === 'text/markdown', 'openFile(markdown)');
const dir = await api.selectDirectory();
check(typeof dir.token === 'string' && !('path' in dir), 'selectDirectory → token');
const exported = await api.exportRequestLog(dir);
out.exported = exported.path;
out.folderToken = dir.token;
return out;
