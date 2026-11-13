//! The navigation policy: the page may navigate within its own origin and nowhere else.
//!
//! Anything off-origin that the *user* asked for (a link click, a form, a gesture, `window.open`)
//! is cancelled and, when it is http(s), handed to the OS browser. Anything else is refused
//! outright. The platform layer feeds WebKit's (or WKWebView's, or WebView2's) navigation events
//! through these pure functions so the policy is the same, and tested, everywhere.
//!
//! Subframes matter: article HTML in the editor routinely embeds a video in an `<iframe>`, and an
//! embed's own load must not be mistaken for the app navigating away. WebKitGTK cannot say whether
//! a navigation *action* targets the main frame, so the platform layer applies
//! [`for_navigation`] (which lets a non-user subframe-shaped load through) and then
//! [`for_main_frame_response`] (which refuses an off-origin main-frame response).

/// What to do with a navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Cancel it and open this http(s) URL in the OS browser.
    OpenExternally(String),
    /// Cancel it.
    Refuse,
}

/// Where a URL points, relative to the app's origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `<scheme>://app` itself.
    App,
    /// `about:blank` / `about:srcdoc`, which frames inherit their parent's origin from.
    Blank,
    /// http or https.
    Web,
    /// Anything else (`file:`, `mailto:`, another custom scheme, …).
    Other,
}

pub fn classify(url: &str, origin: &str) -> Target {
    if let Some(rest) = url.strip_prefix(origin)
        && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
    {
        return Target::App;
    }
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if url == "about:blank" || url == "about:srcdoc" {
        Target::Blank
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        Target::Web
    } else {
        Target::Other
    }
}

/// A navigation action in any frame. `user` is true for a link click, a form submission, or
/// anything carrying a user gesture.
pub fn for_navigation(url: &str, origin: &str, user: bool) -> Decision {
    match (classify(url, origin), user) {
        (Target::App | Target::Blank, _) => Decision::Allow,
        (Target::Web, true) => Decision::OpenExternally(url.to_string()),
        (Target::Other, true) => Decision::Refuse,
        // Possibly a subframe loading its own content (an embed): let it through here; if it is
        // in fact the main frame, `for_main_frame_response` stops it before anything renders.
        (Target::Web, false) => Decision::Allow,
        (Target::Other, false) => Decision::Refuse,
    }
}

/// A request for a new window (`window.open`, `target=_blank`). The shell never opens one.
pub fn for_new_window(url: &str, origin: &str, user: bool) -> Decision {
    match classify(url, origin) {
        Target::Web if user => Decision::OpenExternally(url.to_string()),
        _ => Decision::Refuse,
    }
}

/// A navigation action known to target the main frame (macOS: `targetFrame.isMainFrame`). Nothing
/// off-origin may load there: a user's http(s) navigation goes to the OS browser, anything else is
/// refused. Unlike [`for_navigation`] nothing is let through on the strength of a later check.
pub fn for_main_frame_navigation(url: &str, origin: &str, user: bool) -> Decision {
    match (classify(url, origin), user) {
        (Target::App | Target::Blank, _) => Decision::Allow,
        (Target::Web, true) => Decision::OpenExternally(url.to_string()),
        _ => Decision::Refuse,
    }
}

/// A navigation action known to target a subframe (macOS: `targetFrame` is not the main frame).
/// The layered [`for_navigation`] rule, except that a frame may only be sent to the app's own
/// origin by the app: `source_is_app` is whether the frame that started the navigation has the app's
/// origin (`Some(false)`: a foreign frame, which would make the engine request the app's URLs with
/// no `Origin` header). `None`, the engine naming no source, is refused too: WebKit names the
/// parent frame even for an iframe's initial load (measured, security.md M1), so a nameless source
/// is not a case the app's own page produces.
pub fn for_subframe_navigation(
    url: &str,
    origin: &str,
    user: bool,
    source_is_app: Option<bool>,
) -> Decision {
    if classify(url, origin) == Target::App && source_is_app != Some(true) {
        return Decision::Refuse;
    }
    for_navigation(url, origin, user)
}

/// Whether a frame whose security origin is `protocol://host` is the app itself, and so may act
/// for it. Compared member by member, as WebKit reports them, so `shell://app.evil` is not the app.
pub fn frame_is_app(protocol: &str, host: &str, origin: &str) -> bool {
    origin
        .split_once("://")
        .is_some_and(|(scheme, authority)| protocol == scheme && host == authority)
}

