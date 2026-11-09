//! Phase 4 spike (grafida/grafida#88): the shell owns the macOS webview through objc2.
//!
//! The same page (`../common/index.html`) and the same tests as the Phase 0 spikes (ADR 0001), on
//! WKWebView: the `boson://app` origin and secure context, POST bodies 0 B → 50 MiB checked by
//! length and SHA-256, repeated headers both ways, `Cache-Control` pass-through, the token round
//! trip, the UI during a slow request; then a macOS-only page (`/extra`) for what wry's handler
//! gets wrong: a request the page aborts (WebKit calls `stopURLSchemeTask:`, and a late answer
//! must never reach the stopped task) and the body *forms* WebKit hands over (`HTTPBody` or a
//! stream) for an ArrayBuffer, a string, a Blob and FormData.
//!
//! Everything WebKit-facing happens on the main thread. The "worker" answers from other threads,
//! and its answers are posted back through tao's event loop, exactly as the shell's worker pipe is.
//!
//! Env: SPIKE_AUTOPICK=<path> bypasses the dialog; SPIKE_QUIT_AFTER_REPORT=1 quits after /extra;
//! GRAFIDA_DEBUG=1 turns on the Web Inspector.

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained, runtime::ProtocolObject,
};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_foundation::{
    NSData, NSDictionary, NSHTTPURLResponse, NSInputStream, NSNumber, NSObject,
    NSObjectNSKeyValueCoding, NSObjectProtocol,
    NSString, NSURL, NSURLRequest, ns_string,
};
use objc2_web_kit::{WKURLSchemeHandler, WKURLSchemeTask, WKWebView, WKWebViewConfiguration};
use sha2::{Digest, Sha256};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::macos::WindowExtMacOS,
    window::WindowBuilder,
};

const INDEX: &str = include_str!("../../common/index.html");
const EXTRA: &str = include_str!("../extra.html");
/// The shell reads a body stream in chunks of this size, never "while bytes are available".
const CHUNK: usize = 1 << 20;

struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

enum UserEvent {
    Respond(u64, Response),
    Navigate(&'static str),
    Quit,
}

type Responder = Box<dyn FnOnce(Response) + Send>;

/// Counters printed with the report: what WebKit handed over, and what the shell did with it.
#[derive(Default)]
struct Stats {
    bodies_as_data: AtomicUsize,
    bodies_as_stream: AtomicUsize,
    stream_reads: AtomicUsize,
    completed_on_main: AtomicUsize,
    completed_off_main: AtomicUsize,
    stopped_tasks: AtomicUsize,
    late_answers_dropped: AtomicUsize,
    task_exceptions: AtomicUsize,
}

static STATS: std::sync::LazyLock<Stats> = std::sync::LazyLock::new(Stats::default);

thread_local! {
    /// Parked tasks, by our id. Main thread only.
    static TASKS: RefCell<HashMap<u64, Retained<ProtocolObject<dyn WKURLSchemeTask>>>> = RefCell::default();
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaSpikeSchemeHandler"]
    #[ivars = EventLoopProxy<UserEvent>]
    struct SchemeHandler;

    unsafe impl NSObjectProtocol for SchemeHandler {}

    unsafe impl WKURLSchemeHandler for SchemeHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
            let task: Retained<ProtocolObject<dyn WKURLSchemeTask>> = task.retain();
            let request = unsafe { task.request() };
            TASKS.with(|t| t.borrow_mut().insert(id, task));
            let proxy = self.ivars().clone();
            accept(id, &request, proxy);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            let ptr = task as *const _ as *const c_void;
            TASKS.with(|t| {
                let mut t = t.borrow_mut();
                let id = t
                    .iter()
                    .find(|(_, v)| Retained::as_ptr(v) as *const c_void == ptr)
                    .map(|(k, _)| *k);
                if let Some(id) = id {
                    t.remove(&id);
                    STATS.stopped_tasks.fetch_add(1, Ordering::SeqCst);
                    eprintln!("[stop] task {id}");
                }
            });
        }
    }
);

