//! The macOS scheme bridge: `WKURLSchemeHandler` ⇄ [`crate::http`] (ADR 0001, grafida/grafida#88).
//!
//! The handler is registered on the shell's own `WKWebViewConfiguration`. Every task is parked in a
//! main-thread map under an id. A body WebKit hands over as `HTTPBody` is checked against
//! `maxBodyBytes` before a byte of it is copied; one handed over as `HTTPBodyStream` is read **to
//! EOF in bounded chunks on a reader thread** (never "while bytes are available"), and reading stops
//! past the limit → `413`. The request then goes through [`Bridge::handle`] on the main thread, and
//! the answer comes back through the event loop and completes the task on the main thread.
//!
//! `webView:stopURLSchemeTask:` removes the task from the map, so a late answer finds nothing and is
//! dropped: touching a stopped task raises an Objective-C exception.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ptr::NonNull,
    rc::Rc,
    sync::Mutex,
};

use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class,
    msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_foundation::{
    NSData, NSDictionary, NSHTTPURLResponse, NSInputStream, NSObject, NSObjectProtocol, NSString,
    NSURLRequest, ns_string,
};
use objc2_web_kit::{WKFrameInfo, WKURLSchemeHandler, WKURLSchemeTask, WKWebView};
use tao::event_loop::EventLoopProxy;

use super::UserEvent;
use crate::{
    bridge::Bridge,
    http::{self, Request, Response},
    log::Log,
    navigation,
    protocol::HeaderMap,
};

/// Bytes asked of a body stream per read.
const CHUNK: usize = 1 << 20;

type Task = Retained<ProtocolObject<dyn WKURLSchemeTask>>;

/// What the main thread keeps for the scheme handler.
pub struct Scheme {
    pub bridge: Bridge,
    pending: RefCell<HashMap<u64, Task>>,
    next_id: Cell<u64>,
    proxy: Mutex<EventLoopProxy<UserEvent>>,
    log: std::sync::Arc<Log>,
    /// The "no `_frame`" note has been logged.
    frame_unknown_noted: Cell<bool>,
}

impl Scheme {
    pub fn new(
        bridge: Bridge,
        proxy: EventLoopProxy<UserEvent>,
        log: std::sync::Arc<Log>,
    ) -> Rc<Scheme> {
        Rc::new(Scheme {
            bridge,
            pending: RefCell::default(),
            next_id: Cell::new(1),
            proxy: Mutex::new(proxy),
            log,
            frame_unknown_noted: Cell::new(false),
        })
    }

    /// `webView:startURLSchemeTask:`.
    fn start(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        // SAFETY: the task is alive for the duration of this call; we retain it.
        let request: Retained<NSURLRequest> = unsafe { task.request() };
        self.pending.borrow_mut().insert(id, task.retain());

        let url = request
            .URL()
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let method = request
            .HTTPMethod()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "GET".into());
        if !self.frame_may_request(task) {
            self.log.line(
                "shell",
                &format!("{method} {url} from a foreign frame refused"),
            );
            self.finish(id, Response::error(403, "cross_origin"));
            return;
        }
        let mut headers = HeaderMap::new();
        if let Some(all) = request.allHTTPHeaderFields() {
            for key in all.allKeys().iter() {
                if let Some(value) = all.objectForKey(&key) {
                    // Foundation has already joined a repeated header with ", ".
                    http::append(&mut headers, &key.to_string(), value.to_string());
                }
            }
        }
        let limit = self.bridge.max_body_bytes();