/// Whether a request for one of the app's URLs, made from a frame, may be answered: the main frame
/// always (nothing off-origin ever renders there), a subframe only when it has the app's origin. A
/// foreign frame's `<img src>` or `no-cors` fetch of the app's URLs carries no `Origin`, and on
/// WKWebView no `Sec-Fetch-Site` either, so the frame is the only thing that gives it away
/// (security.md M1).
pub fn frame_may_request(is_main_frame: bool, protocol: &str, host: &str, origin: &str) -> bool {
    is_main_frame || frame_is_app(protocol, host, origin)
}

/// The document-start script every platform injects into **every frame** while the manifest's
/// `webview.frameGuard` is on (security.md L1): one of the app's own pages, loaded in a frame with
/// a foreign ancestor anywhere above it (an embed framing `<scheme>://app/…`, to overlay it), stops
/// loading and empties itself before any of its own script runs. Inert everywhere else: in the
/// main frame, in a foreign document (an embed's own page), and in the app's own `about:blank` /
/// `srcdoc` frames (TinyMCE's), whose `location.origin` is not the app's.
///
/// The bridge's `frame-ancestors 'self'` says the same in a header, but WKWebView enforces neither
/// it nor `X-Frame-Options` on a scheme handler's response (measured, macOS 27.2), so the script
/// is what holds. `location.ancestorOrigins` names every ancestor (an opaque one as `"null"`); an
/// engine without it walks the parents, where a cross-origin one throws.
pub fn frame_buster(origin: &str) -> String {
    let app = serde_json::to_string(origin).expect("a string serialises");
    format!(
        r#"(() => {{
  const APP = {app};
  if (window === window.top || location.origin !== APP) return;
  let foreign = false;
  const ancestors = location.ancestorOrigins;
  if (ancestors) {{
    for (let i = 0; i < ancestors.length; i++) if (ancestors[i] !== APP) foreign = true;
  }} else {{
    try {{
      for (let w = window.parent; ; w = w.parent) {{
        if (w.origin !== APP) {{ foreign = true; break; }}
        if (w === w.top) break;
      }}
    }} catch (e) {{ foreign = true; }}
  }}
  if (!foreign) return;
  window.stop();
  const empty = () => {{ if (document.documentElement) document.documentElement.remove(); }};
  empty();
  document.addEventListener("readystatechange", empty);
}})();"#
    )
}

