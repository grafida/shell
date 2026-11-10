//! The macOS shell: a tao window holding a `WKWebView` the shell builds itself, through objc2
//! (ADR 0001, grafida/grafida#88). wry and muda are not used: the shell owns the
//! `WKWebViewConfiguration`, the `WKURLSchemeHandler`, the navigation and UI delegates, a
//! `WKWebView` subclass (drops, context menu) and the AppKit menu bar.
//!
//! Everything WebKit- and AppKit-facing happens on the main thread. Worker I/O runs off it, and its
//! answers come back through tao's event loop.

mod alert;
mod keys;
mod menu;
mod scheme;
mod webview;

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use objc2::{
    ClassType, MainThreadMarker, MainThreadOnly,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSPasteboard, NSView, NSWorkspace};
use objc2_foundation::{
    NSArray, NSBundle, NSNumber, NSObjectNSKeyValueCoding, NSString, NSURL, NSURLRequest,
    NSUserDefaults,
};
use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration};
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::macos::{WindowBuilderExtMacOS, WindowExtMacOS},
    window::{Theme as TaoTheme, WindowBuilder},
};

use crate::{
    bridge::{Bridge, ClipboardDone, DialogDone, FileFilter, Host},
    cli::Launch,
    dialog::{self, DialogKind, DialogRequest, Picks},
    http::{Request, Response},
    log::Log,
    manifest::{Manifest, MenuPreset, Theme},
    supervisor::{self, Notice, Supervisor},
};

pub use alert::alert;
use scheme::{Scheme, SchemeHandler};
use webview::{Delegate, ShellWebView};

pub enum UserEvent {
    /// A request whose body was read off the main thread, ready for the bridge.
    Accept(u64, Request),
    Respond(u64, Response),
    Worker(Notice),
    SetTitle(String),
    SetTheme(Theme),
    Dialog(DialogRequest),
    ReadClipboard(ClipboardDone),
    About,
    /// A parked menu shortcut got no verdict from the page in time (see `keys`).
    KeyTimeout(u64),
    Quit,
    #[cfg(all(feature = "automation", debug_assertions))]
    Automation(crate::automation::Command),
}

/// Process-wide settings, before any thread exists.
///
/// WKWebView gates all continuous spell checking on the `WebContinuousSpellCheckingEnabled` user
/// default, which WebKit reads once and caches; nothing sets it for an app that has never had a
/// Spelling menu. The shell sets it in its own defaults domain (never the global one) on every
/// launch when the manifest wants spell checking, as Grafida's `MacSpellCheck` does under Boson, so
/// the page's `spellcheck` attribute is what turns checking on and off. Edit ▸ Spelling and Grammar
/// ▸ Check Spelling While Typing still toggles it for the session.
///
/// # Safety
/// Kept `unsafe` to match the Linux signature (which sets environment variables).
pub unsafe fn prepare(manifest: &Manifest) -> Vec<String> {
    let mut notes = Vec::new();
    if manifest.webview.spell_check {
        NSUserDefaults::standardUserDefaults().setBool_forKey(
            true,
            &NSString::from_str("WebContinuousSpellCheckingEnabled"),
        );
        notes.push("spell checking: WebContinuousSpellCheckingEnabled set".into());
    }
    notes
}

