//! The Linux shell: a tao window holding a WebKitGTK (4.1 API) webview the shell builds itself.
//!
//! wry is deliberately not used on Linux. Its WebKit context is private, so the scheme, the website
//! data directories and the sandbox could only be configured after it had created the webview, and
//! `webkit_web_context_set_sandbox_enabled()` is a fatal error once a web process exists. Building
//! the `WebContext` and the `WebView` here lets the shell do everything in the order WebKit needs:
//! context (data dirs, sandbox, spelling, scheme, security manager) → webview → load.

mod alert;
mod decoration;
pub mod environment;
mod scheme;

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gtk::{gio, glib, prelude::*};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::unix::WindowExtUnix,
    window::{Theme as TaoTheme, Window, WindowBuilder},
};
use webkit2gtk::{
    ContextMenuAction, ContextMenuExt, ContextMenuItemExt, DownloadExt, NavigationPolicyDecision,
    NavigationPolicyDecisionExt, PolicyDecision, PolicyDecisionExt, PolicyDecisionType,
    ResponsePolicyDecision, ResponsePolicyDecisionExt, SecurityManagerExt, SettingsExt,
    URIRequestExt, UserContentInjectedFrames, UserContentManagerExt, UserScript,
    UserScriptInjectionTime, WebContext, WebContextExt, WebView, WebViewExt, WebsiteDataManager,
};

use crate::{
    bridge::{Bridge, ClipboardDone, DialogDone, FileFilter, Host},
    cli::Launch,
    dialog::{self, DialogKind, DialogRequest, Picks},
    http::Response,
    log::Log,
    manifest::Theme,
    navigation::{self, Decision},
    supervisor::{self, Notice, Supervisor},
};

pub use alert::alert;

pub enum UserEvent {
    Respond(u64, Response),
    Worker(Notice),
    SetTitle(String),
    SetTheme(Theme),
    Dialog(DialogRequest),
    ReadClipboard(ClipboardDone),
    Quit,
    #[cfg(all(feature = "automation", debug_assertions))]
    Automation(crate::automation::Command),
}

#[cfg(all(feature = "automation", debug_assertions))]
impl std::fmt::Debug for crate::automation::Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("request", &self.request)
            .finish()
    }
}

