//! The Linux scheme bridge: WebKitGTK `URISchemeRequest` ⇄ [`crate::http`].
//!
//! The scheme is registered on the WebKit context **by the shell** (not through wry), secure and
//! CORS-enabled (ADR 0001). Every request is parked in a UI-thread map under an id, its body is read
//! **to EOF, in bounded chunks, asynchronously on the main loop** (never with `G_MAXSSIZE`, which is
//! saucer#57, and never "until the first short read", which is wry's reader), and it is completed
//! later from that map, on the UI thread, when the answer arrives through the event loop.

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use gtk::{gio, glib, prelude::*};
use webkit2gtk::{URISchemeRequest, URISchemeRequestExt, URISchemeResponse, URISchemeResponseExt};

use crate::{
    bridge::Bridge,
    http::{self, Request, Response},
    protocol::HeaderMap,
};

/// Bytes asked of the body stream per read.
const CHUNK: usize = 1 << 20;

/// What the UI thread keeps for the scheme handler.
pub struct Scheme {
    pub bridge: Bridge,
    pending: RefCell<HashMap<u64, URISchemeRequest>>,
    next_id: RefCell<u64>,
    /// Posts a finished response back to the UI thread: `(request id, response)`.
    post: Poster,
}

pub type Poster = Arc<dyn Fn(u64, Response) + Send + Sync>;

impl Scheme {
    pub fn new(bridge: Bridge, post: Poster) -> Rc<Scheme> {
        Rc::new(Scheme {
            bridge,
            pending: RefCell::new(HashMap::new()),
            next_id: RefCell::new(1),
            post,
        })
    }

    /// The `register_uri_scheme` callback.
    pub fn accept(self: &Rc<Scheme>, request: &URISchemeRequest) {
        let id = {
            let mut next = self.next_id.borrow_mut();
            let id = *next;
            *next += 1;
            id
        };
        self.pending.borrow_mut().insert(id, request.clone());

        let method = request
            .http_method()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "GET".into());
        let url = request.uri().map(|u| u.to_string()).unwrap_or_default();
        let mut headers = HeaderMap::new();
        if let Some(h) = request.http_headers() {
            h.foreach(|name, value| http::append(&mut headers, name, value));
        }
        let body = request.http_body();

        let this = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let limit = this.bridge.max_body_bytes();
            let body = match body {
                None => Ok(Vec::new()),
                Some(stream) => read_to_end(&stream, limit).await,
            };
            match body {
                Err(ReadError::TooLarge) => this.finish(id, this.bridge.too_large()),
                Err(ReadError::Io(e)) => this.finish(
                    id,
                    Response::json(400, &serde_json::json!({"error": "bad_body", "detail": e})),
                ),
                Ok(body) => {
                    let request = Request {
                        method,
                        url,
                        headers,
                        body,
                    };
                    // The responder may run on any thread; it only posts back to the UI thread.
                    let post = this.post.clone();
                    this.bridge
                        .handle(request, Box::new(move |response| post(id, response)));
                }
            }
        });
    }

    /// Completes a parked request. UI thread only.
    pub fn finish(&self, id: u64, response: Response) {
        let Some(request) = self.pending.borrow_mut().remove(&id) else {
            return; // already finished, or WebKit gave up on it
        };
        let Response {
            status,
            headers,
            body,
        } = response;
        let length = body.len() as i64;
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(body));
        let reply = URISchemeResponse::new(&stream, length);
        reply.set_status(u32::from(status), None);

        let soup = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
        for (name, values) in &headers {
            // The body length is B; a worker's own framing headers would contradict it.
            if name == "content-length" || name == "transfer-encoding" {
                continue;
            }
            for value in values {
                soup.append(name, value);
            }
        }
        if let Some(content_type) = http::first(&headers, "content-type") {
            reply.set_content_type(content_type);
        }
        reply.set_http_headers(soup);
        request.finish_with_response(&reply);
    }
}

enum ReadError {
    TooLarge,
    Io(String),
}

async fn read_to_end(stream: &gio::InputStream, limit: u64) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(CHUNK, glib::Priority::DEFAULT)
            .await
            .map_err(|e| ReadError::Io(e.to_string()))?;
        if chunk.is_empty() {
            return Ok(body); // EOF
        }
        if body.len() as u64 + chunk.len() as u64 > limit {
            return Err(ReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
}
