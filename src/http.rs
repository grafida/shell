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
    fn append_keeps_repeated_values_in_order() {
        let mut h = HeaderMap::new();
        append(&mut h, "X-Multi", "one");
        append(&mut h, "x-multi", "two");
        assert_eq!(h["x-multi"], vec!["one", "two"]);
        assert_eq!(first(&h, "x-multi"), Some("one"));
    }
}