/// Runs the app until the window closes. Returns the process exit status.
pub fn run(launch: Launch, log: Arc<Log>) -> i32 {
    let Some(mtm) = MainThreadMarker::new() else {
        alert(
            &launch.manifest.title,
            "The shell must run on the main thread.",
            "",
        );
        return 1;
    };
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
        .with_theme(tao_theme(manifest.window.theme))
        .with_automatic_window_tabbing(false);
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
    let host = Arc::new(MacHost {
        proxy: Mutex::new(proxy.clone()),
    });
    let bridge = Bridge::new(
        origin.clone(),
        manifest.worker.max_body_bytes,
        manifest.no_store_paths.clone(),
        host,
        supervisor.clone(),
    );
    let scheme = Scheme::new(bridge, proxy.clone(), log.clone());

    // ── The configuration: everything that must precede the webview ─────────────────────────
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    let scheme_name = NSString::from_str(&manifest.scheme);
    // WebKit raises for a scheme it handles itself (http, https, file, about, …).
    if unsafe { WKWebView::handlesURLScheme(&scheme_name, mtm) } {
        supervisor.shutdown();
        alert(
            &manifest.title,
            "The app's URL scheme cannot be used.",
            &format!(
                "WebKit handles `{}` itself; the manifest's `scheme` must be one it does not.",
                manifest.scheme
            ),
        );
        return 1;
    }
    let handler = SchemeHandler::new(mtm, scheme.clone());
    unsafe {
        config.setURLSchemeHandler_forURLScheme(
            Some(ProtocolObject::from_ref(&*handler)),
            &scheme_name,
        );
    }
    let devtools = manifest.webview.devtools.enabled(|k| std::env::var(k).ok());
    if devtools {
        let prefs = unsafe { config.preferences() };
        unsafe {
            prefs.setValue_forKey(
                Some(&NSNumber::new_bool(true)),
                &NSString::from_str("developerExtrasEnabled"),
            );
        }
    }
    unsafe {
        let guard = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(&webview::blob_guard(&origin)),
            WKUserScriptInjectionTime::AtDocumentStart,
            false,
        );
        config.userContentController().addUserScript(&guard);
        let reporter = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(keys::REPORTER),
            WKUserScriptInjectionTime::AtDocumentStart,
            false,
        );
        config.userContentController().addUserScript(&reporter);
    }
    let key_state = keys::KeyState::new(proxy.clone());
    let key_reporter = keys::KeyReporter::new(mtm, key_state.clone());
    key_reporter.install(&*unsafe { config.userContentController() });

    // ── The webview ─────────────────────────────────────────────────────────────────────────
    // SAFETY: tao's content view, alive as long as `window`.
    let content: &NSView = unsafe { &*(window.ns_view() as *const NSView) };
    let webview = ShellWebView::new(
        mtm,
        content.bounds(),
        &config,
        scheme.clone(),
        log.clone(),
        manifest.webview.context_menu,
        key_state.clone(),
    );
    webview.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    // Transparent until the page paints: the window's own background, which follows the theme.
    unsafe {
        webview.setValue_forKey(
            Some(&NSNumber::new_bool(false)),
            &NSString::from_str("drawsBackground"),
        );
    }
    if devtools {
        set_inspectable(&webview);
    }
    let delegate = Delegate::new(mtm, origin.clone(), log.clone());
    unsafe {
        webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        webview.setAllowsBackForwardNavigationGestures(false);
    }
    content.addSubview(&webview);
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
    let exit = Rc::new(Cell::new(0));
    let stopped = Cell::new(false);
    let mut menu_target = None;
    let menu_proxy = proxy.clone();
    let fatal_proxy = proxy.clone();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        // Also reached through LoopDestroyed when AppKit terminates the app (Dock ▸ Quit, logout):
        // the worker gets its graceful shutdown and the automation file goes either way.
        let stop = |control_flow: &mut ControlFlow, code: i32| {
            if stopped.replace(true) {
                return;
            }
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
            Event::NewEvents(StartCause::Init) => {
                // After applicationDidFinishLaunching, so nothing replaces the menu bar.
                if manifest.menu.preset == MenuPreset::Standard {
                    menu_target = Some(menu::install(
                        mtm,
                        &manifest.title,
                        &manifest.menu.reserved_chords,
                        menu_proxy.clone(),
                    ));
                }
                for item in menu::strip_reserved(mtm, &manifest.menu.reserved_chords) {
                    log.line(
                        "shell",
                        &format!("menu item {item:?}: its shortcut is reserved for the page"),
                    );
                }
                webview
                    .window()
                    .map(|w| w.makeFirstResponder(Some(&webview)));
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => stop(control_flow, 0),
            Event::LoopDestroyed => stop(control_flow, exit.get()),
            Event::UserEvent(UserEvent::Accept(id, request)) => scheme.handle(id, request),
            Event::UserEvent(UserEvent::Respond(id, response)) => scheme.finish(id, response),
            Event::UserEvent(UserEvent::Worker(Notice::Ready { first: true, .. })) => {
                load(&webview, &start_url);
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
            Event::UserEvent(UserEvent::SetTheme(theme)) => window.set_theme(tao_theme(theme)),
            Event::UserEvent(UserEvent::Dialog(request)) => {
                dialog::answer(&window, &picks, request)
            }
            Event::UserEvent(UserEvent::ReadClipboard(done)) => done(read_clipboard()),
            Event::UserEvent(UserEvent::About) => {
                // The page shows its own About; the shell has no panel of its own.
                evaluate(
                    &webview,
                    "window.dispatchEvent(new CustomEvent('shell:about'))",
                );
            }
            #[cfg(all(feature = "automation", debug_assertions))]
            Event::UserEvent(UserEvent::Automation(command)) => {
                automation(&webview, &picks, &scheme, &log, command);
            }
            Event::UserEvent(UserEvent::KeyTimeout(seq)) => key_state.timeout(mtm, seq),
            Event::UserEvent(UserEvent::Quit) => stop(control_flow, exit.get()),
            _ => {}
        }
        let _keep = (&handler, &delegate, &config, &menu_target, &key_reporter);
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

fn load(webview: &WKWebView, url: &str) {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    let request = NSURLRequest::requestWithURL(&url);
    unsafe { webview.loadRequest(&request) };
}

/// Runs a script in the page, ignoring its result.
fn evaluate(webview: &WKWebView, script: &str) {
    unsafe { webview.evaluateJavaScript_completionHandler(&NSString::from_str(script), None) };
}

/// `isInspectable` (macOS 13.3+) is needed for Safari's Web Inspector on newer systems; older ones
/// go by `developerExtrasEnabled` alone, and lack the selector.
fn set_inspectable(webview: &WKWebView) {
    let object: &AnyObject = webview.as_ref();
    let responds: bool =
        unsafe { objc2::msg_send![object, respondsToSelector: sel!(setInspectable:)] };
    if responds {
        let _: () = unsafe { objc2::msg_send![object, setInspectable: true] };
    }
}

/// e.g. `WebKit 621.1.2, macOS 27.2`: the system WebKit, which a Safari update replaces.
fn webkit_version() -> String {
    // SAFETY: a class object WebKit defines.
    let bundle = unsafe { NSBundle::bundleForClass(WKWebView::class()) };
    let version = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleVersion"))
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|v| v.to_string())
        .unwrap_or_else(|| "?".into());
    let os = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    format!(
        "WebKit {version}, macOS {}.{}.{}",
        os.majorVersion, os.minorVersion, os.patchVersion
    )
}

/// The general pasteboard's text: the shell owns the window, so no helper tool, no prompt.
fn read_clipboard() -> Option<String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard
        .stringForType(&NSString::from_str("public.utf8-plain-text"))
        .map(|s| s.to_string())
}

