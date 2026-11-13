//! The Windows shell: a tao window holding a WebView2 controller the shell creates itself
//! (grafida/grafida#91).
//!
//! wry is not used, as on Linux and macOS (ADR 0001, "Windows"): the shell
//! creates the WebView2 **environment** itself, with the app's scheme registered as a real custom
//! scheme (`CoreWebView2CustomSchemeRegistration`: secure, with an authority, the app's origin
//! allowed), its user data folder under `%LOCALAPPDATA%\<dataDir>\WebView2` (the default beside the
//! executable is not writable under Program Files), and then the **controller** directly on tao's
//! window. Both are created asynchronously and their completion handlers run on the UI thread from
//! the message loop, so the shell never runs a nested message pump.

mod alert;
#[cfg(all(feature = "automation", debug_assertions))]
mod automation;
pub mod environment;
mod scheme;

use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

use tao::{
    dpi::{LogicalSize, PhysicalSize},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowExtWindows,
    window::{Theme as TaoTheme, Window, WindowBuilder},
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, ContextMenuRequestedEventHandler,
    CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, Microsoft::Web::WebView2::Win32::*,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler, ProcessFailedEventHandler,
    take_pwstr,
};
use windows::{
    Win32::{
        Foundation::{HGLOBAL, HWND, LPARAM, RECT, WPARAM},
        System::{
            Com::CoTaskMemFree,
            DataExchange::{
                CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
        UI::{
            Shell::{SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{BOOL, HSTRING, Interface, PCWSTR, PWSTR, w},
};

use crate::{
    bridge::{Bridge, ClipboardDone, DialogDone, FileFilter, Host, utf16_until_nul},
    cli::Launch,
    dialog::{self, DialogKind, DialogRequest, Picks},
    log::Log,
    manifest::Theme,
    navigation::{self, Decision},
    supervisor::{self, Notice, Supervisor},
};

pub use alert::alert;
pub use environment::prepare;
use scheme::{Scheme, SchemeEvent};

/// `EX_UNAVAILABLE`: the WebView2 runtime is missing.
const EXIT_UNAVAILABLE: i32 = 69;

/// Where an absent WebView2 runtime is downloaded from.
const RUNTIME_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// Browser features that only get in an app's way: the selection "mini menu" (wry#535) and the
/// PDF one, and SmartScreen's per-navigation checks (tauri#1345): the page only ever loads the
/// app's own scheme.
const BROWSER_ARGUMENTS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

pub enum UserEvent {
    Scheme(SchemeEvent),
    Worker(Notice),
    SetTitle(String),
    SetTheme(Theme),
    Dialog(DialogRequest),
    ReadClipboard(ClipboardDone),
    /// The WebView2 environment or controller completion handler has stored its result in
    /// [`CREATED`] (COM objects are not `Send`, so they cannot travel in the event).
    Created,
    /// WebView2's browser process exited: the webview is dead and cannot come back by itself.
    WebviewGone(String),
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

enum Created {
    Environment(windows::core::Result<ICoreWebView2Environment>),
    Controller(windows::core::Result<ICoreWebView2Controller>),
}

thread_local! {
    /// Results of the asynchronous creation steps, handed to the event loop (UI thread only).
    static CREATED: RefCell<Vec<Created>> = const { RefCell::new(Vec::new()) };
}

/// URLs the automation channel records instead of opening; `None` = open them.
static RECORDED_OPENS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The window's own icon (title bar, Alt+Tab) from the executable's first icon, which
/// shell-packager writes into the app's shell: the taskbar and Task Manager read the file, but a
/// window has the generic one until it is given its own (grafida/grafida#91). The bare shell has
/// no icon resource and keeps the generic one.
fn set_window_icon(hwnd: HWND) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::{
        Shell::ExtractIconExW,
        WindowsAndMessaging::{HICON, ICON_BIG, ICON_SMALL, SendMessageW, WM_SETICON},
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let (mut big, mut small) = (HICON::default(), HICON::default());
    // SAFETY: the path is NUL-terminated and outlives the call; one icon of each size is asked
    // for. The icons live as long as the window (the process), so they are never destroyed.
    let found = unsafe {
        ExtractIconExW(
            PCWSTR(path.as_ptr()),
            0,
            Some(&mut big),
            Some(&mut small),
            1,
        )
    };
    if found == 0 {
        return;
    }
    for (kind, icon) in [(ICON_BIG, big), (ICON_SMALL, small)] {
        if !icon.is_invalid() {
            // SAFETY: a live window of this thread and a live icon.
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                )
            };
        }
    }
}

/// The installed WebView2 runtime's version, or why there is none.
pub fn runtime_version() -> Result<String, String> {
    let mut version = PWSTR::null();
    // SAFETY: a plain call into the statically linked loader; the string is ours to free.
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) }
        .map_err(|e| e.message())?;
    let version = take_pwstr(version);
    if version.is_empty() {
        Err("no WebView2 runtime is installed".into())
    } else {
        Ok(version)
    }
}

/// Runs the app until the window closes. Returns the process exit status.
pub fn run(launch: Launch, log: Arc<Log>) -> i32 {
    let manifest = launch.manifest.clone();
    let origin = manifest.origin();

    let runtime = match runtime_version() {
        Ok(v) => v,
        Err(e) => {
            log.line("shell", &format!("no WebView2 runtime: {e}"));
            alert(
                &manifest.title,
                &format!(
                    "{} needs the Microsoft Edge WebView2 Runtime, which is not installed.",
                    manifest.title
                ),
                &format!(
                    "Download the Evergreen runtime from {RUNTIME_URL}, install it, and start {} again.",
                    manifest.title
                ),
            );
            return EXIT_UNAVAILABLE;
        }
    };

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
    let hwnd = HWND(window.hwnd() as _);
    set_window_icon(hwnd);

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

    // ── The WebView2 environment (asynchronous; see `UserEvent::Created`) ───────────────────
    let data_dir = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(manifest.data_dir())
        .join("WebView2");
    if let Err(e) = create_environment(&manifest.scheme, &origin, &data_dir, proxy.clone()) {
        alert(
            &manifest.title,
            "The WebView2 environment could not be created.",
            &e.message(),
        );
        return 1;
    }

    let host = Arc::new(WindowsHost {
        proxy: Mutex::new(proxy.clone()),
        runtime: runtime.clone(),
    });
    let bridge = Some(
        Bridge::new(
            origin.clone(),
            manifest.worker.max_body_bytes,
            manifest.no_store_paths.clone(),
            host,
            supervisor.clone(),
        )
        .with_frame_guard(manifest.webview.frame_guard),
    );
    let devtools = manifest.webview.devtools.enabled(|k| std::env::var(k).ok());
    log.line(
        "shell",
        &format!(
            "{} {} on WebView2 {runtime} (devtools {}), origin {origin}, profile {}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            if devtools { "on" } else { "off" },
            data_dir.display()
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
    let context_menu = manifest.webview.context_menu;
    let frame_guard = manifest.webview.frame_guard;
    let mut bridge = bridge;
    let mut scheme_handler: Option<Rc<Scheme>> = None;
    let mut controller: Option<ICoreWebView2Controller> = None;
    let mut webview: Option<ICoreWebView2> = None;
    let mut worker_ready = false;
    let mut exit = 0;
    let fatal_proxy = Mutex::new(proxy.clone());
    let respond_proxy = Arc::new(Mutex::new(proxy.clone()));

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let stop = |control_flow: &mut ControlFlow,
                    code: i32,
                    controller: &Option<ICoreWebView2Controller>| {
            #[cfg(all(feature = "automation", debug_assertions))]
            if let Some(channel) = &_automation {
                channel.close();
            }
            // The start of the stop path, for the log: the worker's "stopped" line follows it.
            log.line("shell", &format!("stopping (exit status {code})"));
            window.set_visible(false);
            if let Some(c) = controller {
                // SAFETY: COM call on the UI thread.
                let _ = unsafe { c.Close() };
            }
            supervisor.shutdown();
            *control_flow = ControlFlow::ExitWithCode(code);
        };

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => stop(control_flow, exit, &controller),
                WindowEvent::Resized(size) => fit(&controller, size),
                WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    fit(&controller, *new_inner_size)
                }
                WindowEvent::Moved(_) => {
                    if let Some(c) = &controller {
                        // SAFETY: COM call on the UI thread. Keeps popups and the IME in place.
                        let _ = unsafe { c.NotifyParentWindowPositionChanged() };
                    }
                }
                WindowEvent::Focused(true) => {
                    if let Some(c) = &controller {
                        // SAFETY: COM call on the UI thread. Fails while minimised; harmless.
                        let _ = unsafe { c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
                    }
                }
                _ => {}
            },
            Event::UserEvent(UserEvent::Created) => {
                for created in CREATED.with(|c| std::mem::take(&mut *c.borrow_mut())) {
                    match created {
                        Created::Environment(Ok(env)) => {
                            if let Some(bridge) = bridge.take() {
                                let proxy = respond_proxy.clone();
                                scheme_handler = Some(Scheme::new(
                                    bridge,
                                    env.clone(),
                                    Arc::new(move |e| {
                                        let _ =
                                            proxy.lock().unwrap().send_event(UserEvent::Scheme(e));
                                    }),
                                ));
                            }
                            if let Err(e) = create_controller(&env, hwnd, proxy.clone()) {
                                alert(
                                    &title,
                                    "The WebView2 controller could not be created.",
                                    &e.message(),
                                );
                                stop(control_flow, 1, &controller);
                            }
                        }
                        Created::Controller(Ok(c)) => {
                            let options = WebviewOptions {
                                origin: &origin,
                                devtools,
                                context_menu,
                                frame_guard,
                            };
                            match configure(
                                &c,
                                &window,
                                &options,
                                scheme_handler.as_ref(),
                                &log,
                                proxy.clone(),
                            ) {
                                Ok(w) => {
                                    if worker_ready {
                                        navigate(&w, &start_url);
                                    }
                                    webview = Some(w);
                                    controller = Some(c);
                                }
                                Err(e) => {
                                    alert(&title, "The webview could not be set up.", &e.message());
                                    stop(control_flow, 1, &controller);
                                }
                            }
                        }
                        Created::Environment(Err(e)) | Created::Controller(Err(e)) => {
                            log.line(
                                "shell",
                                &format!("WebView2 creation failed: {}", e.message()),
                            );
                            alert(&title, "The webview could not be created.", &e.message());
                            stop(control_flow, 1, &controller);
                        }
                    }
                }
            }
            Event::UserEvent(UserEvent::Scheme(SchemeEvent::Body(id, body))) => {
                if let Some(s) = &scheme_handler {
                    s.body_read(id, body);
                }
            }
            Event::UserEvent(UserEvent::Scheme(SchemeEvent::Respond(id, response))) => {
                if let Some(s) = &scheme_handler {
                    s.finish(id, response);
                }
            }
            Event::UserEvent(UserEvent::Worker(Notice::Ready { first: true, .. })) => {
                worker_ready = true;
                if let Some(w) = &webview {
                    navigate(w, &start_url);
                }
            }
            Event::UserEvent(UserEvent::Worker(Notice::Ready { .. })) => {}
            Event::UserEvent(UserEvent::Worker(Notice::Restarting { .. })) => {}
            Event::UserEvent(UserEvent::Worker(Notice::Fatal(failure))) => {
                exit = 1;
                fatal(
                    hwnd,
                    &title,
                    &format!("{title} stopped working: {}.", failure.reason),
                    failure.stderr_tail.join("\n"),
                    &fatal_proxy,
                );
            }
            Event::UserEvent(UserEvent::WebviewGone(reason)) => {
                exit = 1;
                fatal(
                    hwnd,
                    &title,
                    &format!("{title} stopped working: {reason}."),
                    String::new(),
                    &fatal_proxy,
                );
            }
            Event::UserEvent(UserEvent::SetTitle(t)) => window.set_title(&t),
            Event::UserEvent(UserEvent::SetTheme(theme)) => window.set_theme(tao_theme(theme)),
            Event::UserEvent(UserEvent::Dialog(request)) => {
                dialog::answer(&window, &picks, request)
            }
            Event::UserEvent(UserEvent::ReadClipboard(done)) => done(read_clipboard(hwnd)),
            #[cfg(all(feature = "automation", debug_assertions))]
            Event::UserEvent(UserEvent::Automation(command)) => {
                automation::run(webview.as_ref(), hwnd, &picks, command)
            }
            Event::UserEvent(UserEvent::Quit) => stop(control_flow, exit, &controller),
            _ => {}
        }
    });
}

