//! The shell ↔ worker wire protocol, version 1.
//!
//! The normative text is `protocol/v1.md`; this module implements it for the shell side, and
//! `tests/protocol_fixtures.rs` holds it to the conformance fixtures in `protocol/fixtures/`.
//! A failing fixture is a question about which side is wrong — never a file to regenerate.
//!
//! The decoder enforces both length limits *before* reading what a length announces, and stops at
//! the first malformed byte instead of scanning for the next magic, so a corrupt stream ends in a
//! [`ErrorKind`] and never in an allocation of whatever a garbage length says, or a wait for bytes
//! that will not come.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};

use serde_json::{Map, Number, Value};

/// Wire protocol version; must equal `Grafida\Shell\Protocol::VERSION` in grafida/shell-php.
pub const PROTOCOL_VERSION: u32 = 1;

/// The four bytes every frame starts with. They do not change with the protocol version.
pub const MAGIC: [u8; 4] = *b"GSF1";

/// The largest header a receiver accepts (fixed).
pub const MAX_HEADER_BYTES: u32 = 1 << 20;

/// The default body limit, per direction; the manifest may change it (`worker.maxBodyBytes`).
pub const DEFAULT_MAX_BODY_BYTES: u64 = 128 << 20;

/// The largest integer any header member may carry: 2^53 − 1, so every JSON reader is exact.
pub const MAX_INT: u64 = (1 << 53) - 1;

/// Header name → every value it was sent with, in order. Repeated headers are never collapsed.
pub type HeaderMap = BTreeMap<String, Vec<String>>;

/// A decoded frame header (protocol/v1.md §3), in normal form: unknown members are dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Header {
    /// Worker → shell, once, first.
    Ready {
        protocol: u64,
        pid: u64,
        app: Option<String>,
        version: Option<String>,
    },
    /// Shell → worker.
    Request {
        id: u64,
        method: String,
        url: String,
        headers: HeaderMap,
    },
    /// Worker → shell.
    Response {
        id: u64,
        status: u16,
        headers: HeaderMap,
    },
    /// Either direction.
    Shutdown,
}

impl Header {
    /// Whether frames of this type may carry a body.
    pub fn allows_body(&self) -> bool {
        matches!(self, Header::Request { .. } | Header::Response { .. })
    }

    /// The header as a JSON object in normal form.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        match self {
            Header::Ready {
                protocol,
                pid,
                app,
                version,
            } => {
                m.insert("type".into(), "ready".into());
                m.insert("protocol".into(), Number::from(*protocol).into());
                m.insert("pid".into(), Number::from(*pid).into());
                if let Some(app) = app {
                    m.insert("app".into(), app.clone().into());
                }
                if let Some(version) = version {
                    m.insert("version".into(), version.clone().into());
                }
            }
            Header::Request {
                id,
                method,
                url,
                headers,
            } => {
                m.insert("type".into(), "request".into());
                m.insert("id".into(), Number::from(*id).into());
                m.insert("method".into(), method.clone().into());
                m.insert("url".into(), url.clone().into());
                m.insert("headers".into(), header_map_to_value(headers));
            }
            Header::Response {
                id,
                status,
                headers,
            } => {
                m.insert("type".into(), "response".into());
                m.insert("id".into(), Number::from(*id).into());
                m.insert("status".into(), Number::from(*status).into());
                m.insert("headers".into(), header_map_to_value(headers));
            }
            Header::Shutdown => {
                m.insert("type".into(), "shutdown".into());
            }
        }
        Value::Object(m)
    }

    /// Validates a JSON value as a header (protocol/v1.md §3 and §4 step 5).
    pub fn from_value(value: &Value) -> Result<Header, Error> {
        let Value::Object(m) = value else {
            return Err(bad_header("the header is not a JSON object"));
        };
        let Some(Value::String(kind)) = m.get("type") else {
            return Err(bad_header("`type` is missing or not a string"));
        };
        match kind.as_str() {
            "ready" => Ok(Header::Ready {
                protocol: int(m, "protocol", 1, MAX_INT)?,
                pid: int(m, "pid", 0, MAX_INT)?,
                app: optional_string(m, "app")?,
                version: optional_string(m, "version")?,
            }),
            "request" => {
                let id = int(m, "id", 0, MAX_INT)?;
                let method = string(m, "method")?;
                if method.is_empty() || !method.bytes().all(is_tchar) {
                    return Err(bad_header("`method` is not an HTTP token"));
                }
                let url = string(m, "url")?;
                if url.is_empty() {
                    return Err(bad_header("`url` is empty"));
                }
                Ok(Header::Request {
                    id,
                    method,
                    url,
                    headers: header_map(m)?,
                })
            }
            "response" => Ok(Header::Response {
                id: int(m, "id", 0, MAX_INT)?,
                status: int(m, "status", 100, 599)? as u16,
                headers: header_map(m)?,
            }),
            "shutdown" => Ok(Header::Shutdown),
            other => Err(Error::Protocol(
                ErrorKind::UnknownType,
                format!("`{other}`"),
            )),
        }
    }
}