/// Hands dropped files to the page: tokens, then the `shell:drop` event (`{files: [{token, name,
/// size}], x, y}`, viewport coordinates). Also what the automation channel's `drop` command calls.
fn deliver_drop(
    webview: &WKWebView,
    scheme: &Scheme,
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
    evaluate(
        webview,
        &format!("window.dispatchEvent(new CustomEvent('shell:drop', {{detail: {detail}}}));"),
    );
    log.line("shell", &format!("{count} file(s) dropped at {x},{y}"));
}

/// External URLs the automation channel recorded instead of opening. `None` unless it is open, so
/// an automated run never launches a browser on the desktop it is running on.
static RECORDED_OPENS: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn open_uri(uri: &str) -> Result<(), String> {
    if let Some(recorded) = RECORDED_OPENS.lock().unwrap().as_mut() {
        recorded.push(uri.to_string());
        return Ok(());
    }
    let url = NSURL::URLWithString(&NSString::from_str(uri)).ok_or("not a URL")?;
    if NSWorkspace::sharedWorkspace().openURL(&url) {
        Ok(())
    } else {
        Err("no application opened it".into())
    }
}

/// Selects a file in Finder (or shows a folder's contents).
fn reveal(path: &Path) -> Result<(), String> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace()
        .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    Ok(())
}

/// The platform services behind `_native/*`. Called on the UI thread; anything that needs the
/// window goes through the event loop.
struct MacHost {
    proxy: Mutex<EventLoopProxy<UserEvent>>,
}