/// The native alert for a failure the app cannot go on after, then [`UserEvent::Quit`]. A message
/// box runs its own modal loop; on a thread of its own it cannot re-enter tao's event handler.
fn fatal(
    hwnd: HWND,
    title: &str,
    message: &str,
    detail: String,
    proxy: &Mutex<EventLoopProxy<UserEvent>>,
) {
    let (title, message) = (title.to_string(), message.to_string());
    let parent = hwnd.0 as isize;
    let quit = Mutex::new(proxy.lock().unwrap().clone());
    std::thread::spawn(move || {
        alert::show(HWND(parent as _), &title, &message, &detail, || {
            let _ = quit.lock().unwrap().send_event(UserEvent::Quit);
        });
    });
}

/// Starts creating the environment; the result arrives as [`UserEvent::Created`].
fn create_environment(
    scheme: &str,
    origin: &str,
    data_dir: &Path,
    proxy: EventLoopProxy<UserEvent>,
) -> windows::core::Result<()> {
    let options = CoreWebView2EnvironmentOptions::default();
    // SAFETY: plain setters on our own options object, then the loader call; the completion
    // handler runs later on this (the UI) thread.
    unsafe {
        let registration = CoreWebView2CustomSchemeRegistration::new(scheme.to_string());
        registration.set_treat_as_secure(true);
        registration.set_has_authority_component(true);
        registration.set_allowed_origins(vec![origin.to_string()]);
        options.set_scheme_registrations(vec![Some(registration.into())]);
        options.set_additional_browser_arguments(BROWSER_ARGUMENTS.to_string());
        // Debug builds: SHELL_WINDOWS_LANGUAGE=none passes no language, any other value that one.
        let language = match std::env::var("SHELL_WINDOWS_LANGUAGE") {
            Ok(v) if cfg!(debug_assertions) && v == "none" => None,
            Ok(v) if cfg!(debug_assertions) => Some(v),
            _ => ui_language(),
        };
        if let Some(language) = language {
            options.set_language(language);
        }
        if !cfg!(debug_assertions) {
            // The Evergreen runtime only: never a Beta/Dev/Canary Edge that happens to be installed.
            options.set_release_channels(COREWEBVIEW2_RELEASE_CHANNELS_STABLE);
        }
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(data_dir.as_os_str()),
            &ICoreWebView2EnvironmentOptions::from(options),
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |code, env| {
                let result = code.and_then(|()| {
                    env.ok_or_else(|| {
                        windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
                    })
                });
                CREATED.with(|c| c.borrow_mut().push(Created::Environment(result)));
                let _ = proxy.send_event(UserEvent::Created);
                Ok(())
            })),
        )
    }
}

