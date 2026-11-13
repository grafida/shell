//! The macOS webview: a `WKWebView` subclass (file drops, the context menu) and the navigation and
//! UI delegates that apply [`crate::navigation`] (grafida/grafida#88).

use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSDragOperation, NSDraggingInfo, NSEvent, NSMenu, NSUserInterfaceItemIdentification, NSView,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, NSURL};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};

use super::{deliver_drop, keys::KeyState, open_uri, scheme::Scheme};
use crate::{
    log::Log,
    navigation::{self, Decision},
};

/// The pasteboard types a drag from a file manager carries, and the one a drag out of a browser
/// (a link, an image) carries besides. Literals rather than AppKit's constants: the same strings,
/// with nothing to resolve at load time.
const FILE_URL: &str = "public.file-url";
const HTML: &str = "public.html";

/// Context-menu items that would navigate, reload, open a window or download. WebKit's own
/// identifiers (`_WKMenuItemIdentifiers.h`), stable strings.
const REMOVED_MENU_ITEMS: &[&str] = &[
    "WKMenuItemIdentifierGoBack",
    "WKMenuItemIdentifierGoForward",
    "WKMenuItemIdentifierReload",
    "WKMenuItemIdentifierStop",
    "WKMenuItemIdentifierOpenLinkInNewWindow",
    "WKMenuItemIdentifierOpenImageInNewWindow",
    "WKMenuItemIdentifierOpenFrameInNewWindow",
    "WKMenuItemIdentifierOpenMediaInNewWindow",
    "WKMenuItemIdentifierDownloadLinkedFile",
    "WKMenuItemIdentifierDownloadImage",
    "WKMenuItemIdentifierDownloadMedia",
];

pub struct ViewIvars {
    scheme: Rc<Scheme>,
    log: Arc<Log>,
    context_menu: bool,
    /// The current drag is a file drag the shell has taken from WebKit.
    file_drag: Cell<bool>,
    keys: Rc<KeyState>,
}

define_class!(
    // SAFETY: WKWebView may be subclassed; every override below calls `super` for whatever it does
    // not take for itself, and the ivars are only touched on the main thread.
    #[unsafe(super(WKWebView, NSView, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaShellWebView"]
    #[ivars = ViewIvars]
    pub struct ShellWebView;

    impl ShellWebView {
        // ── File drops (grafida/grafida#85 for the design) ──────────────────────────────────
        //
        // The page cannot read the user's files, so the shell takes a drag from a file manager
        // (file URLs, no HTML) before WebKit does and hands the page tokens. A link or an image
        // dragged out of a browser offers HTML and stays WebKit's, as does a drag whose source is
        // the page itself.

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if is_file_drag(sender) {
                self.ivars().file_drag.set(true);
                return NSDragOperation::Copy;
            }
            self.ivars().file_drag.set(false);
            unsafe { msg_send![super(self), draggingEntered: sender] }
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if self.ivars().file_drag.get() {
                return NSDragOperation::Copy;
            }
            unsafe { msg_send![super(self), draggingUpdated: sender] }
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if self.ivars().file_drag.replace(false) {
                return;
            }
            unsafe { msg_send![super(self), draggingExited: sender] }
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            if self.ivars().file_drag.get() {
                return Bool::YES;
            }
            unsafe { msg_send![super(self), prepareForDragOperation: sender] }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> Bool {
            if !self.ivars().file_drag.get() {
                return unsafe { msg_send![super(self), performDragOperation: sender] };
            }
            let paths = dropped_files(sender);
            let (x, y) = self.viewport_point(sender.draggingLocation());
            let ivars = self.ivars();
            deliver_drop(self, &ivars.scheme, paths, x, y, &ivars.log);
            Bool::YES
        }

        #[unsafe(method(concludeDragOperation:))]
        fn conclude_drag(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if self.ivars().file_drag.replace(false) {
                return;
            }
            unsafe { msg_send![super(self), concludeDragOperation: sender] }
        }

        // ── The context menu ────────────────────────────────────────────────────────────────

        // ── Key equivalents: see `keys` for who gets a menu shortcut first ─────────────────

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            if self.ivars().keys.route(self.mtm(), event) == Some(true) {
                return Bool::YES;
            }
            unsafe { msg_send![super(self), performKeyEquivalent: event] }
        }

        #[unsafe(method(willOpenMenu:withEvent:))]
        fn will_open_menu(&self, menu: &NSMenu, event: &NSEvent) {
            unsafe {
                let _: () = msg_send![super(self), willOpenMenu: menu, withEvent: event];
            }
            if !self.ivars().context_menu {
                menu.removeAllItems();
                return;
            }
            for item in menu.itemArray().iter() {
                let removed = item
                    .identifier()
                    .is_some_and(|id| REMOVED_MENU_ITEMS.contains(&id.to_string().as_str()));
                if removed {
                    menu.removeItem(&item);
                }
            }
        }
    }
);