/// A complete frame: header plus raw body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: Header,
    pub body: Vec<u8>,
}

/// The protocol error codes of protocol/v1.md §4. Every one of them is fatal to the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Truncated,
    BadMagic,
    HeaderTooLarge,
    BodyTooLarge,
    BadJson,
    BadHeader,
    UnknownType,
}

impl ErrorKind {
    /// The code shared by the spec, the fixtures, both implementations and the logs.
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::Truncated => "truncated",
            ErrorKind::BadMagic => "bad_magic",
            ErrorKind::HeaderTooLarge => "header_too_large",
            ErrorKind::BodyTooLarge => "body_too_large",
            ErrorKind::BadJson => "bad_json",
            ErrorKind::BadHeader => "bad_header",
            ErrorKind::UnknownType => "unknown_type",
        }
    }
}

/// A protocol violation, or an I/O failure of the underlying stream.
#[derive(Debug)]
pub enum Error {
    Protocol(ErrorKind, String),
    Io(io::Error),
}

impl Error {
    /// The protocol error code, or `None` for an I/O failure.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Error::Protocol(kind, _) => Some(kind.code()),
            Error::Io(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Protocol(kind, message) => write!(f, "{}: {message}", kind.code()),
            Error::Io(e) => write!(f, "i/o: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// Reads the next frame, checking in the order protocol/v1.md §4 makes normative.
///
/// Returns `Ok(None)` at a clean end of stream (EOF exactly at a frame boundary). The body is read
/// in bounded chunks as it arrives, never into a buffer sized up front from `B`.
pub fn read_frame<R: Read>(reader: &mut R, max_body: u64) -> Result<Option<Frame>, Error> {
    // 1. Magic.
    let mut magic = [0u8; 4];
    match read_full(reader, &mut magic)? {
        0 => return Ok(None),
        4 => {}
        n => return Err(truncated(format!("EOF after {n} of 4 magic bytes"))),
    }
    if magic != MAGIC {
        return Err(Error::Protocol(
            ErrorKind::BadMagic,
            format!("expected GSF1, got {magic:02x?}"),
        ));
    }

    // 2. H.
    let mut h = [0u8; 4];
    if read_full(reader, &mut h)? != 4 {
        return Err(truncated("EOF inside the header length".into()));
    }
    let h = u32::from_le_bytes(h);
    if h > MAX_HEADER_BYTES {
        return Err(Error::Protocol(
            ErrorKind::HeaderTooLarge,
            format!("{h} bytes > {MAX_HEADER_BYTES}"),
        ));
    }

    // 3. Header bytes.
    let mut raw = vec![0u8; h as usize];
    let got = read_full(reader, &mut raw)?;
    if got != raw.len() {
        return Err(truncated(format!("EOF after {got} of {h} header bytes")));
    }

    // 4–5. JSON, then shape — before B, so a bad header is reported without waiting for a body.
    let value: Value = serde_json::from_slice(&raw)
        .map_err(|e| Error::Protocol(ErrorKind::BadJson, e.to_string()))?;
    let header = Header::from_value(&value)?;

    // 6. B.
    let mut b = [0u8; 8];
    if read_full(reader, &mut b)? != 8 {
        return Err(truncated("EOF inside the body length".into()));
    }
    let b = u64::from_le_bytes(b);
    if b != 0 && !header.allows_body() {
        return Err(bad_header("this frame type carries no body"));
    }
    if b > max_body {
        return Err(Error::Protocol(
            ErrorKind::BodyTooLarge,
            format!("{b} bytes > {max_body}"),
        ));
    }

    // 7. Body, in bounded chunks: `take` + `read_to_end` grows the buffer as bytes arrive.
    let mut body = Vec::new();
    reader.by_ref().take(b).read_to_end(&mut body)?;
    if body.len() as u64 != b {
        return Err(truncated(format!(
            "EOF after {} of {b} body bytes",
            body.len()
        )));
    }

    Ok(Some(Frame { header, body }))
}

/// Encodes a frame. The header is validated with the decoder's own rules first, so this never
/// emits a frame a conforming receiver would reject.
pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, Error> {
    let value = frame.header.to_value();
    // Round-trip through the validator: catches upper-case names, CR/LF in values, bad tokens,
    // out-of-range integers — anything the caller put into the typed header.
    Header::from_value(&value)?;
    if !frame.body.is_empty() && !frame.header.allows_body() {
        return Err(bad_header("this frame type carries no body"));
    }
    let json = serde_json::to_vec(&value).map_err(|e| bad_header(&e.to_string()))?;
    if json.len() > MAX_HEADER_BYTES as usize {
        return Err(Error::Protocol(
            ErrorKind::HeaderTooLarge,
            format!("{} bytes > {MAX_HEADER_BYTES}", json.len()),
        ));
    }

    let mut out = Vec::with_capacity(16 + json.len() + frame.body.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(frame.body.len() as u64).to_le_bytes());
    out.extend_from_slice(&frame.body);
    Ok(out)
}

/// Encodes and writes a frame, then flushes.
pub fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> Result<(), Error> {
    writer.write_all(&encode_frame(frame)?)?;
    writer.flush()?;
    Ok(())
}

/// Reads until `buf` is full or EOF; returns how many bytes were read. Retries interrupted reads.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

fn truncated(message: String) -> Error {
    Error::Protocol(ErrorKind::Truncated, message)
}

fn bad_header(message: &str) -> Error {
    Error::Protocol(ErrorKind::BadHeader, message.to_string())
}

fn int(m: &Map<String, Value>, key: &str, min: u64, max: u64) -> Result<u64, Error> {
    // `as_u64` is `Some` only for a JSON integer literal: 17.0, 1e2 and negatives are `None`.
    match m.get(key).and_then(Value::as_u64) {
        Some(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(bad_header(&format!(
            "`{key}` is missing or not an integer in {min}..={max}"
        ))),
    }
}

fn string(m: &Map<String, Value>, key: &str) -> Result<String, Error> {
    match m.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(bad_header(&format!("`{key}` is missing or not a string"))),
    }
}

fn optional_string(m: &Map<String, Value>, key: &str) -> Result<Option<String>, Error> {
    match m.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad_header(&format!("`{key}` is not a string"))),
    }
}

fn header_map(m: &Map<String, Value>) -> Result<HeaderMap, Error> {
    let Some(Value::Object(headers)) = m.get("headers") else {
        return Err(bad_header("`headers` is missing or not an object"));
    };
    let mut out = HeaderMap::new();
    for (name, values) in headers {
        if name.is_empty() || !name.bytes().all(|c| is_tchar(c) && !c.is_ascii_uppercase()) {
            return Err(bad_header(&format!(
                "header name `{name}` is not a lower-case token"
            )));
        }
        let Value::Array(values) = values else {
            return Err(bad_header(&format!("header `{name}` is not an array")));
        };
        if values.is_empty() {
            return Err(bad_header(&format!("header `{name}` has no values")));
        }
        let mut list = Vec::with_capacity(values.len());
        for v in values {
            let Value::String(s) = v else {
                return Err(bad_header(&format!(
                    "header `{name}` has a non-string value"
                )));
            };
            if s.contains(['\r', '\n', '\0']) {
                return Err(bad_header(&format!(
                    "header `{name}` has CR, LF or NUL in a value"
                )));
            }
            list.push(s.clone());
        }
        out.insert(name.clone(), list);
    }
    Ok(out)
}

fn header_map_to_value(headers: &HeaderMap) -> Value {
    Value::Object(
        headers
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    Value::Array(v.iter().cloned().map(Value::String).collect()),
                )
            })
            .collect(),
    )
}

