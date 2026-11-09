// Phase 0 spike (grafida/grafida#82): Rust + wry/tao/rfd on Linux.
//
// Tests: (a) boson://app origin + large POST bodies, (b) _native/dialog/open-file token round trip,
// (c) idle CPU (measured externally), plus a slow request that must not block the UI thread.
// Env: SPIKE_AUTOPICK=<path> bypasses the native dialog so (b) can run unattended.

use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use sha2::{Digest, Sha256};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::WindowBuilder,
};
use wry::{
    http::{header::CONTENT_TYPE, Request, Response},
    NewWindowResponse, WebViewBuilder,
};

#[cfg(windows)]
mod win;

/// Platform-neutral responder: on Linux/macOS it wraps wry's, on Windows it posts to the UI thread.
pub type Responder = Box<dyn FnOnce(Response<Cow<'static, [u8]>>) + Send>;

const INDEX: &str = include_str!("../../common/index.html");

pub type Tokens = Arc<Mutex<HashMap<String, std::path::PathBuf>>>;

pub enum UserEvent {
    Quit,
    #[allow(dead_code)]
    Respond(u64, Response<Cow<'static, [u8]>>),
}

fn json(status: u16, v: serde_json::Value) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .header("cache-control", "no-store")
        .body(Cow::Owned(v.to_string().into_bytes()))
        .unwrap()
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn handle(req: Request<Vec<u8>>, responder: Responder, tokens: Tokens, seq: Arc<AtomicU64>, proxy: tao::event_loop::EventLoopProxy<UserEvent>) {
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let method = req.method().as_str().to_string();
    eprintln!("[req] {method} {uri} body={}", req.body().len());

    match (method.as_str(), path.as_str()) {
        ("GET", "/") => responder(
            Response::builder()
                .header(CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Cow::Borrowed(INDEX.as_bytes()))
                .unwrap(),
        ),
        ("POST", "/api/echo") => {
            let b = req.body();
            responder(json(200, serde_json::json!({"bytes": b.len(), "sha256": sha(b)})));
        }
        ("GET", "/api/headers") => responder(
            Response::builder()
                .header(CONTENT_TYPE, "application/json")
                .header("cache-control", "no-store")
                .header("x-multi", "one")
                .header("x-multi", "two")
                .body(Cow::Borrowed(&b"{}"[..]))
                .unwrap(),
        ),
        ("GET", "/api/slow") => {
            let ms: u64 = uri
                .query()
                .and_then(|q| q.strip_prefix("ms="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(3000);
            // Off the UI thread, exactly as the worker pipe will be.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                responder(json(200, serde_json::json!({"slept": ms})));
            });
        }
        ("POST", "/_native/dialog/open-file") => {
            std::thread::spawn(move || {
                let picked = match std::env::var_os("SPIKE_AUTOPICK") {
                    Some(p) => Some(std::path::PathBuf::from(p)),
                    None => pollster::block_on(rfd::AsyncFileDialog::new().set_title("Spike: pick a file").pick_file())
                        .map(|h| h.path().to_path_buf()),
                };
                let Some(p) = picked else {
                    responder(json(200, serde_json::json!({"cancelled": true})));
                    return;
                };
                let n = seq.fetch_add(1, Ordering::SeqCst);
                let token = format!("t{n}-{}", &sha(format!("{}{n}{:?}", p.display(), std::time::SystemTime::now()).as_bytes())[..16]);
                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                tokens.lock().unwrap().insert(token.clone(), p);
                responder(json(200, serde_json::json!({"files": [{"token": token, "name": name, "size": size}]})));
            });
        }
        ("POST", "/api/read-file") => {
            // Shell side of the token flow: strip any X-Shell-File-Path the page sent, resolve the
            // token, forward the real path. The "worker" here is inline.
            let mut headers = req.headers().clone();
            let injected = headers.remove("x-shell-file-path").is_some();
            let token = headers.get("x-shell-file").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let resolved = token.and_then(|t| tokens.lock().unwrap().get(&t).cloned());
            match resolved {
                None => responder(json(400, serde_json::json!({"error": "no_file", "strippedInjectedPath": injected}))),
                Some(p) => match std::fs::read(&p) {
                    Ok(b) => responder(json(200, serde_json::json!({"path": p, "bytes": b.len(), "sha256": sha(&b)}))),
                    Err(e) => responder(json(500, serde_json::json!({"error": e.to_string()}))),
                },
            }
        }
        ("POST", "/report") => {
            println!("REPORT {}", String::from_utf8_lossy(req.body()));
            responder(json(200, serde_json::json!({"ok": true})));
            if std::env::var_os("SPIKE_QUIT_AFTER_REPORT").is_some() {
                let _ = proxy.send_event(UserEvent::Quit);
            }
        }
        _ => responder(json(404, serde_json::json!({"error": "not_found", "path": path}))),
    }
}

fn main() -> wry::Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Shell spike (wry)")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 860.0))
        .build(&event_loop)
        .unwrap();

    let tokens: Tokens = Arc::default();
    let seq = Arc::new(AtomicU64::new(1));

    let mut builder = WebViewBuilder::new();
    #[cfg(not(windows))]
    {
        let proxy = proxy.clone();
        builder = builder.with_asynchronous_custom_protocol("boson".into(), move |_id, req, r| {
            handle(req, Box::new(move |resp| r.respond(resp)), tokens.clone(), seq.clone(), proxy.clone())
        });
    }
    #[cfg(windows)]
    let env = {
        use wry::WebViewBuilderExtWindows;
        let env = win::create_environment().expect("WebView2 environment");
        builder = builder.with_environment(env.clone());
        env
    };
    let builder = builder
        .with_navigation_handler(|url| {
            let ok = url.starts_with("boson://app/");
            if !ok {
                eprintln!("[nav] blocked {url} -> would open in OS browser");
            }
            ok
        })
        .with_new_window_req_handler(|url, _| {
            eprintln!("[new-window] blocked {url} -> would open in OS browser");
            NewWindowResponse::Deny
        })
        .with_devtools(std::env::var_os("GRAFIDA_DEBUG").is_some())
        .with_url("boson://app/");

    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().unwrap())?
    };
    #[cfg(not(target_os = "linux"))]
    let _webview = builder.build(&window)?;

    #[cfg(windows)]
    {
        use wry::WebViewExtWindows;
        win::attach_scheme_handler(&_webview.webview(), env, tokens.clone(), seq.clone(), proxy.clone())
            .expect("attach boson:// handler");
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Quit) => {
                *control_flow = ControlFlow::Exit
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::Respond(id, resp)) => win::complete(id, resp),
            _ => {}
        }
    });
}