impl ShellWebView {
    pub fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        config: &WKWebViewConfiguration,
        scheme: Rc<Scheme>,
        log: Arc<Log>,
        context_menu: bool,
        keys: Rc<KeyState>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            scheme,
            log,
            context_menu,
            file_drag: Cell::new(false),
            keys,
        });
        // SAFETY: WKWebView's designated initialiser.
        unsafe { msg_send![super(this), initWithFrame: frame, configuration: config] }
    }

    /// A drag location (window coordinates) as the page's viewport coordinates (CSS pixels, top
    /// left origin; the page is never zoomed).
    fn viewport_point(&self, in_window: NSPoint) -> (i32, i32) {
        let p = self.convertPoint_fromView(in_window, None);
        let y = if self.isFlipped() {
            p.y
        } else {
            self.bounds().size.height - p.y
        };
        (p.x.round() as i32, y.round() as i32)
    }
}

fn is_file_drag(sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
    if sender.draggingSource().is_some() {
        return false; // a drag that started inside this app
    }
    let Some(types) = sender.draggingPasteboard().types() else {
        return false;
    };
    let types: Vec<String> = types.iter().map(|t| t.to_string()).collect();
    types.iter().any(|t| t == FILE_URL) && !types.iter().any(|t| t == HTML)
}

/// The regular files a drag carries (folders are not offered to the page, as on Linux).
fn dropped_files(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let pasteboard = sender.draggingPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        return Vec::new();
    };
    let file_url = NSString::from_str(FILE_URL);
    items
        .iter()
        .filter_map(|item| item.stringForType(&file_url))
        // Finder hands over file *reference* URLs (file:///.file/id=…); resolve them to a path.
        .filter_map(|s| NSURL::URLWithString(&s))
        .filter_map(|url| url.filePathURL())
        .filter_map(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()))
        .filter(|p| p.is_file())
        .collect()
}

// ── Navigation and UI delegates ─────────────────────────────────────────────────────────────

pub struct DelegateIvars {
    origin: String,
    log: Arc<Log>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are immutable.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GrafidaShellWebViewDelegate"]
    #[ivars = DelegateIvars]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_action(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request() }
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let user = is_user_initiated(action);
            let origin = self.ivars().origin.as_str();
            // macOS knows the target exactly: none is a new window, else the main frame or a
            // subframe (an embed, which may load its own web content).
            let (what, decision) = match unsafe { action.targetFrame() } {
                None => (
                    "new window for",
                    navigation::for_new_window(&url, origin, user),
                ),
                Some(frame) if unsafe { frame.isMainFrame() } => (
                    "navigation to",
                    navigation::for_main_frame_navigation(&url, origin, user),
                ),
                Some(_) => (
                    "navigation to",
                    navigation::for_subframe_navigation(
                        &url,
                        origin,
                        user,
                        self.source_is_app(action),
                    ),
                ),
            };
            let policy = self.apply(what, &url, decision);
            handler.call((policy,));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _web_view: &WKWebView,
            response: &WKNavigationResponse,
            handler: &block2::DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let url = unsafe { response.response() }
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let log = &self.ivars().log;
            // Something WebKit cannot show would become a download: the shell saves nothing for
            // the page, and never answers with the Download policy.
            if !unsafe { response.canShowMIMEType() } {
                log.line(
                    "shell",
                    &format!(
                        "refused a download of {url}: the shell does not save files for the page"
                    ),
                );
                handler.call((WKNavigationResponsePolicy::Cancel,));
                return;
            }
            if unsafe { response.isForMainFrame() }
                && navigation::for_main_frame_response(&url, &self.ivars().origin)
                    != Decision::Allow
            {
                log.line("shell", &format!("main-frame response from {url} refused"));
                handler.call((WKNavigationResponsePolicy::Cancel,));
                return;
            }
            handler.call((WKNavigationResponsePolicy::Allow,));
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn content_process_terminated(&self, _web_view: &WKWebView) {
            self.ivars()
                .log
                .line("shell", "the web content process terminated");
        }
    }

    unsafe impl WKUIDelegate for Delegate {
        /// `window.open()` and anything else that wants a new window: never one. A user's http(s)
        /// URL goes to the OS browser instead.
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _web_view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> *mut WKWebView {
            let url = unsafe { action.request() }
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let decision =
                navigation::for_new_window(&url, &self.ivars().origin, is_user_initiated(action));
            self.apply("new window for", &url, decision);
            std::ptr::null_mut()
        }
    }
);

