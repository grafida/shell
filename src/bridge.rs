//! The platform-neutral half of the scheme handler: everything that happens to a request between
//! the webview handing it over and the worker (or the shell itself) answering it.
//!
//! The platform layer turns a webview request into a [`Request`], calls [`Bridge::handle`] on the
//! UI thread, and completes the webview request from the [`Responder`] — which may be called on any
//! thread, so the platform layer posts it back to the UI thread through its event loop. Nothing in
//! here blocks.
//!
//! In order, for every request:
//! 1. a request carrying a foreign `Origin`, or `Sec-Fetch-Site: cross-site` (what a browser sends
//!    instead of an `Origin` on a plain GET), is refused (`403 cross_origin`), so a page embedded in
//!    the app (an article's `<iframe>`) cannot drive the app's API; with the manifest's
//!    `webview.frameGuard` off, a cross-site navigation *into a frame* is let through;
//! 2. a URL of the scheme with any authority but the app's (`shell://other/…`, which the engines
//!    route here too) is `404 not_found`, so the worker's `url` always starts with the origin;
//! 3. `/_native/*` is answered by the shell ([`crate::native`]) and never reaches the worker;
//! 4. any `x-shell-file-path` the page sent is dropped, and an `x-shell-file` token is resolved
//!    into `x-shell-file-path` (`400` for an unknown or stale token, without contacting the worker);
//! 5. a request carrying `x-shell-clipboard: read` gets the OS clipboard's text as its **body** and
//!    `x-shell-clipboard: text` in place of the request header, so the app reads the clipboard the
//!    shell read for it (any other value of that header is dropped);
//! 6. the request goes to the worker, and on the way back `cache-control: no-store` is added under
//!    the manifest's `noStorePaths` when the worker sent no cache header of its own, and
//!    `content-security-policy: frame-ancestors 'self'` when the frame guard is on and the worker
//!    sent no framing policy of its own (`x-frame-options`, or a `frame-ancestors` directive). Not
//!    every engine enforces it on a scheme handler's response (WKWebView does not), which is why
//!    each platform also injects [`crate::navigation::frame_buster`] into every frame.
//!
//! The body limit is the platform layer's to enforce while it reads (it must stop reading, not read
//! and then check), using [`Bridge::max_body_bytes`] and answering [`Bridge::too_large`].

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{
    http::{self, Request, Response},
    manifest::Theme,
    native,
    navigation::{self, Target},
    supervisor::{Responder, Supervisor},
    tokens::TokenStore,
};

pub const FILE_TOKEN_HEADER: &str = "x-shell-file";
pub const FILE_PATH_HEADER: &str = "x-shell-file-path";
pub const CLIPBOARD_HEADER: &str = "x-shell-clipboard";

/// Where requests for the app go: the supervisor in the real shell, a stub in tests.
pub trait Backend: Send + Sync {
    fn submit(&self, request: Request, respond: Responder);
}

