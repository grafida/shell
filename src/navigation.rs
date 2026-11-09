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

    const O: &str = "boson://app";

    #[test]
    fn classification() {
        assert_eq!(classify("boson://app/", O), Target::App);
        assert_eq!(classify("boson://app", O), Target::App);
        assert_eq!(classify("boson://app?x", O), Target::App);
        assert_eq!(classify("boson://app/api/x#y", O), Target::App);
        assert_eq!(classify("boson://apple/", O), Target::Other);
        assert_eq!(classify("boson://app.evil/", O), Target::Other);
        assert_eq!(classify("boson://other/", O), Target::Other);
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
            for_navigation("boson://app/#help", O, true),
            Decision::Allow
        );
    }

    #[test]
    fn an_embed_may_load_but_never_replace_the_app() {
        let embed = "https://www.youtube.com/embed/xyz";
        assert_eq!(for_navigation(embed, O, false), Decision::Allow);
        assert_eq!(for_main_frame_response(embed, O), Decision::Refuse);
        assert_eq!(for_main_frame_response("boson://app/", O), Decision::Allow);
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
        assert_eq!(for_new_window("boson://app/", O, true), Decision::Refuse);
        assert_eq!(for_new_window("about:blank", O, true), Decision::Refuse);
    }
}