/// Starts creating the controller on the window; the result arrives as [`UserEvent::Created`].
fn create_controller(
    env: &ICoreWebView2Environment,
    hwnd: HWND,
    proxy: EventLoopProxy<UserEvent>,
) -> windows::core::Result<()> {
    // SAFETY: COM call on the UI thread; the handler runs later on it.
    unsafe {
        env.CreateCoreWebView2Controller(
            hwnd,
            &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |code, c| {
                let result = code.and_then(|()| {
                    c.ok_or_else(|| {
                        windows::core::Error::from(windows::Win32::Foundation::E_POINTER)
                    })
                });
                CREATED.with(|cr| cr.borrow_mut().push(Created::Controller(result)));
                let _ = proxy.send_event(UserEvent::Created);
                Ok(())
            })),
        )
    }
}

/// The user's UI language as a BCP 47 tag, for WebView2's own strings (the context menu).
fn ui_language() -> Option<String> {
    use windows::Win32::Globalization::{
        GetUserDefaultUILanguage, LCIDToLocaleName, LOCALE_ALLOW_NEUTRAL_NAMES,
    };
    let mut name = [0u16; 85];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide; the length returned includes the NUL.
    let n = unsafe {
        LCIDToLocaleName(
            u32::from(GetUserDefaultUILanguage()),
            Some(&mut name),
            LOCALE_ALLOW_NEUTRAL_NAMES,
        )
    };
    (n > 1).then(|| String::from_utf16_lossy(&name[..n as usize - 1]))
}

