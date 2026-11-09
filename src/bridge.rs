//! The platform-neutral half of the scheme handler: everything that happens to a request between
//! the webview handing it over and the worker (or the shell itself) answering it.
//!
//! The platform layer turns a webview request into a [`Request`], calls [`Bridge::handle`] on the
//! UI thread, and completes the webview request from the [`Responder`] — which may be called on any
//! thread, so the platform layer posts it back to the UI thread through its event loop. Nothing in
//! here blocks.
//!
//! In order, for every request:
//! 1. a request carrying a foreign `Origin` is refused (`403 cross_origin`), so a page embedded in
//!    the app (an article's `<iframe>`) cannot drive the app's API;
//! 2. `/_native/*` is answered by the shell ([`crate::native`]) and never reaches the worker;
//! 3. any `x-shell-file-path` the page sent is dropped, and an `x-shell-file` token is resolved
//!    into `x-shell-file-path` (`400` for an unknown or stale token, without contacting the worker);
//! 4. the request goes to the worker, and on the way back `cache-control: no-store` is added under
//!    the manifest's `noStorePaths` when the worker sent no cache header of its own.
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
    supervisor::{Responder, Supervisor},
    tokens::TokenStore,
};

pub const FILE_TOKEN_HEADER: &str = "x-shell-file";
pub const FILE_PATH_HEADER: &str = "x-shell-file-path";

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
        }
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

        let no_store = self
            .no_store_paths
            .iter()
            .any(|prefix| request.path().starts_with(prefix.as_str()));

        self.backend.submit(
            request,
            Box::new(move |mut response| {
                if no_store
                    && !response.headers.contains_key("cache-control")
                    && !response.headers.contains_key("expires")
                {
                    http::append(&mut response.headers, "cache-control", "no-store");
                }
                respond(response);
            }),
        );
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
        fn webview_version(&self) -> String {
            "TestKit 1.0".into()
        }
    }

    pub fn bridge() -> (Bridge, Arc<FakeHost>, Arc<Echo>) {
        let host = Arc::new(FakeHost::default());
        let echo = Arc::new(Echo::default());
        let bridge = Bridge::new(
            "boson://app".into(),
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
                url: format!("boson://app{path}"),
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
            "boson://app".into(),
            1 << 20,
            vec!["/api/".into()],
            Arc::new(FakeHost::default()),
            Arc::new(Cached),
        );
        let r = call(&b, "GET", "/api/x", &[], "");
        assert_eq!(r.headers["cache-control"], vec!["max-age=60", "private"]);
    }

    #[test]
    fn a_foreign_origin_never_reaches_the_worker() {
        let (b, _, echo) = bridge();
        for origin in ["https://evil.example", "null", "boson://apple"] {
            let r = call(&b, "POST", "/api/sites", &[("origin", origin)], "{}");
            assert_eq!(r.status, 403, "{origin}");
            assert_eq!(json(&r)["error"], "cross_origin");
        }
        assert!(echo.seen.lock().unwrap().is_empty());
        assert_eq!(
            call(&b, "POST", "/api/sites", &[("origin", "boson://app")], "{}").status,
            200
        );
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
    fn native_paths_never_reach_the_worker() {
        let (b, _, echo) = bridge();
        assert_eq!(call(&b, "GET", "/_native/nope", &[], "").status, 404);
        assert_eq!(call(&b, "GET", "/_native/info", &[], "").status, 200);
        assert!(echo.seen.lock().unwrap().is_empty());
    }
}