impl Backend for Supervisor {
    fn submit(&self, request: Request, respond: Responder) {
        Supervisor::submit(self, request, respond);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// The result of a dialog: `None` when the user cancelled.
pub type Picked = Option<Vec<PathBuf>>;
pub type DialogDone = Box<dyn FnOnce(Picked) + Send>;

/// The clipboard's text: `None` when it holds no text (or nothing at all).
pub type ClipboardDone = Box<dyn FnOnce(Option<String>) + Send>;

/// What the platform provides to the native endpoints. Every method may be called on the UI thread
/// and must return at once; a dialog reports back through `done`, from any thread.
pub trait Host: Send + Sync {
    fn open_file(&self, filters: Vec<FileFilter>, multiple: bool, done: DialogDone);
    fn select_folder(&self, done: DialogDone);
    fn save_file(&self, suggested_name: String, filters: Vec<FileFilter>, done: DialogDone);
    fn open_url(&self, url: &str) -> Result<(), String>;
    fn reveal(&self, path: &Path) -> Result<(), String>;
    fn set_window_theme(&self, theme: Theme);
    fn set_window_title(&self, title: &str);
    fn quit(&self);
    /// Reads the OS clipboard's text. The shell owns the window, so it can read the clipboard
    /// without a helper tool and without a prompt.
    fn read_clipboard(&self, done: ClipboardDone);
    /// e.g. `WebKitGTK 2.54.2`.
    fn webview_version(&self) -> String;
}

pub struct Bridge {
    pub(crate) origin: String,
    max_body_bytes: u64,
    no_store_paths: Vec<String>,
    pub(crate) host: Arc<dyn Host>,
    backend: Arc<dyn Backend>,
    pub(crate) tokens: Arc<Mutex<TokenStore>>,
    pub(crate) dialog_open: Arc<Mutex<bool>>,
    frame_guard: bool,
}

impl Bridge {
    pub fn new(
        origin: String,
        max_body_bytes: u64,
        no_store_paths: Vec<String>,
        host: Arc<dyn Host>,
        backend: Arc<dyn Backend>,
    ) -> Bridge {
        Bridge {
            origin,
            max_body_bytes,
            no_store_paths,
            host,
            backend,
            tokens: Arc::new(Mutex::new(TokenStore::new())),
            dialog_open: Arc::new(Mutex::new(false)),
            frame_guard: true,
        }
    }

    /// The manifest's `webview.frameGuard` (on unless the app turns it off): whether only the app
    /// may frame its pages. Off, no `frame-ancestors` is added, and a foreign frame's *navigation*
    /// to the app (`Sec-Fetch-Site: cross-site` with `Sec-Fetch-Mode: navigate` into a frame) is let
    /// through, so the page can be framed; a foreign frame's subresource requests stay refused.
    pub fn with_frame_guard(mut self, on: bool) -> Bridge {
        self.frame_guard = on;
        self
    }

    pub fn frame_guard(&self) -> bool {
        self.frame_guard
    }

    pub fn max_body_bytes(&self) -> u64 {
        self.max_body_bytes
    }

    /// The answer to a request whose body exceeded the limit.
    pub fn too_large(&self) -> Response {
        Response::error(413, "body_too_large")
    }

    pub fn handle(&self, mut request: Request, respond: Responder) {
        if let Some(origin) = http::first(&request.headers, "origin")
            && origin != self.origin
        {
            respond(Response::error(403, "cross_origin"));
            return;
        }

        // A browser sends no `Origin` on a plain GET (an `<img>`, a `no-cors` fetch, a frame
        // navigating itself), but it does say where the request came from. With the frame guard
        // off, a foreign frame may still be sent to one of the app's pages (that is framing).
        if http::first(&request.headers, "sec-fetch-site")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("cross-site"))
            && (self.frame_guard || !is_frame_navigation(&request))
        {
            respond(Response::error(403, "cross_origin"));
            return;
        }

        // The engines route by scheme alone, so `<scheme>://other/…` lands here too, and
        // `path()` ignores the authority. Only the app's own reaches the worker or `/_native/*`:
        // the worker can rely on its `url` starting with the origin.
        if navigation::classify(&request.url, &self.origin) != Target::App {
            respond(Response::error(404, "not_found"));
            return;
        }

        if request.path().starts_with("/_native/") {
            native::handle(self, request, respond);
            return;
        }

        request.headers.remove(FILE_PATH_HEADER);
        if let Some(tokens) = request.headers.remove(FILE_TOKEN_HEADER) {
            match self.resolve_file_header(&tokens) {
                Ok(path) => http::append(&mut request.headers, FILE_PATH_HEADER, path),
                Err(code) => {
                    respond(Response::error(400, code));
                    return;
                }
            }
        }

        let wants_clipboard = request
            .headers
            .remove(CLIPBOARD_HEADER)
            .is_some_and(|values| values.iter().any(|v| v.trim() == "read"));

        let backend = self.backend.clone();
        let no_store = self
            .no_store_paths
            .iter()
            .any(|prefix| request.path().starts_with(prefix.as_str()));

        if !wants_clipboard {
            forward(&*backend, no_store, self.frame_guard, request, respond);
            return;
        }

        let limit = self.max_body_bytes;
        let frame_guard = self.frame_guard;
        self.host.read_clipboard(Box::new(move |text| {
            let text = text.unwrap_or_default();
            if text.len() as u64 > limit {
                respond(Response::error(413, "body_too_large"));
                return;
            }
            request.body = text.into_bytes();
            http::append(&mut request.headers, CLIPBOARD_HEADER, "text");
            forward(&*backend, no_store, frame_guard, request, respond);
        }));
    }

    /// Issues file tokens for paths the user handed the app some other way than a dialog (a drop),
    /// as the entries a `dialog/open-file` answer carries: `{token, name, size}`.
    pub fn issue_files(&self, paths: Vec<PathBuf>) -> Vec<serde_json::Value> {
        let mut tokens = self.tokens.lock().unwrap();
        paths
            .into_iter()
            .map(|path| native::file_entry(&mut tokens, path, crate::tokens::Kind::File))
            .collect()
    }

    fn resolve_file_header(&self, values: &[String]) -> Result<String, &'static str> {
        let [token] = values else {
            return Err("bad_file_token");
        };
        let tokens = self.tokens.lock().unwrap();
        let picked = tokens.resolve(token.trim()).map_err(|e| e.code())?;
        header_safe_path(&picked.path).ok_or("unsupported_path")
    }
}