/// Runs the app until the window closes. Returns the process exit status.
pub fn run(launch: Launch, log: Arc<Log>) -> i32 {
    let manifest = launch.manifest.clone();
    let origin = manifest.origin();

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let mut builder = WindowBuilder::new()
        .with_title(&manifest.title)
        .with_inner_size(LogicalSize::new(
            manifest.window.width,
            manifest.window.height,
        ))
        .with_theme(tao_theme(manifest.window.theme));
    if let (Some(w), Some(h)) = (manifest.window.min_width, manifest.window.min_height) {
        builder = builder.with_min_inner_size(LogicalSize::new(w, h));
    }
    let window = match builder.build(&event_loop) {
        Ok(window) => window,
        Err(e) => {
            alert(
                &manifest.title,
                "The window could not be created.",
                &e.to_string(),
            );
            return 1;
        }
    };

    // ── WebKit context: everything that must precede the first web process ──────────────────
    let data_dir = glib::user_data_dir()
        .join(manifest.data_dir())
        .join("webkit");
    let cache_dir = glib::user_cache_dir()
        .join(manifest.data_dir())
        .join("webkit");
    let data_manager = WebsiteDataManager::builder()
        .base_data_directory(data_dir.to_string_lossy().as_ref())
        .base_cache_directory(cache_dir.to_string_lossy().as_ref())
        .build();
    let context = WebContext::builder()
        .website_data_manager(&data_manager)
        .build();
    context.set_sandbox_enabled(true);
    if manifest.webview.spell_check {
        let languages = spell_languages();
        let refs: Vec<&str> = languages.iter().map(String::as_str).collect();
        context.set_spell_checking_languages(&refs);
        context.set_spell_checking_enabled(true);
        // WebKit keeps only the languages enchant has a dictionary for. A clean install often has
        // none for the user's language: checking then silently underlines nothing, so say which.
        let loaded: Vec<String> = context
            .spell_checking_languages()
            .iter()
            .map(|l| l.to_string())
            .collect();
        let missing: Vec<&str> = languages
            .iter()
            .filter(|l| !spell_covered(l, &loaded))
            .map(String::as_str)
            .collect();
        log.line(
            "shell",
            &if missing.is_empty() {
                format!("spell checking: {}", languages.join(", "))
            } else {
                format!(
                    "spell checking: {} (no dictionary installed for: {}; install hunspell or aspell dictionaries for them)",
                    if loaded.is_empty() { "off".to_string() } else { loaded.join(", ") },
                    missing.join(", ")
                )
            },
        );
    }
    if let Some(security) = context.security_manager() {
        security.register_uri_scheme_as_secure(&manifest.scheme);
        security.register_uri_scheme_as_cors_enabled(&manifest.scheme);
    }
    {
        let log = log.clone();
        context.connect_download_started(move |_, download| {
            log.line(
                "shell",
                "refused a download: the shell does not save files for the page",
            );
            download.cancel();
        });
    }

    // ── The worker ──────────────────────────────────────────────────────────────────────────
    let supervisor = {
        let notify_proxy = Mutex::new(proxy.clone());
        let log = log.clone();
        let config = supervisor::Config {
            command: launch.worker.clone(),
            ready_timeout: Duration::from_millis(manifest.worker.ready_timeout_ms),
            max_body_bytes: manifest.worker.max_body_bytes,
            restart_max: manifest.worker.restart.max,
            restart_window: Duration::from_secs(manifest.worker.restart.window_seconds),
            shutdown_grace: supervisor::SHUTDOWN_GRACE,
        };
        match Supervisor::start(
            config,
            Box::new(move |notice| {
                let _ = notify_proxy
                    .lock()
                    .unwrap()
                    .send_event(UserEvent::Worker(notice));
            }),
            Arc::new(move |source: &str, line: &str| log.line(source, line)),
        ) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                alert(
                    &manifest.title,
                    "The worker supervisor could not start.",
                    &e.to_string(),
                );
                return 1;
            }
        }
    };

    // ── The scheme bridge ───────────────────────────────────────────────────────────────────
    let host = Arc::new(LinuxHost {
        proxy: Mutex::new(proxy.clone()),
    });
    let bridge = Bridge::new(
        origin.clone(),
        manifest.worker.max_body_bytes,
        manifest.no_store_paths.clone(),
        host,
        supervisor.clone(),
    )
    .with_frame_guard(manifest.webview.frame_guard);
    let respond_proxy = Mutex::new(proxy.clone());
    let scheme = scheme::Scheme::new(
        bridge,
        Arc::new(move |id, response| {
            let _ = respond_proxy
                .lock()
                .unwrap()
                .send_event(UserEvent::Respond(id, response));
        }),
    );
    {
        let scheme = scheme.clone();
        context.register_uri_scheme(&manifest.scheme, move |request| scheme.accept(request));
    }

    // ── The webview ─────────────────────────────────────────────────────────────────────────
    let webview = WebView::with_context(&context);
    // WebKitGTK, like WKWebView, may well ignore the bridge's `frame-ancestors` on a scheme
    // response; the script holds either way (security.md L1).
    if manifest.webview.frame_guard
        && let Some(content) = webview.user_content_manager()
    {
        content.add_script(&UserScript::new(
            &navigation::frame_buster(&origin),
            UserContentInjectedFrames::AllFrames,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        ));
    }
    let devtools = manifest.webview.devtools.enabled(|k| std::env::var(k).ok());
    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(devtools);
        settings.set_enable_back_forward_navigation_gestures(false);
        settings.set_javascript_can_open_windows_automatically(false);
    }
    webview.set_background_color(&background(&window));
    install_policy(&webview, &origin, log.clone());
    install_context_menu(&webview, manifest.webview.context_menu);
    install_file_drops(&webview, scheme.clone(), log.clone());
    {
        let log = log.clone();
        webview.connect_web_process_terminated(move |_, reason| {
            log.line("shell", &format!("the web process terminated: {reason:?}"));
        });
    }
    match window.default_vbox() {
        Some(vbox) => vbox.pack_start(&webview, true, true, 0),
        None => {
            alert(&manifest.title, "The window has no content area.", "");
            return 1;
        }
    }
    webview.show();
    webview.grab_focus();
    log.line(
        "shell",
        &format!(
            "{} {} on {} (devtools {}), origin {origin}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            webkit_version(),
            if devtools { "on" } else { "off" }
        ),
    );

    // ── The automation channel (debug builds with the `automation` feature only) ────────────
    let picks: Picks = Arc::default();
    #[cfg(all(feature = "automation", debug_assertions))]
    let _automation = if launch.automation {
        let proxy = Mutex::new(proxy.clone());
        match crate::automation::open(&manifest.id, move |command| {
            let _ = proxy
                .lock()
                .unwrap()
                .send_event(UserEvent::Automation(command));
        }) {
            Ok(channel) => {
                *RECORDED_OPENS.lock().unwrap() = Some(Vec::new());
                log.line(
                    "shell",
                    &format!(
                        "automation channel open: {}",
                        crate::automation::runtime_file(&manifest.id).display()
                    ),
                );
                Some(channel)
            }
            Err(e) => {
                log.line("shell", &format!("automation channel failed: {e}"));
                None
            }
        }
    } else {
        None
    };

    // ── The event loop ──────────────────────────────────────────────────────────────────────
    let start_url = manifest.start_url();
    let title = manifest.title.clone();
    let exit = Rc::new(std::cell::Cell::new(0));
    let fatal_proxy = proxy.clone();
    let mut decoration = decoration::Decoration::default();
    if manifest.window.theme != Theme::System
        && let Some(line) = decoration.apply(window.gtk_window(), manifest.window.theme)
    {
        log.line("shell", &line);
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let stop = |control_flow: &mut ControlFlow, code: i32| {
            #[cfg(all(feature = "automation", debug_assertions))]
            if let Some(channel) = &_automation {
                channel.close();
            }
            window.set_visible(false);
            supervisor.shutdown();
            exit.set(code);
            *control_flow = ControlFlow::ExitWithCode(code);
        };

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => stop(control_flow, 0),
            Event::UserEvent(UserEvent::Respond(id, response)) => scheme.finish(id, response),
            Event::UserEvent(UserEvent::Worker(Notice::Ready { first: true, .. })) => {
                webview.load_uri(&start_url);
            }
            Event::UserEvent(UserEvent::Worker(Notice::Ready { .. })) => {}
            Event::UserEvent(UserEvent::Worker(Notice::Restarting { .. })) => {}
            Event::UserEvent(UserEvent::Worker(Notice::Fatal(failure))) => {
                let tail = failure.stderr_tail.join("\n");
                exit.set(1);
                let quit = Mutex::new(fatal_proxy.clone());
                alert::show(
                    &window,
                    &title,
                    &format!("{title} stopped working: {}.", failure.reason),
                    &tail,
                    move || {
                        let _ = quit.lock().unwrap().send_event(UserEvent::Quit);
                    },
                );
            }
            Event::UserEvent(UserEvent::SetTitle(t)) => window.set_title(&t),
            Event::UserEvent(UserEvent::SetTheme(theme)) => {
                window.set_theme(tao_theme(theme));
                // GTK's dark preference reaches only a title bar GTK draws; KWin draws its own.
                if let Some(line) = decoration.apply(window.gtk_window(), theme) {
                    log.line("shell", &line);
                }
            }
            Event::UserEvent(UserEvent::Dialog(request)) => {
                dialog::answer(&window, &picks, request)
            }
            #[cfg(all(feature = "automation", debug_assertions))]
            Event::UserEvent(UserEvent::Automation(command)) => {
                automation(&webview, &picks, &scheme, &log, command);
            }
            Event::UserEvent(UserEvent::ReadClipboard(done)) => {
                // GTK's own clipboard, read asynchronously on the UI thread: no wl-paste/xclip,
                // and no prompt, because the window asking is the focused one.
                gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD)
                    .request_text(move |_, text| done(text.map(str::to_owned)));
            }
            Event::UserEvent(UserEvent::Quit) => stop(control_flow, exit.get()),
            _ => {}
        }
    });
}