/// What the manifest says about the webview.
#[derive(Clone, Copy)]
struct WebviewOptions<'a> {
    origin: &'a str,
    devtools: bool,
    context_menu: bool,
    frame_guard: bool,
}

/// Settings, bounds and every handler on a new controller. Returns its webview.
fn configure(
    controller: &ICoreWebView2Controller,
    window: &Window,
    options: &WebviewOptions,
    scheme: Option<&Rc<Scheme>>,
    log: &Arc<Log>,
    proxy: EventLoopProxy<UserEvent>,
) -> windows::core::Result<ICoreWebView2> {
    let WebviewOptions {
        origin,
        devtools,
        context_menu,
        frame_guard,
    } = *options;
    // SAFETY: COM calls on the UI thread, on objects created on it.
    unsafe {
        if let Ok(c2) = controller.cast::<ICoreWebView2Controller2>() {
            let _ = c2.SetDefaultBackgroundColor(background(window));
        }
        fit(&Some(controller.clone()), window.inner_size());
        controller.SetIsVisible(true)?;
        let webview = controller.CoreWebView2()?;

        let settings = webview.Settings()?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetAreDevToolsEnabled(devtools)?;
        settings.SetAreDefaultContextMenusEnabled(context_menu)?;
        settings.SetIsZoomControlEnabled(false)?;
        if let Ok(s) = settings.cast::<ICoreWebView2Settings3>() {
            s.SetAreBrowserAcceleratorKeysEnabled(browser_accelerator_keys())?;
        }
        if let Ok(s) = settings.cast::<ICoreWebView2Settings4>() {
            s.SetIsGeneralAutofillEnabled(false)?;
            s.SetIsPasswordAutosaveEnabled(false)?;
        }
        if let Ok(s) = settings.cast::<ICoreWebView2Settings5>() {
            s.SetIsPinchZoomEnabled(false)?;
        }
        if let Ok(s) = settings.cast::<ICoreWebView2Settings6>() {
            s.SetIsSwipeNavigationEnabled(false)?;
        }

        if let Some(scheme) = scheme {
            let name = origin.split("://").next().unwrap_or_default();
            scheme.install(&webview, name)?;
        }
        install_policy(&webview, origin, log.clone())?;
        if frame_guard {
            install_frame_buster(&webview, origin, log.clone())?;
        }
        if context_menu {
            install_context_menu(&webview, devtools, log.clone())?;
        }
        install_key_probe(controller, log.clone())?;
        {
            let log = log.clone();
            let mut token = Default::default();
            let proxy = Mutex::new(proxy);
            webview.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |sender, args| {
                    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                    if let Some(args) = args {
                        let _ = args.ProcessFailedKind(&mut kind);
                    }
                    log.line(
                        "shell",
                        &format!("a WebView2 process failed: {}", process_failure(kind)),
                    );
                    match kind {
                        // Nothing left to show the page in: say so and stop, as for a worker
                        // that cannot be restarted (otherwise the window stays blank for good).
                        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => {
                            let _ = proxy.lock().unwrap().send_event(UserEvent::WebviewGone(
                                process_failure(kind).to_string(),
                            ));
                        }
                        // The page's own renderer: WebView2 starts a new one on the next
                        // navigation, so load the page again.
                        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
                        | COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
                            if let Some(w) = sender {
                                let _ = w.Reload();
                            }
                        }
                        // A frame's, the GPU's or a utility process: WebView2 recovers by itself.
                        _ => {}
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        Ok(webview)
    }
}

/// [`navigation::frame_buster`] in every document the webview creates, child frames included
/// (WebView2's documentation: a document-created script runs in iframes too; `framing.js`'s
/// `foreign, worker allows` row is what shows it), before the page's own scripts
/// (security.md L1). The main frame does not need it (the script is inert there).
fn install_frame_buster(
    webview: &ICoreWebView2,
    origin: &str,
    log: Arc<Log>,
) -> windows::core::Result<()> {
    let script = HSTRING::from(navigation::frame_buster(origin));
    // SAFETY: a COM call on the UI thread; the handler also runs on it.
    unsafe {
        webview.AddScriptToExecuteOnDocumentCreated(
            &script,
            &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                move |code, _id| {
                    if let Err(e) = code {
                        log.line(
                            "shell",
                            &format!("the frame guard script was not added: {}", e.message()),
                        );
                    }
                    Ok(())
                },
            )),
        )
    }
}