/// The response to the main frame's main resource: only the app's own pages may render there.
pub fn for_main_frame_response(url: &str, origin: &str) -> Decision {
    match classify(url, origin) {
        Target::App | Target::Blank => Decision::Allow,
        _ => Decision::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: &str = "shell://app";

    #[test]
    fn the_frame_buster_names_the_app_as_a_string_literal() {
        assert!(frame_buster(O).contains(r#"const APP = "shell://app";"#));
        assert!(frame_buster(r#"a"b://app"#).contains(r#"const APP = "a\"b://app";"#));
    }

    #[test]
    fn a_foreign_frame_cannot_send_a_subframe_to_the_app() {
        // The Origin-less GET that the bridge cannot tell from the app's own (security.md M1).
        assert_eq!(
            for_subframe_navigation("shell://app/api/export", O, false, Some(false)),
            Decision::Refuse
        );
        assert_eq!(
            for_subframe_navigation("shell://app/api/export", O, true, Some(false)),
            Decision::Refuse
        );
        // The app's own frames are legitimate; a nameless source is not something they produce.
        assert_eq!(
            for_subframe_navigation("shell://app/embed.html", O, false, Some(true)),
            Decision::Allow
        );
        assert_eq!(
            for_subframe_navigation("shell://app/embed.html", O, false, None),
            Decision::Refuse
        );
        // Everything else is the layered rule, whoever asks.
        for source in [Some(true), Some(false), None] {
            assert_eq!(
                for_subframe_navigation("https://youtu.be/x", O, false, source),
                for_navigation("https://youtu.be/x", O, false)
            );
            assert_eq!(
                for_subframe_navigation("about:blank", O, false, source),
                Decision::Allow
            );
            assert_eq!(
                for_subframe_navigation("shell://apple/", O, false, source),
                Decision::Refuse
            );
        }
        // Look-alikes of the app's origin are not the app.
        assert_eq!(
            for_subframe_navigation("shell://app.evil/", O, false, Some(false)),
            Decision::Refuse
        );
    }

    #[test]
    fn only_the_main_frame_and_the_apps_own_frames_may_request_the_app() {
        // The main frame's first load has no origin yet (measured on WebKit: `://`).
        assert!(frame_may_request(true, "", "", O));
        assert!(frame_may_request(true, "http", "127.0.0.1", O));
        assert!(frame_may_request(false, "shell", "app", O));
        // A foreign embed's subresources: the Origin-less GETs of security.md M1.
        assert!(!frame_may_request(false, "http", "127.0.0.1", O));
        assert!(!frame_may_request(false, "https", "www.youtube.com", O));
        assert!(!frame_may_request(false, "", "", O));
        assert!(!frame_may_request(false, "shell", "app.evil", O));
        assert!(!frame_may_request(false, "shell", "apple", O));
        assert!(!frame_may_request(false, "shells", "app", O));
        assert!(
            !frame_is_app("shell", "app", "shell"),
            "an origin without a scheme"
        );
    }

    #[test]
    fn classification() {
        assert_eq!(classify("shell://app/", O), Target::App);
        assert_eq!(classify("shell://app", O), Target::App);
        assert_eq!(classify("shell://app?x", O), Target::App);
        assert_eq!(classify("shell://app/api/x#y", O), Target::App);
        assert_eq!(classify("shell://apple/", O), Target::Other);
        assert_eq!(classify("shell://app.evil/", O), Target::Other);
        assert_eq!(classify("shell://other/", O), Target::Other);
        assert_eq!(classify("about:blank", O), Target::Blank);
        assert_eq!(classify("about:srcdoc", O), Target::Blank);
        assert_eq!(classify("https://grafida.app/", O), Target::Web);
        assert_eq!(classify("HTTP://x/", O), Target::Web);
        assert_eq!(classify("file:///etc/passwd", O), Target::Other);
        assert_eq!(classify("mailto:a@b", O), Target::Other);
        assert_eq!(classify("javascript:alert(1)", O), Target::Other);
    }

    #[test]
    fn a_clicked_web_link_goes_to_the_browser() {
        assert_eq!(
            for_navigation("https://grafida.app/docs", O, true),
            Decision::OpenExternally("https://grafida.app/docs".into())
        );
        assert_eq!(for_navigation("mailto:a@b", O, true), Decision::Refuse);
        assert_eq!(
            for_navigation("shell://app/#help", O, true),
            Decision::Allow
        );
    }

    #[test]
    fn an_embed_may_load_but_never_replace_the_app() {
        let embed = "https://www.youtube.com/embed/xyz";
        assert_eq!(for_navigation(embed, O, false), Decision::Allow);
        assert_eq!(for_main_frame_response(embed, O), Decision::Refuse);
        assert_eq!(for_main_frame_response("shell://app/", O), Decision::Allow);
        assert_eq!(
            for_navigation("file:///etc/passwd", O, false),
            Decision::Refuse
        );
    }

    #[test]
    fn new_windows_never_open() {
        assert_eq!(
            for_new_window("https://x/", O, true),
            Decision::OpenExternally("https://x/".into())
        );
        assert_eq!(for_new_window("https://x/", O, false), Decision::Refuse);
        assert_eq!(for_new_window("shell://app/", O, true), Decision::Refuse);
        assert_eq!(for_new_window("about:blank", O, true), Decision::Refuse);
    }

    #[test]
    fn the_main_frame_never_loads_anything_off_origin() {
        assert_eq!(
            for_main_frame_navigation("shell://app/x", O, false),
            Decision::Allow
        );
        assert_eq!(
            for_main_frame_navigation("about:blank", O, false),
            Decision::Allow
        );
        assert_eq!(
            for_main_frame_navigation("https://example.com/", O, true),
            Decision::OpenExternally("https://example.com/".into())
        );
        // A script redirecting the app, with no gesture: refused, not opened, not loaded.
        assert_eq!(
            for_main_frame_navigation("https://example.com/", O, false),
            Decision::Refuse
        );
        assert_eq!(
            for_main_frame_navigation("file:///etc/passwd", O, true),
            Decision::Refuse
        );
        assert_eq!(
            for_main_frame_navigation("shell://apple/", O, true),
            Decision::Refuse
        );
    }
}