/// RFC 9110 `tchar`.
fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(headers: HeaderMap) -> Frame {
        Frame {
            header: Header::Request {
                id: 1,
                method: "GET".into(),
                url: "boson://app/".into(),
                headers,
            },
            body: Vec::new(),
        }
    }

    #[test]
    fn protocol_version_is_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn encoder_refuses_what_the_decoder_would_reject() {
        let upper = HeaderMap::from([("Content-Type".to_string(), vec!["x".to_string()])]);
        let crlf = HeaderMap::from([("x-a".to_string(), vec!["1\r\nx-b: 2".to_string()])]);
        let empty = HeaderMap::from([("x-a".to_string(), Vec::new())]);
        for headers in [upper, crlf, empty] {
            let err = encode_frame(&request(headers)).unwrap_err();
            assert_eq!(err.code(), Some("bad_header"));
        }

        let shutdown_with_body = Frame {
            header: Header::Shutdown,
            body: b"x".to_vec(),
        };
        assert_eq!(
            encode_frame(&shutdown_with_body).unwrap_err().code(),
            Some("bad_header")
        );
    }

    #[test]
    fn repeated_headers_round_trip_in_order() {
        let headers = HeaderMap::from([(
            "x-multi".to_string(),
            vec!["one".to_string(), "two".to_string()],
        )]);
        let frame = request(headers);
        let bytes = encode_frame(&frame).unwrap();
        let back = read_frame(&mut bytes.as_slice(), DEFAULT_MAX_BODY_BYTES)
            .unwrap()
            .unwrap();
        assert_eq!(back, frame);
    }
}