impl SchemeHandler {
    fn new(mtm: MainThreadMarker, proxy: EventLoopProxy<UserEvent>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(proxy);
        unsafe { msg_send![super(this), init] }
    }
}

/// Reads what WebKit handed over, on the main thread for `HTTPBody` (already in memory) and on a
/// reader thread for a stream, then hands the request to the "worker".
fn accept(id: u64, request: &NSURLRequest, proxy: EventLoopProxy<UserEvent>) {
    let url = request
        .URL()
        .and_then(|u| u.absoluteString())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let method = request
        .HTTPMethod()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "GET".into());
    let mut headers = Vec::new();
    if let Some(all) = request.allHTTPHeaderFields() {
        for key in all.allKeys().iter() {
            if let Some(value) = all.objectForKey(&key) {
                headers.push((key.to_string().to_ascii_lowercase(), value.to_string()));
            }
        }
    }
    let respond: Responder = Box::new(move |r| {
        let _ = proxy.send_event(UserEvent::Respond(id, r));
    });

    if let Some(data) = request.HTTPBody() {
        STATS.bodies_as_data.fetch_add(1, Ordering::SeqCst);
        let body = data.to_vec();
        std::thread::spawn(move || {
            handle(
                Request {
                    method,
                    url,
                    headers,
                    body,
                },
                respond,
            )
        });
    } else if let Some(stream) = request.HTTPBodyStream() {
        STATS.bodies_as_stream.fetch_add(1, Ordering::SeqCst);
        let stream = SendStream(stream);
        std::thread::spawn(move || {
            let body = stream.read_to_end();
            handle(
                Request {
                    method,
                    url,
                    headers,
                    body,
                },
                respond,
            )
        });
    } else {
        std::thread::spawn(move || {
            handle(
                Request {
                    method,
                    url,
                    headers,
                    body: Vec::new(),
                },
                respond,
            )
        });
    }
}

/// An `NSInputStream` handed to one reader thread, which alone uses it from then on.
struct SendStream(Retained<NSInputStream>);
// SAFETY: the stream is moved to exactly one thread and never touched by another afterwards.
unsafe impl Send for SendStream {}

impl SendStream {
    /// Blocking reads of `CHUNK` until `read` answers 0 (end of stream) or < 0 (an error).
    fn read_to_end(self) -> Vec<u8> {
        let stream = self.0;
        stream.open();
        let mut out = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = unsafe { stream.read_maxLength(NonNull::new(buf.as_mut_ptr()).unwrap(), buf.len()) };
            STATS.stream_reads.fetch_add(1, Ordering::SeqCst);
            match n {
                0 => break,
                n if n < 0 => {
                    eprintln!("[stream] read error: {:?}", stream.streamError());
                    break;
                }
                n => out.extend_from_slice(&buf[..n as usize]),
            }
        }
        stream.close();
        out
    }
}

/// Main thread: answer the task, unless WebKit has stopped it meanwhile.
fn complete(id: u64, response: Response) {
    if MainThreadMarker::new().is_some() {
        STATS.completed_on_main.fetch_add(1, Ordering::SeqCst);
    } else {
        STATS.completed_off_main.fetch_add(1, Ordering::SeqCst);
    }
    let Some(task) = TASKS.with(|t| t.borrow_mut().remove(&id)) else {
        STATS.late_answers_dropped.fetch_add(1, Ordering::SeqCst);
        eprintln!("[late] answer for stopped task {id} dropped");
        return;
    };
    let request = unsafe { task.request() };
    let Some(url) = request.URL() else { return };

    // Foundation's header fields are a dictionary: a repeated header is joined with ", ", which
    // is how fetch() shows it on every platform anyway.
    let mut merged: Vec<(String, String)> = Vec::new();
    for (k, v) in response.headers {
        match merged.iter_mut().find(|(mk, _)| mk.eq_ignore_ascii_case(&k)) {
            Some((_, mv)) => {
                mv.push_str(", ");
                mv.push_str(&v);
            }
            None => merged.push((k, v)),
        }
    }
    merged.push(("content-length".into(), response.body.len().to_string()));
    let keys: Vec<Retained<NSString>> = merged.iter().map(|(k, _)| NSString::from_str(k)).collect();
    let values: Vec<Retained<NSString>> = merged.iter().map(|(_, v)| NSString::from_str(v)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let value_refs: Vec<&NSString> = values.iter().map(|v| &**v).collect();
    let fields = NSDictionary::from_slices(&key_refs, &value_refs);

    let http = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
        NSHTTPURLResponse::alloc(),
        &url,
        response.status as isize,
        Some(ns_string!("HTTP/1.1")),
        Some(&fields),
    );
    let Some(http) = http else { return };
    let data = NSData::with_bytes(&response.body);
    let result = objc2::exception::catch(std::panic::AssertUnwindSafe(|| unsafe {
        task.didReceiveResponse(&http);
        task.didReceiveData(&data);
        task.didFinish();
    }));
    if let Err(e) = result {
        STATS.task_exceptions.fetch_add(1, Ordering::SeqCst);
        eprintln!("[exception] task {id}: {e:?}");
    }
}