/// A frame being sent to one of the app's pages (`Sec-Fetch-Mode: navigate` into an `iframe` or a
/// `frame`), as opposed to a subresource request.
fn is_frame_navigation(request: &Request) -> bool {
    let is = |name, values: &[&str]| {
        http::first(&request.headers, name)
            .is_some_and(|v| values.iter().any(|w| v.trim().eq_ignore_ascii_case(w)))
    };
    is("sec-fetch-mode", &["navigate"]) && is("sec-fetch-dest", &["iframe", "frame"])
}

fn forward(
    backend: &dyn Backend,
    no_store: bool,
    frame_guard: bool,
    request: Request,
    respond: Responder,
) {
    backend.submit(
        request,
        Box::new(move |mut response| {
            if no_store
                && !response.headers.contains_key("cache-control")
                && !response.headers.contains_key("expires")
            {
                http::append(&mut response.headers, "cache-control", "no-store");
            }
            if frame_guard && !sets_framing_policy(&response.headers) {
                http::append(
                    &mut response.headers,
                    "content-security-policy",
                    NO_FOREIGN_FRAMING,
                );
            }
            respond(response);
        }),
    );
}

/// What the shell adds so that only the app itself may frame its pages (security.md L1): a foreign
/// page the app embeds (an article's `<iframe>`) could otherwise frame a live app page and overlay
/// it. On every response, not only HTML: an SVG, a PDF or a JSON answer renders in a frame too,
/// and `frame-ancestors` means nothing to a subresource.
const NO_FOREIGN_FRAMING: &str = "frame-ancestors 'self'";

/// Whether the worker chose its own framing policy: an `x-frame-options`, or an enforced
/// `content-security-policy` with a `frame-ancestors` directive. A policy without one gets the
/// shell's as a second policy beside it (the engine enforces every policy), which leaves the
/// worker's untouched. Policies are split on `,` (repeated headers), directives on `;`.
fn sets_framing_policy(headers: &crate::protocol::HeaderMap) -> bool {
    headers.contains_key("x-frame-options")
        || headers
            .get("content-security-policy")
            .into_iter()
            .flatten()
            .flat_map(|value| value.split([',', ';']))
            .filter_map(|directive| directive.split_ascii_whitespace().next())
            .any(|name| name.eq_ignore_ascii_case("frame-ancestors"))
}

