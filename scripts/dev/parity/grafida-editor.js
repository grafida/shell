// Grafida's editor inside the shell: paste as plain text (the shell reads the clipboard) and an
// image file dropped on the editor (the shell takes the drop). Needs a site; the script seeds one.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };

openNewArticle();
for (let i = 0; i < 100 && !(State.tinyMCEEditor && State.tinyMCEEditor.initialized); i++) await sleep(100);
const ed = State.tinyMCEEditor;
check(!!ed, 'the editor opened');

await navigator.clipboard.writeText('Plain​ text\r\nsecond line');
ed.setContent('<p>start</p>');
ed.selection.select(ed.getBody(), true);
ed.selection.collapse(false);
await insertClipboardAsPlainText(ed);
out.pasted = ed.getContent();
// A Wayland compositor hands the clipboard only to the focused window; a desktop nobody has touched
// (GNOME logs in to its Overview) leaves it unfocused, and a real Ctrl+Shift+V always comes with focus.
out.focused = document.hasFocus();
check(out.pasted === '<p>startPlain text<br>second line</p>',
    'Ctrl+Shift+V path: ' + out.pasted + (out.focused ? '' : ' (the window has no keyboard focus: the compositor gives the clipboard only to the focused window)'));

const r = ed.iframeElement.getBoundingClientRect();
out.dropAt = [Math.round(r.left + r.width / 2), Math.round(r.top + 40)];
return out;