fn process_failure(kind: COREWEBVIEW2_PROCESS_FAILED_KIND) -> &'static str {
    match kind {
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED => "the browser process exited",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED => "the page's renderer exited",
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE => {
            "the page's renderer stopped responding"
        }
        COREWEBVIEW2_PROCESS_FAILED_KIND_FRAME_RENDER_PROCESS_EXITED => "a frame's renderer exited",
        COREWEBVIEW2_PROCESS_FAILED_KIND_GPU_PROCESS_EXITED => "the GPU process exited",
        _ => "a utility process exited",
    }
}

fn navigate(webview: &ICoreWebView2, url: &str) {
    // SAFETY: COM call on the UI thread.
    let _ = unsafe { webview.Navigate(&HSTRING::from(url)) };
}

/// Makes the webview fill the window's client area; hides it while the window is minimised (a
/// hidden webview stops rendering, which is what idle CPU wants).
fn fit(controller: &Option<ICoreWebView2Controller>, size: PhysicalSize<u32>) {
    let Some(c) = controller else { return };
    // SAFETY: COM calls on the UI thread.
    unsafe {
        if size.width == 0 || size.height == 0 {
            let _ = c.SetIsVisible(false);
            return;
        }
        let _ = c.SetIsVisible(true);
        let _ = c.SetBounds(RECT {
            left: 0,
            top: 0,
            right: size.width as i32,
            bottom: size.height as i32,
        });
    }
}

