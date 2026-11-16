# Scheme bridge, request policy and frame guard

Read before changing `src/bridge.rs`, the per-platform scheme handlers, or the framing / CSP behaviour.

- **We own the scheme bridge where wry's falls short.** wry's Linux body reader stops at the first
  short read and collapses repeated headers; register the scheme on the WebKit `WebContext`
  ourselves, read to EOF in bounded chunks, keep repeated headers, mark the scheme secure **and**
  CORS-enabled.
- **`src/bridge.rs` is the platform-neutral half and is where request policy lives**: a foreign
  `Origin` → `403 cross_origin` (an article's `<iframe>` must not drive the app's API); any host of
  the scheme but the origin's (the engines route by scheme alone) → `404 not_found`; an escaped `/`
  or NUL in the path → `404 bad_path` (Apache's default); the path checks (`/_native/`,
  `noStorePaths`) compare the path percent-decoded once, as a decoding worker routes it, never the
  raw form (security.md L6), while the worker still gets the URL verbatim; `/_native/*`
  never reaches the worker; `x-shell-file-path` from the page is always dropped and an `x-shell-file`
  token is resolved into it (the token itself is not forwarded); `no-store` is added under the
  manifest's `noStorePaths` when the worker sent no `cache-control`/`expires`;
  `content-security-policy: frame-ancestors 'self'` is added to every worker response unless the
  worker sent `x-frame-options` or a `frame-ancestors` directive of its own (security.md L1).
- **Only the app frames its pages while `webview.frameGuard` is on (the default), and the header is
  not what holds it**: WKWebView enforces neither `frame-ancestors` nor `x-frame-options` on a scheme
  response (measured; the rest of the CSP it does), so every platform also injects
  `navigation::frame_buster` into every frame at document start, which empties an app page with a
  foreign ancestor. Off, the bridge adds no header and lets a cross-site *frame navigation* through,
  the buster is not injected, and macOS drops M1's source-frame checks; subresources stay refused.
  `tests/apps/echo/framing.js` checks both states (`tests/apps/unguarded` is the guard-off app).