impl MacHost {
    fn post(&self, event: UserEvent) {
        let _ = self.proxy.lock().unwrap().send_event(event);
    }
}

impl Host for MacHost {
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

/// Runs one automation command on the UI thread.
#[cfg(all(feature = "automation", debug_assertions))]
fn automation(
    webview: &WKWebView,
    picks: &Picks,
    scheme: &Scheme,
    log: &Log,
    command: crate::automation::Command,
) {
    use block2::RcBlock;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use objc2_web_kit::WKContentWorld;
    use serde_json::{Value, json};

    let crate::automation::Command { request, reply } = command;
    let fail = |e: String| json!({"ok": false, "error": e});
    let error_text = |error: &NSError| {
        let info = error.userInfo();
        info.objectForKey(&NSString::from_str("WKJavaScriptExceptionMessage"))
            .and_then(|m| m.downcast::<NSString>().ok())
            .map(|m| m.to_string())
            .unwrap_or_else(|| error.localizedDescription().to_string())
    };
    match request.get("cmd").and_then(Value::as_str) {
        Some("eval") => {
            let js = request.get("js").and_then(Value::as_str).unwrap_or("");
            // The page's own world, so the script sees the app's globals; the result travels as
            // JSON text rather than as Foundation objects.
            let body = format!(
                "const __shellResult = await (async () => {{\n{js}\n}})();\n\
                 return JSON.stringify(__shellResult === undefined ? null : __shellResult);"
            );
            let reply = Mutex::new(Some(reply));
            let handler = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
                let answer = if let Some(error) = unsafe { error.as_ref() } {
                    json!({"ok": false, "error": error_text(error)})
                } else {
                    let text = unsafe { result.as_ref() }
                        .and_then(|r| r.downcast_ref::<NSString>())
                        .map(|s| s.to_string());
                    let value = text
                        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                        .unwrap_or(Value::Null);
                    json!({"ok": true, "result": value})
                };
                if let Some(reply) = reply.lock().unwrap().take() {
                    let _ = reply.send(answer);
                }
            });
            unsafe {
                webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &NSString::from_str(&body),
                    None::<&NSDictionary<NSString, AnyObject>>,
                    None,
                    &WKContentWorld::pageWorld(MainThreadMarker::new().unwrap()),
                    Some(&handler),
                );
            }
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
            let reply = Mutex::new(Some(reply));
            let handler = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                let answer = match (unsafe { image.as_ref() }, unsafe { error.as_ref() }) {
                    (Some(image), _) => match png(image) {
                        Some(bytes) => match std::fs::write(&path, bytes) {
                            Ok(()) => json!({"ok": true}),
                            Err(e) => json!({"ok": false, "error": e.to_string()}),
                        },
                        None => json!({"ok": false, "error": "could not encode the snapshot"}),
                    },
                    (None, Some(error)) => {
                        json!({"ok": false, "error": error.localizedDescription().to_string()})
                    }
                    (None, None) => json!({"ok": false, "error": "no snapshot"}),
                };
                if let Some(reply) = reply.lock().unwrap().take() {
                    let _ = reply.send(answer);
                }
            });
            unsafe { webview.takeSnapshotWithConfiguration_completionHandler(None, &handler) };

            fn png(image: &NSImage) -> Option<Vec<u8>> {
                let tiff = image.TIFFRepresentation()?;
                let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
                let data = unsafe {
                    rep.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                }?;
                Some(data.to_vec())
            }
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
        Some("menu") => {
            // The live menu bar, AppKit's own additions included: what a key equivalent hits.
            use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu};
            fn walk(menu: &NSMenu, path: &str, out: &mut Vec<Value>) {
                for item in menu.itemArray().iter() {
                    let title = format!("{path}{}", item.title());
                    let mask = item.keyEquivalentModifierMask();
                    let mut mods = Vec::new();
                    for (flag, name) in [
                        (NSEventModifierFlags::Command, "cmd"),
                        (NSEventModifierFlags::Control, "ctrl"),
                        (NSEventModifierFlags::Option, "alt"),
                        (NSEventModifierFlags::Shift, "shift"),
                    ] {
                        if mask.contains(flag) {
                            mods.push(name);
                        }
                    }
                    out.push(json!({
                        "title": title,
                        "key": item.keyEquivalent().to_string(),
                        "modifiers": mods,
                        "action": item.action().map(|s| s.name().to_string_lossy().into_owned()),
                        "enabled": item.isEnabled(),
                    }));
                    if let Some(sub) = item.submenu() {
                        walk(&sub, &format!("{title} ▸ "), out);
                    }
                }
            }
            let bar = NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).mainMenu();
            // `perform: "About Grafida"` clicks that item (searched by title, depth first).
            if let Some(title) = request.get("perform").and_then(Value::as_str) {
                fn find(
                    menu: &NSMenu,
                    title: &str,
                ) -> Option<(objc2::rc::Retained<NSMenu>, isize)> {
                    use objc2::Message;
                    for (i, item) in menu.itemArray().iter().enumerate() {
                        if item.title().to_string() == title {
                            return Some((menu.retain(), i as isize));
                        }
                        if let Some(sub) = item.submenu()
                            && let Some(found) = find(&sub, title)
                        {
                            return Some(found);
                        }
                    }
                    None
                }
                let answer = match bar.as_deref().and_then(|b| find(b, title)) {
                    Some((menu, index)) => {
                        menu.performActionForItemAtIndex(index);
                        json!({"ok": true})
                    }
                    None => fail(format!("no menu item {title:?}")),
                };
                let _ = reply.send(answer);
                return;
            }
            let mut items = Vec::new();
            if let Some(bar) = bar {
                walk(&bar, "", &mut items);
            }
            let _ = reply.send(json!({"ok": true, "result": items}));
        }
        Some("key") => {
            // A synthesized key-down queued for NSApp's sendEvent:, i.e. the path a real key press
            // takes: key equivalents (the menu bar) first, then the first responder (the page).
            use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
            use objc2_foundation::NSPoint;
            let key = request.get("key").and_then(Value::as_str).unwrap_or("");
            // US-layout virtual key codes for the letters a test needs.
            let code: u16 = match key {
                "a" => 0,
                "s" => 1,
                "x" => 7,
                "c" => 8,
                "v" => 9,
                "q" => 12,
                "w" => 13,
                "z" => 6,
                "m" => 46,
                "h" => 4,
                _ => {
                    let _ = reply.send(fail(format!("key: no key code for {key:?}")));
                    return;
                }
            };
            let mut flags = NSEventModifierFlags::empty();
            for m in request
                .get("modifiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                flags |= match m {
                    "cmd" => NSEventModifierFlags::Command,
                    "ctrl" => NSEventModifierFlags::Control,
                    "alt" => NSEventModifierFlags::Option,
                    "shift" => NSEventModifierFlags::Shift,
                    _ => NSEventModifierFlags::empty(),
                };
            }
            let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            let window = webview.window();
            // As after a click on the window: the app active, the window key, the page focused.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            if let Some(w) = &window {
                w.makeKeyAndOrderFront(None);
                w.makeFirstResponder(Some(webview));
            }
            let number = window.as_ref().map_or(0, |w| w.windowNumber());
            let chars = if flags.contains(NSEventModifierFlags::Shift) {
                key.to_uppercase()
            } else {
                key.to_string()
            };
            let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                NSEventType::KeyDown,
                NSPoint::new(0.0, 0.0),
                flags,
                0.0,
                number,
                None,
                &NSString::from_str(&chars),
                // As a real event: Shift still applies to charactersIgnoringModifiers.
                &NSString::from_str(&chars),
                false,
                code,
            );
            let Some(event) = event else {
                let _ = reply.send(fail("could not build the event".into()));
                return;
            };
            let _ = reply.send(json!({
                "ok": true,
                "result": {
                    "keyWindow": app.keyWindow().is_some(),
                    "firstResponder": window
                        .and_then(|w| w.firstResponder())
                        .map(|r| r.class().name().to_string_lossy().into_owned()),
                }
            }));
            // Queued, so it is dispatched as a real key press is: NSApp's currentEvent and all.
            app.postEvent_atStart(&event, false);
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
