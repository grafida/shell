//! The Windows scheme bridge: WebView2 `WebResourceRequested` ⇄ [`crate::http`].
//!
//! The scheme is a real WebView2 custom scheme, registered on the environment options before the
//! environment exists (`CoreWebView2CustomSchemeRegistration`, ADR 0001): the page's origin is
//! `<scheme>://app`, never wry's `http://<scheme>.localhost`. The shell filters `<scheme>://*` for
//! every resource context and request source kind, takes a deferral, and parks the event args in a
//! UI-thread map under an id.
//!
//! The body is read **to EOF in bounded chunks on a reader thread** (the Phase 0 spike read it on
//! the UI thread: 50 MiB in 865 ms with the window frozen), through an agile reference to the
//! request's `IStream`; reading stops past `maxBodyBytes` → `413`. The reader posts the body back to
//! the UI thread, which hands the request to [`Bridge::handle`]; the answer is posted back again
//! and completed from the map, on the UI thread, where the deferral was taken.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
        ICoreWebView2, ICoreWebView2_22, ICoreWebView2Deferral, ICoreWebView2Environment,
        ICoreWebView2WebResourceRequest, ICoreWebView2WebResourceRequestedEventArgs,
    },
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::{
    Win32::{
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IStream},
        UI::Shell::SHCreateMemStream,
    },
    core::{HSTRING, Interface, PWSTR},
};
use windows_core::{AgileReference, BOOL};

use crate::{
    bridge::Bridge,
    http::{self, Request, Response},
    protocol::HeaderMap,
};

/// Bytes asked of the body stream per read.
const CHUNK: usize = 1 << 20;

/// What the scheme posts back to the UI thread.
pub enum SchemeEvent {
    /// A request's body has been read (or could not be).
    Body(u64, Result<Vec<u8>, BodyError>),
    /// A request's answer.
    Respond(u64, Response),
}

pub enum BodyError {
    TooLarge,
    Io(String),
}

pub type Poster = Arc<dyn Fn(SchemeEvent) + Send + Sync>;

struct Pending {
    args: ICoreWebView2WebResourceRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
    /// The request, until its body has been read.
    request: Option<Request>,
}

/// What the UI thread keeps for the scheme handler.
pub struct Scheme {
    pub bridge: Bridge,
    env: ICoreWebView2Environment,
    pending: RefCell<HashMap<u64, Pending>>,
    next_id: Cell<u64>,
    post: Poster,
}

impl Scheme {
    pub fn new(bridge: Bridge, env: ICoreWebView2Environment, post: Poster) -> Rc<Scheme> {
        Rc::new(Scheme {
            bridge,
            env,
            pending: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            post,
        })
    }

    /// Adds the filter and the handler to the webview.
    pub fn install(
        self: &Rc<Scheme>,
        webview: &ICoreWebView2,
        scheme: &str,
    ) -> windows::core::Result<()> {
        let filter = HSTRING::from(format!("{scheme}://*"));
        // SAFETY: COM calls on the UI thread, on a live webview.
        unsafe {
            // ICoreWebView2_22 covers every request source (shared workers, iframes); older
            // runtimes get the document-only filter (WebView2Feedback#1114).
            match webview.cast::<ICoreWebView2_22>() {
                Ok(w) => w.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    &filter,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )?,
                Err(_) => webview.AddWebResourceRequestedFilter(
                    &filter,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                )?,
            }
            let this = self.clone();
            let mut token = Default::default();
            webview.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        this.accept(args);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(())
    }

    /// One request, on the UI thread.
    fn accept(self: &Rc<Scheme>, args: ICoreWebView2WebResourceRequestedEventArgs) {
        // SAFETY: COM calls on the UI thread, on the live event args.
        let parsed = unsafe {
            (|| -> windows::core::Result<(Request, Option<IStream>, ICoreWebView2Deferral)> {
                let request = args.Request()?;
                let (parsed, body) = read_head(&request)?;
                Ok((parsed, body, args.GetDeferral()?))
            })()
        };
        let Ok((request, body, deferral)) = parsed else {
            return; // nothing parked: WebView2 answers the request itself
        };
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        self.pending.borrow_mut().insert(
            id,
            Pending {
                args,
                deferral,
                request: Some(request),
            },
        );

        let Some(stream) = body else {
            self.body_read(id, Ok(Vec::new()));
            return;
        };
        let limit = self.bridge.max_body_bytes();
        let post = self.post.clone();
        match AgileReference::new(&stream) {
            Ok(agile) => {
                let spawned = std::thread::Builder::new()
                    .name("scheme-body".into())
                    .spawn(move || post(SchemeEvent::Body(id, read_body(&agile, limit))));
                if let Err(e) = spawned {
                    self.body_read(id, Err(BodyError::Io(e.to_string())));
                }
            }
            // No agile reference: read here, on the UI thread, rather than not at all.
            Err(_) => {
                let body = read_to_end(&stream, limit);
                self.body_read(id, body);
            }
        }
    }

