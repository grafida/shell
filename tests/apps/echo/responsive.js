// The UI thread keeps painting while the worker takes 5 s over one request.
let maxGap = 0, last = performance.now(), running = true;
const tick = (now) => { maxGap = Math.max(maxGap, now - last); last = now; if (running) requestAnimationFrame(tick); };
requestAnimationFrame(tick);
const started = performance.now();
const r = await fetch('/sleep?ms=5000', { cache: 'no-store' });
const took = Math.round(performance.now() - started);
running = false;
return { status: r.status, tookMs: took, maxFrameGapMs: Math.round(maxGap), ok: r.status === 200 && took >= 5000 && maxGap < 100 };