        if let Some(data) = request.HTTPBody() {
            if data.length() as u64 > limit {
                self.finish(id, self.bridge.too_large());
                return;
            }
            let body = data.to_vec();
            self.handle(
                id,
                Request {
                    method,
                    url,
                    headers,
                    body,
                },
            );
        } else if let Some(stream) = request.HTTPBodyStream() {
            let stream = SendStream(stream);
            let proxy = self.proxy.lock().unwrap().clone();
            let spawned = std::thread::Builder::new()
                .name("scheme-body".into())
                .spawn(move || {
                    let event = match stream.read_to_end(limit) {
                        Ok(body) => UserEvent::Accept(
                            id,
                            Request {
                                method,
                                url,
                                headers,
                                body,
                            },
                        ),
                        Err(ReadError::TooLarge) => {
                            UserEvent::Respond(id, Response::error(413, "body_too_large"))
                        }
                        Err(ReadError::Io(e)) => UserEvent::Respond(
                            id,
                            Response::json(
                                400,
                                &serde_json::json!({"error": "bad_body", "detail": e}),
                            ),
                        ),
                    };
                    let _ = proxy.send_event(event);
                });
            if let Err(e) = spawned {
                self.finish(
                    id,
                    Response::json(
                        500,
                        &serde_json::json!({"error": "bad_body", "detail": e.to_string()}),
                    ),
                );
            }
        } else {
            self.handle(
                id,
                Request {
                    method,
                    url,
                    headers,
                    body: Vec::new(),
                },
            );
        }
    }

    /// Whether the frame that made `task`'s request may have the app's URLs
    /// ([`navigation::frame_may_request`]): the main frame, or a subframe of the app's origin. A
    /// foreign embed's `<img src>` or `no-cors` fetch of `<scheme>://app/…` carries no `Origin`,
    /// and WKWebView adds no `Sec-Fetch-Site`, so [`Bridge`] cannot tell it from the app's own
    /// (security.md M1); the frame can. The public `WKURLSchemeTask` says nothing about who asked:
    /// this reads WebKit's private `_frame` (a `WKFrameInfo`, there since macOS 10.15) behind
    /// `respondsToSelector:`. Should it ever go, every request is let through and the log says so
    /// once, rather than the app going dark.
    fn frame_may_request(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) -> bool {
        let object: &AnyObject = task.as_ref();
        // SAFETY: plain selector queries on a live task; `_frame` answers nil or a `WKFrameInfo`.
        let frame: Option<Retained<WKFrameInfo>> = unsafe {
            let responds: bool = msg_send![object, respondsToSelector: sel!(_frame)];
            if responds {
                msg_send![object, _frame]
            } else {
                None
            }
        };
        let Some(frame) = frame else {
            if !self.frame_unknown_noted.replace(true) {
                self.log.line(
                    "shell",
                    "WebKit names no frame for scheme requests: foreign-frame requests cannot be refused",
                );
            }
            return true;
        };
        // SAFETY: the frame info is ours, retained; its origin is a plain value object.
        let (is_main, protocol, host) = unsafe {
            let origin = frame.securityOrigin();
            (
                frame.isMainFrame(),
                origin.protocol().to_string(),
                origin.host().to_string(),
            )
        };
        navigation::frame_may_request(is_main, &protocol, &host, &self.bridge.origin)
    }

    /// Hands a request whose body is read to the bridge. Main thread only.
    pub fn handle(&self, id: u64, request: Request) {
        if !self.pending.borrow().contains_key(&id) {
            return; // stopped while its body was being read
        }
        let proxy = Mutex::new(self.proxy.lock().unwrap().clone());
        self.bridge.handle(
            request,
            Box::new(move |response| {
                let _ = proxy
                    .lock()
                    .unwrap()
                    .send_event(UserEvent::Respond(id, response));
            }),
        );
    }

    /// `webView:stopURLSchemeTask:`: forget the task, so nothing is ever sent to it again.
    fn stop(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        let ptr: *const ProtocolObject<dyn WKURLSchemeTask> = task;
        self.pending
            .borrow_mut()
            .retain(|_, t| !std::ptr::eq(Retained::as_ptr(t), ptr));
    }

    /// Completes a parked task. Main thread only. A task WebKit has stopped is gone from the map,
    /// and its answer is dropped.
    pub fn finish(&self, id: u64, response: Response) {
        let Some(task) = self.pending.borrow_mut().remove(&id) else {
            return;
        };
        // SAFETY: the task is retained and has not been stopped (it would be gone from the map).
        let request = unsafe { task.request() };
        let Some(url) = request.URL() else { return };

        let Response {
            status,
            headers,
            body,
        } = response;
        // Foundation's header fields are a dictionary: repeated values are joined with ", ", which
        // is how fetch() shows them on every platform anyway. The body length is B; the worker's
        // own framing headers would contradict it.
        let mut names: Vec<Retained<NSString>> = Vec::new();
        let mut values: Vec<Retained<NSString>> = Vec::new();
        for (name, list) in &headers {
            if name == "content-length" || name == "transfer-encoding" {
                continue;
            }
            names.push(NSString::from_str(name));
            values.push(NSString::from_str(&list.join(", ")));
        }
        names.push(NSString::from_str("content-length"));
        values.push(NSString::from_str(&body.len().to_string()));
        let name_refs: Vec<&NSString> = names.iter().map(|n| &**n).collect();
        let value_refs: Vec<&NSString> = values.iter().map(|v| &**v).collect();
        let fields = NSDictionary::from_slices(&name_refs, &value_refs);

        let Some(reply) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
            NSHTTPURLResponse::alloc(),
            &url,
            status as isize,
            Some(ns_string!("HTTP/1.1")),
            Some(&fields),
        ) else {
            return;
        };
        let data = NSData::with_bytes(&body);
        // A task WebKit has already given up on raises; never let that unwind into AppKit.
        let sent = objc2::exception::catch(std::panic::AssertUnwindSafe(|| unsafe {
            task.didReceiveResponse(&reply);
            task.didReceiveData(&data);
            task.didFinish();
        }));
        if let Err(e) = sent {
            self.log.line(
                "shell",
                &format!("a scheme task raised when answered: {e:?}"),
            );
        }
    }
}

