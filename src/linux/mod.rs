//! The Linux shell: a tao window holding a WebKitGTK (4.1 API) webview the shell builds itself.
//!
//! wry is deliberately not used on Linux. Its WebKit context is private, so the scheme, the website
//! data directories and the sandbox could only be configured after it had created the webview, and
//! `webkit_web_context_set_sandbox_enabled()` is a fatal error once a web process exists. Building
//! the `WebContext` and the `WebView` here lets the shell do everything in the order WebKit needs:
//! context (data dirs, sandbox, spelling, scheme, security manager) → webview → load.

mod alert;
mod scheme;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use gtk::{gio, glib, prelude::*};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
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
    URIRequestExt, WebContext, WebContextExt, WebView, WebViewExt, WebsiteDataManager,
};

use crate::{
    bridge::{Bridge, DialogDone, FileFilter, Host},
    cli::Launch,
    http::Response,
    log::Log,
    manifest::Theme,
    navigation::{self, Decision},
    supervisor::{self, Notice, Supervisor},
};

pub use alert::alert;

#[derive(Debug)]
pub enum UserEvent {
    Respond(u64, Response),
    Worker(Notice),
    SetTitle(String),
    SetTheme(Theme),
    Dialog(DialogRequest),
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

/// Dialog answers queued by the automation channel (`pick`); always empty without it.
type Picks = Arc<Mutex<VecDeque<Option<Vec<PathBuf>>>>>;

pub struct DialogRequest {
    kind: DialogKind,
    done: DialogDone,
}

impl std::fmt::Debug for DialogRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DialogRequest")
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Debug)]
enum DialogKind {
    Open {
        filters: Vec<FileFilter>,
        multiple: bool,
    },
    Folder,
    Save {
        name: String,
        filters: Vec<FileFilter>,
    },
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
        log.line(
            "shell",
            &format!("spell checking: {}", languages.join(", ")),
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
    );
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
    let devtools = manifest.webview.devtools.enabled(|k| std::env::var(k).ok());
    if let Some(settings) = WebViewExt::settings(&webview) {
        settings.set_enable_developer_extras(devtools);
        settings.set_enable_back_forward_navigation_gestures(false);
        settings.set_javascript_can_open_windows_automatically(false);
    }
    webview.set_background_color(&background(&window));
    install_policy(&webview, &origin, log.clone());
    install_context_menu(&webview, manifest.webview.context_menu);
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

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let stop = |control_flow: &mut ControlFlow, code: i32| {
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
            }
            Event::UserEvent(UserEvent::Dialog(request)) => {
                let queued = picks.lock().unwrap().pop_front();
                match queued {
                    Some(answer) => (request.done)(answer),
                    None => open_dialog(&window, request),
                }
            }
            #[cfg(all(feature = "automation", debug_assertions))]
            Event::UserEvent(UserEvent::Automation(command)) => {
                automation(&webview, &picks, command);
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

/// The window's raw handles, for parenting a portal dialog to it.
struct Parent(RawWindowHandle, RawDisplayHandle);

// SAFETY: the handles are plain identifiers copied from the live window on the UI thread; the
// dialog only uses them to name its parent to the portal while the window still exists.
unsafe impl Send for Parent {}
unsafe impl Sync for Parent {}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: see above.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: see above.
        Ok(unsafe { DisplayHandle::borrow_raw(self.1) })
    }
}

/// Builds the dialog on the UI thread (parented to the window) and waits for it on a helper
/// thread, so the UI never blocks on a dialog.
fn open_dialog(window: &Window, request: DialogRequest) {
    let mut dialog = rfd::AsyncFileDialog::new();
    if let (Ok(w), Ok(d)) = (window.window_handle(), window.display_handle()) {
        dialog = dialog.set_parent(&Parent(w.as_raw(), d.as_raw()));
    }
    let add_filters = |mut dialog: rfd::AsyncFileDialog, filters: &[FileFilter]| {
        for f in filters {
            let ext: Vec<&str> = f.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&f.name, &ext);
        }
        dialog
    };
    let DialogRequest { kind, done } = request;
    let dialog = match &kind {
        DialogKind::Open { filters, .. } => add_filters(dialog, filters),
        DialogKind::Folder => dialog,
        DialogKind::Save { name, filters } => add_filters(dialog, filters).set_file_name(name),
    };

    std::thread::spawn(move || {
        let picked = match kind {
            DialogKind::Open {
                multiple: false, ..
            } => pollster::block_on(dialog.pick_file()).map(|f| vec![f.path().to_path_buf()]),
            DialogKind::Open { multiple: true, .. } => pollster::block_on(dialog.pick_files())
                .map(|fs| fs.iter().map(|f| f.path().to_path_buf()).collect()),
            DialogKind::Folder => {
                pollster::block_on(dialog.pick_folder()).map(|f| vec![f.path().to_path_buf()])
            }
            DialogKind::Save { .. } => {
                pollster::block_on(dialog.save_file()).map(|f| vec![f.path().to_path_buf()])
            }
        };
        done(picked);
    });
}

/// Runs one automation command on the UI thread.
#[cfg(all(feature = "automation", debug_assertions))]
fn automation(webview: &WebView, picks: &Picks, command: crate::automation::Command) {
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