/// The page's window decoration theme, as tao's.
fn tao_theme(theme: Theme) -> Option<TaoTheme> {
    match theme {
        Theme::Light => Some(TaoTheme::Light),
        Theme::Dark => Some(TaoTheme::Dark),
        Theme::System => None,
    }
}

/// A neutral background for the moments before the page paints, matching the window theme.
fn background(window: &Window) -> gtk::gdk::RGBA {
    match window.theme() {
        TaoTheme::Dark => gtk::gdk::RGBA::new(0.13, 0.13, 0.14, 1.0),
        _ => gtk::gdk::RGBA::new(0.98, 0.98, 0.98, 1.0),
    }
}

/// Spell-check languages from the locale, e.g. `el_GR` and `el` for `LANG=el_GR.UTF-8`.
fn spell_languages() -> Vec<String> {
    let mut out: Vec<String> = glib::language_names()
        .iter()
        .map(|l| l.to_string())
        .filter(|l| l != "C" && l != "POSIX" && !l.contains(['.', '@']))
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|l| seen.insert(l.clone()));
    if out.is_empty() {
        out.push("en_US".into());
    }
    out
}

/// A requested language has a dictionary: exactly, or for a bare `en` any regional `en_*` one.
fn spell_covered(language: &str, loaded: &[String]) -> bool {
    loaded.iter().any(|l| {
        l == language || (!language.contains('_') && l.starts_with(&format!("{language}_")))
    })
}