    /// The body has arrived (UI thread): hand the request to the bridge.
    pub fn body_read(&self, id: u64, body: Result<Vec<u8>, BodyError>) {
        let Some(mut request) = self
            .pending
            .borrow_mut()
            .get_mut(&id)
            .and_then(|p| p.request.take())
        else {
            return;
        };
        match body {
            Err(BodyError::TooLarge) => self.finish(id, self.bridge.too_large()),
            Err(BodyError::Io(e)) => self.finish(
                id,
                Response::json(400, &serde_json::json!({"error": "bad_body", "detail": e})),
            ),
            Ok(body) => {
                request.body = body;
                // The responder may run on any thread; it only posts back to the UI thread.
                let post = self.post.clone();
                self.bridge.handle(
                    request,
                    Box::new(move |response| post(SchemeEvent::Respond(id, response))),
                );
            }
        }
    }

    /// Completes a parked request. UI thread only.
    pub fn finish(&self, id: u64, response: Response) {
        let Some(Pending { args, deferral, .. }) = self.pending.borrow_mut().remove(&id) else {
            return;
        };
        let Response {
            status,
            headers,
            body,
        } = response;
        let mut lines = String::new();
        for (name, values) in &headers {
            // The body length is B; a worker's own framing headers would contradict it.
            if name == "content-length" || name == "transfer-encoding" {
                continue;
            }
            for value in values {
                lines.push_str(name);
                lines.push_str(": ");
                lines.push_str(value);
                lines.push_str("\r\n");
            }
        }
        // SAFETY: COM calls on the UI thread, where the deferral was taken.
        unsafe {
            let stream = if body.is_empty() {
                None
            } else {
                SHCreateMemStream(Some(&body))
            };
            if let Ok(reply) = self.env.CreateWebResourceResponse(
                stream.as_ref(),
                i32::from(status),
                &HSTRING::from(reason(status)),
                &HSTRING::from(lines),
            ) {
                let _ = args.SetResponse(&reply);
            }
            let _ = deferral.Complete();
        }
    }
}

/// Method, URL, headers and the body stream of a request.
///
/// # Safety
/// COM calls on the thread that owns `request`.
unsafe fn read_head(
    request: &ICoreWebView2WebResourceRequest,
) -> windows::core::Result<(Request, Option<IStream>)> {
    // SAFETY: as the caller.
    unsafe {
        let mut uri = PWSTR::null();
        request.Uri(&mut uri)?;
        let mut method = PWSTR::null();
        request.Method(&mut method)?;
        let mut headers = HeaderMap::new();
        let it = request.Headers()?.GetIterator()?;
        let mut has = BOOL::default();
        it.HasCurrentHeader(&mut has)?;
        while has.as_bool() {
            let (mut name, mut value) = (PWSTR::null(), PWSTR::null());
            it.GetCurrentHeader(&mut name, &mut value)?;
            http::append(&mut headers, &take_pwstr(name), take_pwstr(value));
            it.MoveNext(&mut has)?;
        }
        let body = request.Content().ok();
        Ok((
            Request {
                method: take_pwstr(method),
                url: take_pwstr(uri),
                headers,
                body: Vec::new(),
            },
            body,
        ))
    }
}

/// The reader thread: joins the MTA, resolves the stream and reads it.
fn read_body(agile: &AgileReference<IStream>, limit: u64) -> Result<Vec<u8>, BodyError> {
    // SAFETY: balanced with CoUninitialize below; the thread is ours.
    let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let result = agile
        .resolve()
        .map_err(|e| BodyError::Io(e.to_string()))
        .and_then(|stream| read_to_end(&stream, limit));
    if joined {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    result
}

/// Reads a stream to EOF (a read of zero bytes), never past `limit`.
fn read_to_end(stream: &IStream, limit: u64) -> Result<Vec<u8>, BodyError> {
    let mut body = Vec::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let mut n = 0u32;
        // SAFETY: the buffer is CHUNK bytes long and outlives the call.
        unsafe { stream.Read(chunk.as_mut_ptr().cast(), CHUNK as u32, Some(&mut n)) }
            .ok()
            .map_err(|e| BodyError::Io(e.to_string()))?;
        if n == 0 {
            return Ok(body);
        }
        if body.len() as u64 + u64::from(n) > limit {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk[..n as usize]);
    }
}

/// A reason phrase. WebView2 wants one; the page sees only the status code.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}
