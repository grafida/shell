// The navigation policy, exercised in the real webview. Needs network access for the embed.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const out = { failures: [] };
const check = (ok, what) => { if (!ok) out.failures.push(what); };
const home = location.href;

const click = (href, target) => {
    const a = document.createElement('a');
    a.href = href;
    if (target) a.target = target;
    document.body.append(a);
    a.click();
    a.remove();
};

click('https://example.com/clicked');
click('https://example.com/blank', '_blank');
click('mailto:someone@example.com');
// A script the UI process injects counts as a user gesture in WebKit, and a gesture survives
// about a second through timers. Run the no-gesture cases from a timer well past that.
await new Promise((resolve) => setTimeout(() => {
    out.popup = window.open('https://example.com/popup') === null;  // blocked outright
    location.href = 'https://example.com/scripted';                 // refused at the response
    resolve();
}, 1500));
check(out.popup, 'a script popup without a gesture is blocked');
await sleep(2500);
check(location.href === home, `main frame still home after a scripted navigation: ${location.href}`);
await new Promise((resolve) => setTimeout(() => { location.href = 'file:///etc/passwd'; resolve(); }, 1500));
await sleep(500);
check(location.href === home, `main frame still home after file:: ${location.href}`);

// An embed loads inside its frame, and does not take the app with it.
const frame = document.createElement('iframe');
const loaded = new Promise((r) => { frame.onload = () => r(true); setTimeout(() => r(false), 8000); });
frame.src = 'https://example.com/embed';
document.body.append(frame);
out.embedLoaded = await loaded;
check(out.embedLoaded, 'the embed loaded in its frame');
check(location.href === home, 'the embed did not navigate the app');
frame.remove();

out.home = location.href;
return out;