fn webkit_version() -> String {
    // SAFETY: plain getters with no preconditions.
    let (major, minor, micro) = unsafe {
        (
            webkit2gtk::ffi::webkit_get_major_version(),
            webkit2gtk::ffi::webkit_get_minor_version(),
            webkit2gtk::ffi::webkit_get_micro_version(),
        )
    };
    format!("WebKitGTK {major}.{minor}.{micro}")
}

/// The navigation policy (see `crate::navigation`), wired into WebKit's policy decisions.
fn install_policy(webview: &WebView, origin: &str, log: Arc<Log>) {
    let origin_for_policy = origin.to_string();
    let policy_log = log.clone();
    webview.connect_decide_policy(move |_, decision: &PolicyDecision, kind| {
        let origin = origin_for_policy.as_str();
        let verdict = match kind {
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
                let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else {
                    return false;
                };
                let Some(action) = nav.navigation_action() else {
                    return false;
                };
                let url = action
                    .request()
                    .and_then(|r| r.uri())
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                // The gesture alone: `el.click()` from a script is a LinkClicked navigation too,
                // and an embed's script must not be able to open browser tabs by itself.
                let user = action.is_user_gesture();
                let verdict = if kind == PolicyDecisionType::NewWindowAction {
                    navigation::for_new_window(&url, origin, user)
                } else {
                    navigation::for_navigation(&url, origin, user)
                };
                (url, verdict)
            }
            PolicyDecisionType::Response => {
                let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                    return false;
                };
                if !response.is_main_frame_main_resource() {
                    return false;
                }
                let url = response
                    .request()
                    .and_then(|r| r.uri())
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                let verdict = navigation::for_main_frame_response(&url, origin);
                (url, verdict)
            }
            _ => return false,
        };

        match verdict {
            (_, Decision::Allow) => false,
            (url, Decision::OpenExternally(target)) => {
                decision.ignore();
                policy_log.line(
                    "shell",
                    &format!("navigation to {url} opened in the OS browser"),
                );
                if let Err(e) = open_uri(&target) {
                    policy_log.line("shell", &format!("could not open {target}: {e}"));
                }
                true
            }
            (url, Decision::Refuse) => {
                decision.ignore();
                policy_log.line("shell", &format!("navigation to {url} refused"));
                true
            }
        }
    });

    // `window.open()` from script arrives here rather than as a policy decision.
    let origin = origin.to_string();
    webview.connect_create(move |_, action| {
        let action = action.clone();
        let url = action
            .request()
            .and_then(|r| r.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        match navigation::for_new_window(&url, &origin, action.is_user_gesture()) {
            Decision::OpenExternally(target) => {
                log.line(
                    "shell",
                    &format!("new window for {url} opened in the OS browser"),
                );
                let _ = open_uri(&target);
            }
            _ => log.line("shell", &format!("new window for {url} refused")),
        }
        None
    });
}

