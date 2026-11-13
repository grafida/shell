//! The shell's own request/response values: what the webview asked for and what goes back to it.
//! Header names are lower-case and every value of a repeated header is kept, in order, exactly as
//! on the wire (protocol/v1.md §3.5).

use crate::protocol::HeaderMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The absolute URL, verbatim as the webview reported it.
    pub url: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Adds one value to a header map, lower-casing the name and keeping earlier values.
pub fn append(headers: &mut HeaderMap, name: &str, value: impl Into<String>) {
    headers
        .entry(name.to_ascii_lowercase())
        .or_default()
        .push(value.into());
}

/// The first value of a header, if any.
pub fn first<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|values| values.first())
        .map(String::as_str)
}

impl Request {
    /// The path of the URL, still percent-encoded; `/` when absent. Query and fragment removed.
    pub fn path(&self) -> &str {
        let rest = match self.url.find("://") {
            Some(i) => &self.url[i + 3..],
            None => &self.url,
        };
        let rest = rest.split(['?', '#']).next().unwrap_or("");
        match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        }
    }

    /// [`Request::path`] percent-decoded once, which is what the shell's own path checks compare
    /// (security.md L6): a worker that decodes before routing sees `/%61pi/x` as `/api/x`, so the
    /// shell must too, as Apache matches `<Location>` on the decoded path. Bytes, since the result
    /// need not be UTF-8. A `%` not followed by two hex digits is kept as it is (as PHP's
    /// `rawurldecode()` does). `None` when an escape decodes to `/` or NUL: Apache's default
    /// (`AllowEncodedSlashes Off`) refuses those, since `a%2Fb` is one segment to whoever matches
    /// the raw path and two to whoever decodes first. Dot segments need no handling here: the
    /// engines resolve `..`, `%2e%2e` and their mixes before the shell sees the URL (WHATWG URL).
    pub fn decoded_path(&self) -> Option<Vec<u8>> {
        let raw = self.path().as_bytes();
        let mut decoded = Vec::with_capacity(raw.len());
        let mut i = 0;
        while i < raw.len() {
            let escaped = match raw.get(i..i + 3) {
                Some([b'%', hi, lo]) => hex(*hi).zip(hex(*lo)).map(|(hi, lo)| hi << 4 | lo),
                _ => None,
            };
            match escaped {
                Some(b'/' | 0) => return None,
                Some(byte) => {
                    decoded.push(byte);
                    i += 3;
                }
                None => {
                    decoded.push(raw[i]);
                    i += 1;
                }
            }
        }
        Some(decoded)
    }
}

fn hex(digit: u8) -> Option<u8> {
    (digit as char).to_digit(16).map(|d| d as u8)
}

impl Response {
    pub fn new(status: u16) -> Response {
        Response {
            status,
            headers: HeaderMap::new(),
            body: Vec::new(),
        }
    }

    /// A JSON response the shell itself produces. Never cached.
    pub fn json(status: u16, value: &serde_json::Value) -> Response {
        let mut response = Response::new(status);
        append(
            &mut response.headers,
            "content-type",
            "application/json; charset=utf-8",
        );
        append(&mut response.headers, "cache-control", "no-store");
        response.body = value.to_string().into_bytes();
        response
    }

    /// `{"error": code}` with the given status.
    pub fn error(status: u16, code: &str) -> Response {
        Response::json(status, &serde_json::json!({ "error": code }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str) -> Request {
        Request {
            method: "GET".into(),
            url: url.into(),
            headers: HeaderMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn path_strips_origin_query_and_fragment() {
        assert_eq!(req("shell://app/api/x?y=1#z").path(), "/api/x");
        assert_eq!(req("shell://app").path(), "/");
        assert_eq!(req("shell://app?x").path(), "/");
        assert_eq!(req("shell://app/_native/info").path(), "/_native/info");
        assert_eq!(req("shell://app/a%2Fb").path(), "/a%2Fb");
    }

    #[test]
    fn decoded_path_decodes_once_and_refuses_an_escaped_slash_or_nul() {
        let decoded = |url| req(url).decoded_path();
        let ok = |url, want: &[u8]| assert_eq!(decoded(url).as_deref(), Some(want), "{url}");
        ok("shell://app/%61pi/x?y=%2F#%00", b"/api/x");
        ok("shell://app/%5Fnative/%69nfo", b"/_native/info");
        ok("shell://app/%5f%6E", b"/_n");
        ok("shell://app", b"/");
        // Once: `%2541` is `%41`, not `A`.
        ok("shell://app/%2541", b"/%41");
        // Not escapes: kept as they are.
        ok("shell://app/%zz/%4/%", b"/%zz/%4/%");
        ok("shell://app/%4g%g4", b"/%4g%g4");
        // Not UTF-8 after decoding, and raw non-ASCII: bytes either way.
        ok("shell://app/%FF%e2%82%AC", b"/\xff\xe2\x82\xac");
        ok("shell://app/caf\u{e9}", "/caf\u{e9}".as_bytes());
        for url in [
            "shell://app/a%2Fb",
            "shell://app/a%2fb",
            "shell://app/%2F",
            "shell://app/a%00",
            "shell://app/%5Fnative%2Finfo",
        ] {
            assert_eq!(decoded(url), None, "{url}");
        }
    }

    #[test]
    fn append_keeps_repeated_values_in_order() {
        let mut h = HeaderMap::new();
        append(&mut h, "X-Multi", "one");
        append(&mut h, "x-multi", "two");
        assert_eq!(h["x-multi"], vec!["one", "two"]);
        assert_eq!(first(&h, "x-multi"), Some("one"));
    }
}