enum ReadError {
    TooLarge,
    Io(String),
}

/// A body stream handed to one reader thread, which alone uses it from then on.
struct SendStream(Retained<NSInputStream>);
// SAFETY: the stream is moved to exactly one thread and never touched by another afterwards.
unsafe impl Send for SendStream {}

impl SendStream {
    /// Blocking reads of `CHUNK` until `read` answers 0 (end of stream) or < 0 (an error).
    fn read_to_end(self, limit: u64) -> Result<Vec<u8>, ReadError> {
        let stream = self.0;
        stream.open();
        let mut body = Vec::new();
        let mut buf = vec![0u8; CHUNK];
        let result = loop {
            // SAFETY: `buf` is valid for `buf.len()` bytes.
            let n = unsafe {
                stream.read_maxLength(NonNull::new(buf.as_mut_ptr()).unwrap(), buf.len())
            };
            match n {
                0 => break Ok(body),
                n if n < 0 => {
                    break Err(ReadError::Io(
                        stream
                            .streamError()
                            .map(|e| e.localizedDescription().to_string())
                            .unwrap_or_else(|| "read error".into()),
                    ));
                }
                n => {
                    let n = n as usize;
                    if body.len() as u64 + n as u64 > limit {
                        break Err(ReadError::TooLarge);
                    }
                    body.extend_from_slice(&buf[..n]);
                }
            }
        };
        stream.close();
        result
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivar is only touched on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaShellSchemeHandler"]
    #[ivars = Rc<Scheme>]
    pub struct SchemeHandler;

    unsafe impl NSObjectProtocol for SchemeHandler {}

    unsafe impl WKURLSchemeHandler for SchemeHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.ivars().start(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.ivars().stop(task);
        }
    }
);

impl SchemeHandler {
    pub fn new(mtm: MainThreadMarker, scheme: Rc<Scheme>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(scheme);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}