/// Files dropped on the window (grafida/grafida#85).
///
/// The web process is sandboxed and cannot see the user's files, so a file WebKit took in itself
/// would reach the page as nothing but its `file://` URI. The shell therefore takes a file drag
/// from a file manager before WebKit does, issues the same per-session tokens a dialog does, and
/// tells the page with a `shell:drop` event: `{files: [{token, name, size}], x, y}`, the drop point
/// in viewport coordinates. The page reads the files through `X-Shell-File`, as after a dialog.
///
/// A file drag offers `text/uri-list` and no `text/html`; a link or an image dragged out of a
/// browser offers `text/html` and stays WebKit's, as does any drag within the page. WebKit handles
/// these signals in its class closures, which run after ours (they are RUN_LAST), so returning
/// `true` from a boolean one, or stopping the emission of a void one, keeps the drag from it.
fn install_file_drops(webview: &WebView, scheme: Rc<scheme::Scheme>, log: Arc<Log>) {
    use gtk::gdk;
    use std::cell::RefCell;

    fn is_file_drag(context: &gdk::DragContext) -> bool {
        let targets: Vec<String> = context
            .list_targets()
            .iter()
            .map(|a| a.name().to_string())
            .collect();
        targets.iter().any(|t| t == "text/uri-list") && !targets.iter().any(|t| t == "text/html")
    }

    let active: Rc<RefCell<Option<gdk::DragContext>>> = Rc::default();
    let is_active = |active: &RefCell<Option<gdk::DragContext>>, context: &gdk::DragContext| {
        active.borrow().as_ref() == Some(context)
    };

    {
        let active = active.clone();
        webview.connect_drag_motion(move |_, context, _, _, time| {
            if !is_file_drag(context) {
                *active.borrow_mut() = None;
                return false;
            }
            *active.borrow_mut() = Some(context.clone());
            context.drag_status(gdk::DragAction::COPY, time);
            true
        });
    }
    {
        let active = active.clone();
        webview.connect_drag_leave(move |widget, context, _| {
            // GTK sends drag-leave before drag-drop too, so the drag stays ours until it ends.
            if is_active(&active, context) {
                widget.stop_signal_emission_by_name("drag-leave");
            }
        });
    }
    {
        let active = active.clone();
        webview.connect_drag_drop(move |widget, context, _, _, time| {
            if !is_active(&active, context) {
                return false;
            }
            widget.drag_get_data(context, &gdk::Atom::intern("text/uri-list"), time);
            true
        });
    }
    webview.connect_drag_data_received(move |widget, context, x, y, data, _, time| {
        if !is_active(&active, context) {
            return;
        }
        widget.stop_signal_emission_by_name("drag-data-received");
        *active.borrow_mut() = None;

        let paths: Vec<PathBuf> = data
            .uris()
            .iter()
            .filter_map(|uri| gio::File::for_uri(uri).path())
            .filter(|path| path.is_file())
            .collect();
        context.drag_finish(!paths.is_empty(), false, time);
        if let Some(webview) = widget.downcast_ref::<WebView>() {
            deliver_drop(webview, &scheme, paths, x, y, &log);
        }
    });
}