fn json(status: u16, v: serde_json::Value) -> Response {
    Response {
        status,
        headers: vec![
            ("content-type".into(), "application/json".into()),
            ("cache-control".into(), "no-store".into()),
        ],
        body: v.to_string().into_bytes(),
    }
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

type Tokens = Mutex<HashMap<String, std::path::PathBuf>>;
static TOKENS: std::sync::LazyLock<Tokens> = std::sync::LazyLock::new(Tokens::default);

fn stats() -> serde_json::Value {
    let g = |a: &AtomicUsize| a.load(Ordering::SeqCst);
    serde_json::json!({
        "bodiesAsData": g(&STATS.bodies_as_data),
        "bodiesAsStream": g(&STATS.bodies_as_stream),
        "streamReads": g(&STATS.stream_reads),
        "completedOnMain": g(&STATS.completed_on_main),
        "completedOffMain": g(&STATS.completed_off_main),
        "stoppedTasks": g(&STATS.stopped_tasks),
        "lateAnswersDropped": g(&STATS.late_answers_dropped),
        "taskExceptions": g(&STATS.task_exceptions),
    })
}

/// The "worker": runs on a non-main thread, as the real one is a separate process.
fn handle(req: Request, respond: Responder) {
    let (path, query) = {
        let rest = req.url.strip_prefix("boson://app").unwrap_or(&req.url);
        match rest.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (rest.to_string(), String::new()),
        }
    };
    let header = |n: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
    };
    eprintln!("[req] {} {} body={}", req.method, req.url, req.body.len());

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") => respond(Response {
            status: 200,
            headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
            body: INDEX.as_bytes().to_vec(),
        }),
        ("GET", "/extra") => respond(Response {
            status: 200,
            headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
            body: EXTRA.as_bytes().to_vec(),
        }),
        ("POST", "/api/echo") => respond(json(
            200,
            serde_json::json!({"bytes": req.body.len(), "sha256": sha(&req.body), "contentType": header("content-type")}),
        )),
        ("GET", "/api/headers") => respond(Response {
            status: 200,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("cache-control".into(), "no-store".into()),
                ("x-multi".into(), "one".into()),
                ("x-multi".into(), "two".into()),
            ],
            body: b"{}".to_vec(),
        }),
        (_, "/api/request-headers") => respond(json(
            200,
            serde_json::json!({"headers": req.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>(), "bodyBytes": req.body.len()}),
        )),
        ("GET", "/api/slow") => {
            let ms: u64 = query
                .strip_prefix("ms=")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3000);
            std::thread::sleep(Duration::from_millis(ms));
            respond(json(200, serde_json::json!({"slept": ms})));
        }
        ("POST", "/_native/dialog/open-file") => {
            let picked = match std::env::var_os("SPIKE_AUTOPICK") {
                Some(p) => Some(std::path::PathBuf::from(p)),
                None => pollster::block_on(
                    rfd::AsyncFileDialog::new()
                        .set_title("Spike: pick a file")
                        .pick_file(),
                )
                .map(|h| h.path().to_path_buf()),
            };
            let Some(p) = picked else {
                respond(json(200, serde_json::json!({"cancelled": true})));
                return;
            };
            let token = format!(
                "t-{}",
                &sha(format!("{}{:?}", p.display(), std::time::SystemTime::now()).as_bytes())[..32]
            );
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            TOKENS.lock().unwrap().insert(token.clone(), p);
            respond(json(
                200,
                serde_json::json!({"files": [{"token": token, "name": name, "size": size}]}),
            ));
        }
        ("POST", "/api/read-file") => {
            let injected = header("x-shell-file-path").is_some();
            let resolved = header("x-shell-file").and_then(|t| TOKENS.lock().unwrap().get(&t).cloned());
            match resolved {
                None => respond(json(
                    400,
                    serde_json::json!({"error": "no_file", "strippedInjectedPath": injected}),
                )),
                Some(p) => match std::fs::read(&p) {
                    Ok(b) => respond(json(
                        200,
                        serde_json::json!({"path": p, "bytes": b.len(), "sha256": sha(&b)}),
                    )),
                    Err(e) => respond(json(500, serde_json::json!({"error": e.to_string()}))),
                },
            }
        }
        ("POST", "/report") => {
            println!("REPORT {}", String::from_utf8_lossy(&req.body));
            respond(json(200, serde_json::json!({"ok": true})));
            post_from_any_thread(UserEvent::Navigate("boson://app/extra"));
        }
        ("POST", "/report-extra") => {
            // Give a late answer time to arrive (and be dropped) before the counters are read.
            std::thread::sleep(Duration::from_millis(1500));
            let mut v: serde_json::Value =
                serde_json::from_slice(&req.body).unwrap_or(serde_json::Value::Null);
            v["shell"] = stats();
            v["macos"] = serde_json::Value::String(macos_version());
            println!("REPORT2 {v}");
            respond(json(200, serde_json::json!({"ok": true})));
            if std::env::var_os("SPIKE_QUIT_AFTER_REPORT").is_some() {
                post_from_any_thread(UserEvent::Quit);
            }
        }
        _ => respond(json(404, serde_json::json!({"error": "not_found", "path": path}))),
    }
}