/// Whether the browser's own shortcuts (reload, print, find, zoom, back/forward, devtools keys) stay
/// on. Off: an app is not a browser. A debug build can turn them on with
/// `SHELL_WINDOWS_BROWSER_KEYS=on`, for the key probe (grafida/grafida#91).
fn browser_accelerator_keys() -> bool {
    cfg!(debug_assertions) && std::env::var("SHELL_WINDOWS_BROWSER_KEYS").is_ok_and(|v| v == "on")
}

/// The key probe (debug builds, `SHELL_WINDOWS_KEY_LOG=1`): logs every key WebView2 offers the host
/// as an accelerator, without handling any. It answers which keys WebView2 takes before the page.
fn install_key_probe(
    controller: &ICoreWebView2Controller,
    log: Arc<Log>,
) -> windows::core::Result<()> {
    if !(cfg!(debug_assertions) && std::env::var("SHELL_WINDOWS_KEY_LOG").is_ok_and(|v| v == "1")) {
        return Ok(());
    }
    let mut token = Default::default();
    // SAFETY: COM calls on the UI thread.
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &webview2_com::AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                    let mut key = 0u32;
                    let _ = args.KeyEventKind(&mut kind);
                    let _ = args.VirtualKey(&mut key);
                    let mods = modifiers();
                    log.line(
                        "keys",
                        &format!("accelerator kind {} vk 0x{key:02x} {mods}", kind.0),
                    );
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

fn modifiers() -> String {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let down = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| {
        // SAFETY: plain Win32 call.
        (unsafe { GetKeyState(i32::from(vk.0)) }) < 0
    };
    let mut m = Vec::new();
    if down(VK_CONTROL) {
        m.push("ctrl");
    }
    if down(VK_SHIFT) {
        m.push("shift");
    }
    if down(VK_MENU) {
        m.push("alt");
    }
    m.join("+")
}

/// The navigation policy (`src/navigation.rs`) on WebView2's events. Unlike WebKitGTK, WebView2
/// says which frame navigates: `NavigationStarting` is the main frame (nothing off-origin, ever),
/// `FrameNavigationStarting` a subframe (the layered rule, so an embed loads in its `<iframe>`).
fn install_policy(
    webview: &ICoreWebView2,
    origin: &str,
    log: Arc<Log>,
) -> windows::core::Result<()> {
    // SAFETY: COM calls on the UI thread; every handler also runs on it.
    unsafe {
        let mut token = Default::default();
        let o = origin.to_string();
        let l = log.clone();
        webview.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let (url, user) = navigation_of(&args)?;
                    apply(
                        navigation::for_main_frame_navigation(&url, &o, user),
                        &url,
                        &l,
                        || args.SetCancel(true),
                    );
                }
                Ok(())
            })),
            &mut token,
        )?;

        let o = origin.to_string();
        let l = log.clone();
        webview.add_FrameNavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let (url, user) = navigation_of(&args)?;
                    apply(navigation::for_navigation(&url, &o, user), &url, &l, || {
                        args.SetCancel(true)
                    });
                }
                Ok(())
            })),
            &mut token,
        )?;

        let o = origin.to_string();
        let l = log.clone();
        webview.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    // Handled, with no new window: WebView2 opens nothing.
                    args.SetHandled(true)?;
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let url = take_pwstr(uri);
                    let mut user = BOOL::default();
                    args.IsUserInitiated(&mut user)?;
                    apply(
                        navigation::for_new_window(&url, &o, user.as_bool()),
                        &url,
                        &l,
                        || Ok(()),
                    );
                }
                Ok(())
            })),
            &mut token,
        )?;

        if let Ok(w4) = webview.cast::<ICoreWebView2_4>() {
            let l = log.clone();
            w4.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        args.SetCancel(true)?;
                        l.line(
                            "shell",
                            "refused a download: the shell does not save files for the page",
                        );
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
    }
    Ok(())
}

/// The URL and the user-gesture flag of a navigation.
///
/// # Safety
/// COM calls on the UI thread.
unsafe fn navigation_of(
    args: &ICoreWebView2NavigationStartingEventArgs,
) -> windows::core::Result<(String, bool)> {
    // SAFETY: as the caller.
    unsafe {
        let mut uri = PWSTR::null();
        args.Uri(&mut uri)?;
        let mut user = BOOL::default();
        args.IsUserInitiated(&mut user)?;
        Ok((take_pwstr(uri), user.as_bool()))
    }
}