/// Hands dropped files to the page: tokens, then the `shell:drop` event. Also what the automation
/// channel's `drop` command calls, since a native drag cannot be scripted on Wayland.
fn deliver_drop(
    webview: &WebView,
    scheme: &scheme::Scheme,
    paths: Vec<PathBuf>,
    x: i32,
    y: i32,
    log: &Log,
) {
    if paths.is_empty() {
        return;
    }
    let count = paths.len();
    let detail = serde_json::json!({"files": scheme.bridge.issue_files(paths), "x": x, "y": y});
    let script =
        format!("window.dispatchEvent(new CustomEvent('shell:drop', {{detail: {detail}}}));");
    webview.evaluate_javascript(&script, None, None, gio::Cancellable::NONE, |_| {});
    log.line("shell", &format!("{count} file(s) dropped at {x},{y}"));
}

/// The context menu: off when the manifest says so; otherwise WebKit's, minus the entries that
/// would navigate away, reload the app, open a window or download.
fn install_context_menu(webview: &WebView, enabled: bool) {
    webview.connect_context_menu(move |_, menu, _, _| {
        if !enabled {
            return true;
        }
        for item in menu.items() {
            if matches!(
                item.stock_action(),
                ContextMenuAction::GoBack
                    | ContextMenuAction::GoForward
                    | ContextMenuAction::Stop
                    | ContextMenuAction::Reload
                    | ContextMenuAction::OpenLinkInNewWindow
                    | ContextMenuAction::OpenImageInNewWindow
                    | ContextMenuAction::OpenFrameInNewWindow
                    | ContextMenuAction::OpenVideoInNewWindow
                    | ContextMenuAction::OpenAudioInNewWindow
                    | ContextMenuAction::DownloadLinkToDisk
                    | ContextMenuAction::DownloadImageToDisk
                    | ContextMenuAction::DownloadVideoToDisk
                    | ContextMenuAction::DownloadAudioToDisk
            ) {
                menu.remove(&item);
            }
        }
        false
    });
}

/// External URLs the automation channel recorded instead of opening. `None` unless it is open, so
/// an automated run never launches a browser on the desktop it is running on.
static RECORDED_OPENS: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn open_uri(uri: &str) -> Result<(), String> {
    if let Some(recorded) = RECORDED_OPENS.lock().unwrap().as_mut() {
        recorded.push(uri.to_string());
        return Ok(());
    }
    gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>)
        .map_err(|e| e.to_string())
}

/// The platform services behind `_native/*`. Called on the UI thread; anything that needs the
/// window goes through the event loop.
struct LinuxHost {
    proxy: Mutex<EventLoopProxy<UserEvent>>,
}

impl LinuxHost {
    fn post(&self, event: UserEvent) {
        let _ = self.proxy.lock().unwrap().send_event(event);
    }
}

impl Host for LinuxHost {
    fn open_file(&self, filters: Vec<FileFilter>, multiple: bool, done: DialogDone) {
        self.post(UserEvent::Dialog(DialogRequest {
            kind: DialogKind::Open { filters, multiple },
            done,
        }));
    }

    fn select_folder(&self, done: DialogDone) {
        self.post(UserEvent::Dialog(DialogRequest {
            kind: DialogKind::Folder,
            done,
        }));
    }

    fn save_file(&self, name: String, filters: Vec<FileFilter>, done: DialogDone) {
        self.post(UserEvent::Dialog(DialogRequest {
            kind: DialogKind::Save { name, filters },
            done,
        }));
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        open_uri(url)
    }

    fn reveal(&self, path: &Path) -> Result<(), String> {
        reveal(path)
    }

    fn set_window_theme(&self, theme: Theme) {
        self.post(UserEvent::SetTheme(theme));
    }

    fn set_window_title(&self, title: &str) {
        self.post(UserEvent::SetTitle(title.to_string()));
    }

    fn quit(&self) {
        self.post(UserEvent::Quit);
    }

    fn read_clipboard(&self, done: ClipboardDone) {
        self.post(UserEvent::ReadClipboard(done));
    }

    fn webview_version(&self) -> String {
        webkit_version()
    }
}