/// A path as a header value: UTF-8, and free of the bytes a header value may not carry.
pub fn header_safe_path(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    (!s.contains(['\r', '\n', '\0'])).then(|| s.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{protocol::HeaderMap, tokens::Kind};
    use std::sync::mpsc;

    /// Records what reached the "worker" and answers 200 with no headers.
    #[derive(Default)]
    pub struct Echo {
        pub seen: Mutex<Vec<Request>>,
    }

    impl Backend for Echo {
        fn submit(&self, request: Request, respond: Responder) {
            self.seen.lock().unwrap().push(request);
            respond(Response::new(200));
        }
    }

    /// A host whose dialogs pick what the test told them to.
    #[derive(Default)]
    pub struct FakeHost {
        pub pick: Mutex<Picked>,
        pub clipboard: Mutex<Option<String>>,
        pub calls: Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn picked(&self, done: DialogDone) {
            done(self.pick.lock().unwrap().clone());
        }
        fn call(&self, what: String) {
            self.calls.lock().unwrap().push(what);
        }
    }

    impl Host for FakeHost {
        fn open_file(&self, filters: Vec<FileFilter>, multiple: bool, done: DialogDone) {
            self.call(format!("open_file {filters:?} {multiple}"));
            self.picked(done);
        }
        fn select_folder(&self, done: DialogDone) {
            self.call("select_folder".into());
            self.picked(done);
        }
        fn save_file(&self, name: String, filters: Vec<FileFilter>, done: DialogDone) {
            self.call(format!("save_file {name} {filters:?}"));
            self.picked(done);
        }
        fn open_url(&self, url: &str) -> Result<(), String> {
            self.call(format!("open_url {url}"));
            Ok(())
        }
        fn reveal(&self, path: &Path) -> Result<(), String> {
            self.call(format!("reveal {}", path.display()));
            Ok(())
        }
        fn set_window_theme(&self, theme: Theme) {
            self.call(format!("theme {theme:?}"));
        }
        fn set_window_title(&self, title: &str) {
            self.call(format!("title {title}"));
        }
        fn quit(&self) {
            self.call("quit".into());
        }
        fn read_clipboard(&self, done: ClipboardDone) {
            self.call("read_clipboard".into());
            done(self.clipboard.lock().unwrap().clone());
        }
        fn webview_version(&self) -> String {
            "TestKit 1.0".into()
        }
    }

    pub fn bridge() -> (Bridge, Arc<FakeHost>, Arc<Echo>) {
        let host = Arc::new(FakeHost::default());
        let echo = Arc::new(Echo::default());
        let bridge = Bridge::new(
            "shell://app".into(),
            1 << 20,
            vec!["/api/".into()],
            host.clone(),
            echo.clone(),
        );
        (bridge, host, echo)
    }

    pub fn call(
        bridge: &Bridge,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Response {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            http::append(&mut map, k, *v);
        }
        let (tx, rx) = mpsc::channel();
        bridge.handle(
            Request {
                method: method.into(),
                url: format!("shell://app{path}"),
                headers: map,
                body: body.as_bytes().to_vec(),
            },
            Box::new(move |r| tx.send(r).unwrap()),
        );
        rx.recv().unwrap()
    }

    pub fn json(r: &Response) -> serde_json::Value {
        serde_json::from_slice(&r.body).unwrap()
    }

    #[test]
    fn api_responses_without_a_cache_header_get_no_store() {
        let (b, _, _) = bridge();
        let r = call(&b, "GET", "/api/drafts", &[], "");
        assert_eq!(r.headers["cache-control"], vec!["no-store"]);
        let r = call(&b, "GET", "/js/app.js", &[], "");
        assert!(!r.headers.contains_key("cache-control"));
    }

    #[test]
    fn a_cache_header_from_the_worker_is_left_alone() {
        struct Cached;
        impl Backend for Cached {
            fn submit(&self, _: Request, respond: Responder) {
                let mut r = Response::new(200);
                http::append(&mut r.headers, "cache-control", "max-age=60");
                http::append(&mut r.headers, "cache-control", "private");
                respond(r);
            }
        }
        let b = Bridge::new(
            "shell://app".into(),
            1 << 20,
            vec!["/api/".into()],
            Arc::new(FakeHost::default()),
            Arc::new(Cached),
        );
        let r = call(&b, "GET", "/api/x", &[], "");
        assert_eq!(r.headers["cache-control"], vec!["max-age=60", "private"]);
    }

    /// The engines hand the shell every URL of its scheme, whatever the authority; only the app's
    /// own reaches the worker or the native endpoints, so the worker's `url` is always `<origin>/…`.
    #[test]
    fn another_authority_of_the_scheme_is_not_found() {
        let (b, _, echo) = bridge();
        let send = |url: &str| {
            let (tx, rx) = mpsc::channel();
            b.handle(
                Request {
                    method: "GET".into(),
                    url: url.into(),
                    headers: HeaderMap::new(),
                    body: Vec::new(),
                },
                Box::new(move |r| tx.send(r).unwrap()),
            );
            rx.recv().unwrap()
        };
        for url in [
            "shell://evil/api/x",
            "shell://apple/api/x",
            "shell://app.evil/api/x",
            "shell://app:8080/api/x",
            "shell://app@evil/api/x",
            "shell://evil/_native/info",
            "shell:///api/x",
            "shell:/api/x",
        ] {
            let r = send(url);
            assert_eq!(r.status, 404, "{url}");
            assert_eq!(json(&r)["error"], "not_found", "{url}");
        }
        assert!(echo.seen.lock().unwrap().is_empty());

        for url in [
            "shell://app",
            "shell://app/",
            "shell://app?x=1",
            "shell://app/api/x#y",
        ] {
            assert_eq!(send(url).status, 200, "{url}");
        }
        assert_eq!(echo.seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn a_foreign_origin_never_reaches_the_worker() {
        let (b, _, echo) = bridge();
        for origin in ["https://evil.example", "null", "shell://apple"] {
            let r = call(&b, "POST", "/api/sites", &[("origin", origin)], "{}");
            assert_eq!(r.status, 403, "{origin}");
            assert_eq!(json(&r)["error"], "cross_origin");
        }
        assert!(echo.seen.lock().unwrap().is_empty());
        assert_eq!(
            call(&b, "POST", "/api/sites", &[("origin", "shell://app")], "{}").status,
            200
        );
    }

    #[test]
    fn a_cross_site_fetch_never_reaches_the_worker() {
        let (b, _, echo) = bridge();
        // No `Origin`, as a browser sends none on a plain GET: only `Sec-Fetch-Site` gives it away.
        for value in ["cross-site", "Cross-Site", " CROSS-SITE "] {
            for method in ["GET", "POST"] {
                let r = call(&b, method, "/api/export", &[("sec-fetch-site", value)], "");
                assert_eq!(r.status, 403, "{method} {value}");
                assert_eq!(json(&r)["error"], "cross_origin");
            }
            let r = call(&b, "GET", "/_native/info", &[("sec-fetch-site", value)], "");
            assert_eq!(r.status, 403, "native {value}");
            assert_eq!(json(&r)["error"], "cross_origin");
        }
        assert!(echo.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn same_origin_same_site_none_and_absent_sec_fetch_site_pass() {
        let (b, _, echo) = bridge();
        for value in ["same-origin", "same-site", "none", "Same-Origin"] {
            assert_eq!(
                call(&b, "GET", "/api/x", &[("sec-fetch-site", value)], "").status,
                200,
                "{value}"
            );
        }
        assert_eq!(call(&b, "GET", "/api/x", &[], "").status, 200);
        assert_eq!(
            call(
                &b,
                "GET",
                "/_native/info",
                &[("sec-fetch-site", "same-origin")],
                ""
            )
            .status,
            200
        );
        assert_eq!(echo.seen.lock().unwrap().len(), 5);
    }

    /// Answers 200 with the headers it was built with.
    struct Sends(Vec<(&'static str, &'static str)>);

    impl Backend for Sends {
        fn submit(&self, _: Request, respond: Responder) {
            let mut r = Response::new(200);
            for (k, v) in &self.0 {
                http::append(&mut r.headers, k, *v);
            }
            respond(r);
        }
    }

    fn sending(headers: &[(&'static str, &'static str)]) -> Bridge {
        Bridge::new(
            "shell://app".into(),
            1 << 20,
            vec!["/api/".into()],
            Arc::new(FakeHost::default()),
            Arc::new(Sends(headers.to_vec())),
        )
    }

    const NO_FRAMING: &str = "frame-ancestors 'self'";

    #[test]
    fn the_apps_pages_cannot_be_framed_by_a_foreign_page() {
        let (b, _, _) = bridge();
        for path in ["/", "/index.html", "/api/x", "/img/logo.svg"] {
            let r = call(&b, "GET", path, &[], "");
            assert_eq!(
                r.headers["content-security-policy"],
                vec![NO_FRAMING],
                "{path}"
            );
        }
    }

    #[test]
    fn a_framing_policy_from_the_worker_is_left_alone() {
        let own: [&[(&str, &str)]; 5] = [
            &[("content-security-policy", "frame-ancestors *")],
            &[(
                "content-security-policy",
                "default-src 'self'; Frame-Ancestors https://a.example",
            )],
            &[(
                "content-security-policy",
                "default-src 'self', frame-ancestors 'none'",
            )],
            &[("x-frame-options", "DENY")],
            &[
                ("content-security-policy", "default-src 'self'"),
                ("x-frame-options", "SAMEORIGIN"),
            ],
        ];
        for headers in own {
            let r = call(&sending(headers), "GET", "/", &[], "");
            let csp: Vec<_> = headers
                .iter()
                .filter(|(k, _)| *k == "content-security-policy")
                .map(|(_, v)| v.to_string())
                .collect();
            assert_eq!(
                r.headers
                    .get("content-security-policy")
                    .cloned()
                    .unwrap_or_default(),
                csp,
                "{headers:?}"
            );
        }
    }

    #[test]
    fn a_policy_without_frame_ancestors_gets_ours_beside_it() {
        // Several policies are all enforced, so the worker's stays as it is and framing is refused.
        // A report-only policy enforces nothing, so it does not count as the worker's own.
        for (name, value) in [
            ("content-security-policy", "default-src 'self'"),
            (
                "content-security-policy",
                "default-src 'self'; script-src-elem 'self'",
            ),
            ("content-security-policy-report-only", "frame-ancestors *"),
        ] {
            let r = call(&sending(&[(name, value)]), "GET", "/", &[], "");
            let mut want = vec![NO_FRAMING];
            if name == "content-security-policy" {
                want.insert(0, value);
            }
            assert_eq!(
                r.headers["content-security-policy"], want,
                "{name}: {value}"
            );
        }
    }

    #[test]
    fn with_the_frame_guard_off_nothing_is_added() {
        let b = sending(&[]).with_frame_guard(false);
        let r = call(&b, "GET", "/", &[], "");
        assert!(!r.headers.contains_key("content-security-policy"));
    }

    #[test]
    fn with_the_frame_guard_off_a_foreign_frame_may_navigate_to_the_app_and_nothing_more() {
        const FRAME: [(&str, &str); 3] = [
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "iframe"),
        ];
        const IMAGE: [(&str, &str); 3] = [
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "no-cors"),
            ("sec-fetch-dest", "image"),
        ];
        let (on, _, _) = bridge();
        assert_eq!(call(&on, "GET", "/page.html", &FRAME, "").status, 403);

        let (off, _, echo) = bridge();
        let off = off.with_frame_guard(false);
        assert_eq!(call(&off, "GET", "/page.html", &FRAME, "").status, 200);
        let mut frame = FRAME;
        frame[2].1 = "frame";
        assert_eq!(call(&off, "GET", "/page.html", &frame, "").status, 200);
        assert_eq!(call(&off, "GET", "/api/export", &IMAGE, "").status, 403);
        // A navigation of the main frame (or a new window) is not a frame being framed.
        let mut document = FRAME;
        document[2].1 = "document";
        assert_eq!(call(&off, "GET", "/page.html", &document, "").status, 403);
        // A form POST carries a foreign `Origin`, refused whatever the guard says.
        let post = [
            FRAME[0],
            FRAME[1],
            FRAME[2],
            ("origin", "https://evil.example"),
        ];
        assert_eq!(call(&off, "POST", "/api/x", &post, "").status, 403);
        assert_eq!(echo.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn an_injected_path_header_is_stripped() {
        let (b, _, echo) = bridge();
        call(
            &b,
            "POST",
            "/api/dialog/open-file",
            &[("x-shell-file-path", "/etc/shadow")],
            "",
        );
        let seen = echo.seen.lock().unwrap();
        assert!(!seen[0].headers.contains_key(FILE_PATH_HEADER));
    }

    #[test]
    fn a_token_becomes_the_path_and_never_reaches_the_worker_itself() {
        let (b, _, echo) = bridge();
        let token = b
            .tokens
            .lock()
            .unwrap()
            .issue("/home/u/pic.png".into(), Kind::File);
        let r = call(
            &b,
            "POST",
            "/api/dialog/open-file",
            &[
                ("x-shell-file", &token),
                ("x-shell-file-path", "/etc/shadow"),
            ],
            "",
        );
        assert_eq!(r.status, 200);
        let seen = echo.seen.lock().unwrap();
        assert_eq!(seen[0].headers[FILE_PATH_HEADER], vec!["/home/u/pic.png"]);
        assert!(!seen[0].headers.contains_key(FILE_TOKEN_HEADER));
    }

    #[test]
    fn bad_tokens_are_refused_without_contacting_the_worker() {
        let (b, _, echo) = bridge();
        let stale = TokenStore::new().issue("/x".into(), Kind::File);
        let good = b.tokens.lock().unwrap().issue("/x".into(), Kind::File);
        let cases: [(&[(&str, &str)], &str); 3] = [
            (&[("x-shell-file", "forged")], "unknown_file_token"),
            (&[("x-shell-file", &stale)], "stale_file_token"),
            (
                &[("x-shell-file", &good), ("x-shell-file", &good)],
                "bad_file_token",
            ),
        ];
        for (headers, code) in cases {
            let r = call(&b, "POST", "/api/dialog/open-file", headers, "");
            assert_eq!(r.status, 400);
            assert_eq!(json(&r)["error"], code);
        }
        assert!(echo.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn the_clipboard_is_read_by_the_shell_and_handed_over_as_the_body() {
        let (b, host, echo) = bridge();
        *host.clipboard.lock().unwrap() = Some("Καλημέρα\nsecond line".into());
        call(
            &b,
            "GET",
            "/api/clipboard/text",
            &[("x-shell-clipboard", "read")],
            "page body",
        );
        {
            let seen = echo.seen.lock().unwrap();
            assert_eq!(seen[0].body, "Καλημέρα\nsecond line".as_bytes());
            assert_eq!(seen[0].headers[CLIPBOARD_HEADER], vec!["text"]);
        }

        // An empty clipboard is an empty body, still marked as the shell's.
        *host.clipboard.lock().unwrap() = None;
        call(
            &b,
            "GET",
            "/api/clipboard/text",
            &[("x-shell-clipboard", "read")],
            "",
        );
        let seen = echo.seen.lock().unwrap();
        assert!(seen[1].body.is_empty());
        assert_eq!(seen[1].headers[CLIPBOARD_HEADER], vec!["text"]);
    }

    #[test]
    fn a_page_cannot_forge_the_clipboard() {
        let (b, host, echo) = bridge();
        call(
            &b,
            "POST",
            "/api/clipboard/text",
            &[("x-shell-clipboard", "text")],
            "forged",
        );
        let seen = echo.seen.lock().unwrap();
        assert!(
            !seen[0].headers.contains_key(CLIPBOARD_HEADER),
            "the page's value is dropped"
        );
        assert_eq!(seen[0].body, b"forged");
        assert!(
            host.calls.lock().unwrap().is_empty(),
            "nothing asked for the clipboard"
        );
    }

    #[test]
    fn native_paths_never_reach_the_worker() {
        let (b, _, echo) = bridge();
        assert_eq!(call(&b, "GET", "/_native/nope", &[], "").status, 404);
        assert_eq!(call(&b, "GET", "/_native/info", &[], "").status, 200);
        assert!(echo.seen.lock().unwrap().is_empty());
    }
}