impl Delegate {
    /// Whether the frame that started `action` has the app's origin (`<scheme>://app`): `None` when
    /// WebKit names no source frame (not seen in practice: an iframe's initial load names its
    /// parent). A foreign `<iframe>` must not be able to send a frame to the app's URLs, where the
    /// request would carry no `Origin` (security.md M1); its subresources are the scheme handler's
    /// to refuse (`Scheme::frame_may_request`).
    fn source_is_app(&self, action: &WKNavigationAction) -> Option<bool> {
        // SAFETY: the action is live for the call; the frame info and its origin are value objects.
        let (protocol, host) = unsafe {
            let source = action.sourceFrame()?.securityOrigin();
            (source.protocol().to_string(), source.host().to_string())
        };
        Some(navigation::frame_is_app(
            &protocol,
            &host,
            &self.ivars().origin,
        ))
    }

    pub fn new(mtm: MainThreadMarker, origin: String, log: Arc<Log>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { origin, log });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    fn apply(&self, what: &str, url: &str, decision: Decision) -> WKNavigationActionPolicy {
        let log = &self.ivars().log;
        match decision {
            Decision::Allow => WKNavigationActionPolicy::Allow,
            Decision::OpenExternally(target) => {
                log.line("shell", &format!("{what} {url} opened in the OS browser"));
                if let Err(e) = open_uri(&target) {
                    log.line("shell", &format!("could not open {target}: {e}"));
                }
                WKNavigationActionPolicy::Cancel
            }
            Decision::Refuse => {
                log.line("shell", &format!("{what} {url} refused"));
                WKNavigationActionPolicy::Cancel
            }
        }
    }
}

/// Whether a navigation carries a user gesture. WebKit has no public flag for it, and
/// `navigationType` says `LinkActivated` for a script's `el.click()` too, so this asks WebKit's
/// own `_isUserInitiated` (there since macOS 10.11) and treats its absence as "no gesture", the
/// side that never opens a browser.
fn is_user_initiated(action: &WKNavigationAction) -> bool {
    let object: &AnyObject = action.as_ref();
    let responds: bool = unsafe { msg_send![object, respondsToSelector: sel!(_isUserInitiated)] };
    responds && unsafe { msg_send![object, _isUserInitiated] }
}

/// The document-start script that refuses the request bodies WKWebView cannot deliver.
///
/// WebKit hands a scheme handler **no body at all** for a `Blob`/`File`, or for a `FormData` with
/// a file part (ADR 0001, "WKWebView drops file bodies"). Rather than let such a request reach the
/// app empty, `fetch()` rejects and `XMLHttpRequest.send()` / `navigator.sendBeacon()` throw a
/// `TypeError` naming the limitation, for requests to the app's own origin. No conversion.
pub fn blob_guard(origin: &str) -> String {
    format!(
        r#"(() => {{
  const APP = {origin};
  const MESSAGE = "This app cannot send a Blob, File or FormData with a file to " + APP + ": the macOS webview delivers such a body empty. Send the bytes as an ArrayBuffer (await blob.arrayBuffer()) or a string instead.";
  const sameApp = (u) => {{ try {{ return new URL(String(u), location.href).origin === APP; }} catch (e) {{ return false; }} }};
  const hasFile = (body) => body instanceof Blob
    || (body instanceof FormData && [...body.values()].some((v) => v instanceof Blob));
  const flagged = new WeakSet();
  const NativeRequest = window.Request;
  window.Request = new Proxy(NativeRequest, {{
    construct(target, args, newTarget) {{
      const request = Reflect.construct(target, args, newTarget);
      const init = args[1];
      if (init && hasFile(init.body)) flagged.add(request);
      else if (args[0] instanceof NativeRequest && flagged.has(args[0]) && !(init && "body" in init)) flagged.add(request);
      return request;
    }},
  }});
  const nativeFetch = window.fetch;
  window.fetch = function (input, init) {{
    const url = input instanceof NativeRequest ? input.url : input;
    const body = init && "body" in init ? init.body : undefined;
    const file = body !== undefined ? hasFile(body) : (input instanceof NativeRequest && flagged.has(input));
    if (file && sameApp(url)) return Promise.reject(new TypeError(MESSAGE));
    return nativeFetch.apply(this, arguments);
  }};
  const urls = new WeakMap();
  const nativeOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {{
    urls.set(this, url);
    return nativeOpen.apply(this, arguments);
  }};
  const nativeSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function (body) {{
    if (hasFile(body) && sameApp(urls.get(this))) throw new TypeError(MESSAGE);
    return nativeSend.apply(this, arguments);
  }};
  if (navigator.sendBeacon) {{
    const nativeBeacon = navigator.sendBeacon;
    navigator.sendBeacon = function (url, data) {{
      if (hasFile(data) && sameApp(url)) throw new TypeError(MESSAGE);
      return nativeBeacon.apply(this, arguments);
    }};
  }}
}})();"#,
        origin = serde_json::Value::String(origin.to_string())
    )
}