/// Shows a file in the file manager (org.freedesktop.FileManager1), or opens its folder.
fn reveal(path: &Path) -> Result<(), String> {
    let uri = gio::File::for_path(path).uri().to_string();
    let shown = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(|bus| {
        bus.call_sync(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&(vec![uri], "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
    });
    if shown.is_ok() {
        return Ok(());
    }
    let folder = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"))
    };
    open_uri(&gio::File::for_path(folder).uri())
}

/// Runs one automation command on the UI thread.
#[cfg(all(feature = "automation", debug_assertions))]
fn automation(
    webview: &WebView,
    picks: &Picks,
    scheme: &scheme::Scheme,
    log: &Log,
    command: crate::automation::Command,
) {
    use javascriptcore::ValueExt;
    use serde_json::{Value, json};
    use webkit2gtk::{SnapshotOptions, SnapshotRegion};

    let crate::automation::Command { request, reply } = command;
    let fail = |e: String| json!({"ok": false, "error": e});
    match request.get("cmd").and_then(Value::as_str) {
        Some("eval") => {
            let js = request
                .get("js")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            webview.call_async_javascript_function(
                &js,
                None,
                None,
                None,
                gio::Cancellable::NONE,
                move |result| {
                    let answer = match result {
                        Ok(value) => {
                            let result = value
                                .to_json(0)
                                .and_then(|j| serde_json::from_str::<Value>(j.as_str()).ok())
                                .unwrap_or(Value::Null);
                            json!({"ok": true, "result": result})
                        }
                        Err(e) => fail(e.message().to_string()),
                    };
                    let _ = reply.send(answer);
                },
            );
        }
        Some("screenshot") => {
            let Some(path) = request
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
            else {
                let _ = reply.send(fail("screenshot needs a path".into()));
                return;
            };
            webview.snapshot(
                SnapshotRegion::Visible,
                SnapshotOptions::NONE,
                gio::Cancellable::NONE,
                move |result| {
                    let answer = match result.map_err(|e| e.to_string()).and_then(|surface| {
                        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                        surface.write_to_png(&mut file).map_err(|e| e.to_string())
                    }) {
                        Ok(()) => json!({"ok": true}),
                        Err(e) => fail(e),
                    };
                    let _ = reply.send(answer);
                },
            );
        }
        Some("drop") => {
            let paths: Vec<PathBuf> = request
                .get("paths")
                .and_then(Value::as_array)
                .map(|p| {
                    p.iter()
                        .filter_map(Value::as_str)
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default();
            let at = |k: &str| request.get(k).and_then(Value::as_i64).unwrap_or(0) as i32;
            deliver_drop(webview, scheme, paths, at("x"), at("y"), log);
            let _ = reply.send(json!({"ok": true}));
        }
        Some("opened") => {
            let urls = RECORDED_OPENS
                .lock()
                .unwrap()
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default();
            let _ = reply.send(json!({"ok": true, "result": urls}));
        }
        Some("pick") => {
            let answer = match request.get("paths") {
                Some(Value::Null) => None,
                Some(Value::Array(paths)) => Some(
                    paths
                        .iter()
                        .filter_map(Value::as_str)
                        .map(PathBuf::from)
                        .collect(),
                ),
                _ => {
                    let _ = reply.send(fail("pick needs paths: [..] or null".into()));
                    return;
                }
            };
            picks.lock().unwrap().push_back(answer);
            let _ = reply.send(json!({"ok": true}));
        }
        other => {
            let _ = reply.send(fail(format!("unknown command {other:?}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::spell_covered;

    #[test]
    fn a_bare_language_is_covered_by_a_regional_dictionary() {
        let loaded = vec!["en_US".to_string(), "de_DE".to_string()];
        assert!(spell_covered("en_US", &loaded));
        assert!(spell_covered("en", &loaded));
        assert!(!spell_covered("en_GB", &loaded));
        assert!(!spell_covered("el", &loaded));
        assert!(!spell_covered("e", &loaded));
    }
}