fn apply(
    decision: Decision,
    url: &str,
    log: &Log,
    cancel: impl FnOnce() -> windows::core::Result<()>,
) {
    match decision {
        Decision::Allow => {}
        Decision::OpenExternally(url) => {
            let _ = cancel();
            if let Err(e) = open_uri(&url) {
                log.line("shell", &format!("could not open {url}: {e}"));
            }
        }
        Decision::Refuse => {
            let _ = cancel();
            log.line("shell", &format!("refused a navigation to {url}"));
        }
    }
}

/// The context-menu items an app keeps, by WebView2's names: editing, the spelling suggestions,
/// emoji, and copying a link or an image. Everything else (back, reload, save as, print, share,
/// "open in new window", web capture, and whatever a future runtime adds) is removed, so the
/// menu never offers to navigate, reload, open a window or download.
const CONTEXT_MENU_KEEP: [&str; 14] = [
    "undo",
    "redo",
    "cut",
    "copy",
    "paste",
    "pasteAndMatchStyle",
    "selectAll",
    "emoji",
    "spellCheck",
    "addToDictionary",
    "copyLinkLocation",
    "copyImage",
    "copyImageLocation",
    "other",
];

/// Whether a context-menu item stays: by name, or a spelling suggestion (WebView2 names those
/// after nothing we can list), or "Inspect" when devtools are on.
fn keep_menu_item(name: &str, devtools: bool) -> bool {
    // WebView2 names a spelling suggestion "spellcheck", all lower case (seen on runtime 157.0.4341.42);
    // matched without regard to case, so a renamed one still stays.
    CONTEXT_MENU_KEEP.contains(&name)
        || name.to_ascii_lowercase().starts_with("spellcheck")
        || (devtools && name == "inspectElement")
}