static GLOBAL_PROXY: Mutex<Option<EventLoopProxy<UserEvent>>> = Mutex::new(None);

fn post_from_any_thread(event: UserEvent) {
    if let Some(p) = &*GLOBAL_PROXY.lock().unwrap() {
        let _ = p.send_event(event);
    }
}

fn macos_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn load(webview: &WKWebView, url: &str) {
    let url = NSURL::URLWithString(&NSString::from_str(url)).expect("URL");
    let request = NSURLRequest::requestWithURL(&url);
    unsafe { webview.loadRequest(&request) };
}

fn main() {
    let mtm = MainThreadMarker::new().expect("main thread");
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    *GLOBAL_PROXY.lock().unwrap() = Some(proxy.clone());

    let window = WindowBuilder::new()
        .with_title("Shell spike (objc2)")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 860.0))
        .build(&event_loop)
        .unwrap();

    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    let handler = SchemeHandler::new(mtm, proxy.clone());
    unsafe {
        config.setURLSchemeHandler_forURLScheme(
            Some(ProtocolObject::from_ref(&*handler)),
            ns_string!("boson"),
        );
    }
    if std::env::var_os("GRAFIDA_DEBUG").is_some() {
        let prefs = unsafe { config.preferences() };
        let yes = NSNumber::new_bool(true);
        unsafe { prefs.setValue_forKey(Some(&yes), ns_string!("developerExtrasEnabled")) };
    }

    let content: &NSView = unsafe { &*(window.ns_view() as *const NSView) };
    let webview = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), content.bounds(), &config)
    };
    webview.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    content.addSubview(&webview);
    load(&webview, "boson://app/");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let _keep = (&handler, &config);
        match event {
            Event::UserEvent(UserEvent::Respond(id, response)) => complete(id, response),
            Event::UserEvent(UserEvent::Navigate(url)) => load(&webview, url),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(UserEvent::Quit) => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    });
}