fn install_context_menu(
    webview: &ICoreWebView2,
    devtools: bool,
    log: Arc<Log>,
) -> windows::core::Result<()> {
    let Ok(w11) = webview.cast::<ICoreWebView2_11>() else {
        log.line(
            "shell",
            "this WebView2 runtime cannot filter its context menu",
        );
        return Ok(());
    };
    let probe =
        cfg!(debug_assertions) && std::env::var("SHELL_WINDOWS_MENU_LOG").is_ok_and(|v| v == "1");
    let mut token = Default::default();
    // SAFETY: COM calls on the UI thread.
    unsafe {
        w11.add_ContextMenuRequested(
            &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let items = args.MenuItems()?;
                let mut count = 0u32;
                items.Count(&mut count)?;
                let mut seen = Vec::new();
                let mut kept = Vec::new();
                let mut i = 0;
                while i < count {
                    let item = items.GetValueAtIndex(i)?;
                    let mut name = PWSTR::null();
                    item.Name(&mut name)?;
                    let name = take_pwstr(name);
                    let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
                    item.Kind(&mut kind)?;
                    let separator = kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR;
                    seen.push(name.clone());
                    if separator || keep_menu_item(&name, devtools) {
                        if probe && !separator {
                            let mut label = PWSTR::null();
                            item.Label(&mut label)?;
                            kept.push(format!("{name}={}", take_pwstr(label)));
                        }
                        i += 1;
                    } else {
                        items.RemoveValueAtIndex(i)?;
                        count -= 1;
                    }
                }
                tidy_separators(&items)?;
                if probe {
                    log.line(
                        "menu",
                        &format!(
                            "context menu items: {}; kept: {}",
                            seen.join(", "),
                            kept.join(" | ")
                        ),
                    );
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// No separator first, last, or next to another one.
///
/// # Safety
/// COM calls on the UI thread.
unsafe fn tidy_separators(
    items: &ICoreWebView2ContextMenuItemCollection,
) -> windows::core::Result<()> {
    // SAFETY: as the caller.
    unsafe {
        let mut count = 0u32;
        items.Count(&mut count)?;
        let mut previous_separator = true; // a separator at the top goes too
        let mut i = 0;
        while i < count {
            let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
            items.GetValueAtIndex(i)?.Kind(&mut kind)?;
            let separator = kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR;
            if separator && (previous_separator || i + 1 == count) {
                items.RemoveValueAtIndex(i)?;
                count -= 1;
                continue;
            }
            previous_separator = separator;
            i += 1;
        }
    }
    Ok(())
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
fn background(window: &Window) -> COREWEBVIEW2_COLOR {
    match window.theme() {
        TaoTheme::Dark => COREWEBVIEW2_COLOR {
            A: 255,
            R: 33,
            G: 33,
            B: 36,
        },
        _ => COREWEBVIEW2_COLOR {
            A: 255,
            R: 250,
            G: 250,
            B: 250,
        },
    }
}

/// Opens an http(s) URL in the OS browser, or records it while the automation channel is open.
fn open_uri(uri: &str) -> Result<(), String> {
    if let Some(recorded) = RECORDED_OPENS.lock().unwrap().as_mut() {
        recorded.push(uri.to_string());
        return Ok(());
    }
    // SAFETY: plain Win32 call; the strings outlive it.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(uri),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute's documented success test: a value greater than 32.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute failed ({})", result.0 as isize))
    }
}

/// Shows a file selected in Explorer.
fn reveal(path: &Path) -> Result<(), String> {
    let mut pidl = std::ptr::null_mut();
    // SAFETY: the item list is ours and freed below; COM is initialised on the UI thread by tao.
    unsafe {
        SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut pidl, 0, None)
            .map_err(|e| e.message())?;
        let shown = SHOpenFolderAndSelectItems(pidl, None, 0).map_err(|e| e.message());
        CoTaskMemFree(Some(pidl as *const _));
        shown
    }
}

/// The clipboard's text (`CF_UNICODETEXT`), or `None` when it holds none or cannot be opened.
/// Another program may hold the clipboard for a moment, so opening it is retried briefly.
fn read_clipboard(owner: HWND) -> Option<String> {
    // SAFETY: plain Win32 calls; the clipboard's global is read by `global_text`.
    unsafe {
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(Some(owner)).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !opened {
            return None;
        }
        let text = (|| {
            IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)).ok()?;
            let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
            global_text(HGLOBAL(handle.0))
        })();
        let _ = CloseClipboard();
        // An empty clipboard is text too: the page tells "nothing to paste" from "unreadable".
        text.or_else(|| Some(String::new()))
    }
}

/// The UTF-16 text in a global memory object, up to its first NUL, or `None` when it cannot be
/// locked. Its owner may have left the terminator out, so the scan never goes past the
/// allocation's `GlobalSize` (security.md L5).
///
/// # Safety
/// `global` must be a valid global memory handle that nothing frees while this runs.
unsafe fn global_text(global: HGLOBAL) -> Option<String> {
    // SAFETY: the memory is read only while locked, and only within its `GlobalSize` (0 when
    // that fails: an empty slice, so empty text).
    unsafe {
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let units = GlobalSize(global) / 2;
        let text = utf16_until_nul(std::slice::from_raw_parts(ptr, units));
        let _ = GlobalUnlock(global);
        Some(text)
    }
}

/// The platform services behind `_native/*`. Called on the UI thread; anything that needs the
/// window goes through the event loop.
struct WindowsHost {
    proxy: Mutex<EventLoopProxy<UserEvent>>,
    runtime: String,
}

impl WindowsHost {
    fn post(&self, event: UserEvent) {
        let _ = self.proxy.lock().unwrap().send_event(event);
    }
}

impl Host for WindowsHost {
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
        format!("WebView2 {}", self.runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_menu_keeps_editing_and_spelling_only() {
        for keep in [
            "copy",
            "paste",
            "pasteAndMatchStyle",
            "spellCheck",
            "spellcheck",
            "spellCheckSuggestion1",
            "emoji",
        ] {
            assert!(keep_menu_item(keep, false), "{keep}");
        }
        for drop in [
            "back",
            "forward",
            "reload",
            "saveAs",
            "print",
            "share",
            "webCapture",
            "openLinkInNewWindow",
            "saveImageAs",
            "saveLinkAs",
            "createQrCode",
            "inspectElement",
        ] {
            assert!(!keep_menu_item(drop, false), "{drop}");
        }
        assert!(keep_menu_item("inspectElement", true));
    }

    #[test]
    fn unterminated_clipboard_text_is_read_only_within_its_allocation() {
        use windows::Win32::{
            Foundation::GlobalFree,
            System::Memory::{GMEM_MOVEABLE, GlobalAlloc},
        };
        // SAFETY: the global is ours, filled while locked and freed at the end.
        unsafe {
            let global = GlobalAlloc(GMEM_MOVEABLE, 64).unwrap();
            // Windows may round the size up: fill all of it, so no byte of it is a NUL.
            let size = GlobalSize(global);
            assert!(size >= 64);
            let ptr = GlobalLock(global) as *mut u8;
            assert!(!ptr.is_null());
            for i in 0..size {
                *ptr.add(i) = b'A';
            }
            let _ = GlobalUnlock(global);

            let text = global_text(global);
            let _ = GlobalFree(Some(global));
            // 'A','A' little-endian is U+4141, once per two bytes, and nothing beyond.
            assert_eq!(text.unwrap(), "\u{4141}".repeat(size / 2));
        }
    }
}
