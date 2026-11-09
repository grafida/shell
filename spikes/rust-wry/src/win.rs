// Windows: serve a *real* `boson://` scheme instead of wry's `http://boson.localhost` mapping.
//
// 1. Create the WebView2 environment ourselves, with a CoreWebView2CustomSchemeRegistration
//    for "boson" (TreatAsSecure, HasAuthorityComponent) — this must happen before the
//    environment exists — and hand it to wry via `with_environment`.
// 2. Register no wry custom protocol at all; instead add our own WebResourceRequested filter
//    for `boson://*` on the raw ICoreWebView2 wry exposes.
// 3. Requests are answered asynchronously: the event args + deferral are parked in a
//    thread-local map on the UI thread, the handler's responder posts `UserEvent::Respond`
//    through the event loop, and the UI thread completes the deferral.

use std::{borrow::Cow, cell::RefCell, collections::HashMap, fmt::Write as _, sync::{atomic::{AtomicU64, Ordering}, mpsc, Arc}};

use webview2_com::{
    CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2EnvironmentCompletedHandler, WebResourceRequestedEventHandler,
    Microsoft::Web::WebView2::Win32::*,
};
use windows::{
    core::{Interface, HSTRING, PCWSTR, PWSTR},
    Win32::{Foundation::E_POINTER, System::Com::IStream, UI::Shell::SHCreateMemStream},
};
use windows_core::BOOL;
use wry::http::{Request, Response};

use crate::{handle, Tokens, UserEvent};

thread_local! {
    static PENDING: RefCell<HashMap<u64, (ICoreWebView2WebResourceRequestedEventArgs, ICoreWebView2Deferral)>> = RefCell::default();
    static ENV: RefCell<Option<ICoreWebView2Environment>> = RefCell::default();
}
static NEXT: AtomicU64 = AtomicU64::new(1);

fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _)) };
    s
}

pub fn create_environment() -> windows::core::Result<ICoreWebView2Environment> {
    let options = CoreWebView2EnvironmentOptions::default();
    unsafe {
        let reg = CoreWebView2CustomSchemeRegistration::new("boson".to_string());
        reg.set_treat_as_secure(true);
        reg.set_has_authority_component(true);
        reg.set_allowed_origins(vec!["boson://app".to_string()]);
        options.set_scheme_registrations(vec![Some(reg.into())]);
    }
    let data_dir = std::env::var("LOCALAPPDATA").map(|d| format!("{d}\\GrafidaSpike\\WebView2")).unwrap_or_default();
    let (tx, rx) = mpsc::channel();
    unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(data_dir),
            &ICoreWebView2EnvironmentOptions::from(options),
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |code, env| {
                let r = (|| {
                    code?;
                    env.ok_or_else(|| windows::core::Error::from(E_POINTER))
                })();
                let _ = tx.send(r);
                Ok(())
            })),
        )?;
    }
    webview2_com::wait_with_pump(rx).map_err(|e| windows::core::Error::new(windows::core::HRESULT(-1), format!("{e:?}")))?
}

unsafe fn to_http(req: &ICoreWebView2WebResourceRequest) -> windows::core::Result<Request<Vec<u8>>> {
    unsafe {
        let mut uri = PWSTR::null();
        req.Uri(&mut uri)?;
        let mut method = PWSTR::null();
        req.Method(&mut method)?;
        let mut b = Request::builder().uri(take_pwstr(uri)).method(take_pwstr(method).as_str());
        let it = req.Headers()?.GetIterator()?;
        let mut has = BOOL::default();
        it.HasCurrentHeader(&mut has)?;
        while has.as_bool() {
            let (mut k, mut v) = (PWSTR::null(), PWSTR::null());
            it.GetCurrentHeader(&mut k, &mut v)?;
            b = b.header(take_pwstr(k), take_pwstr(v));
            it.MoveNext(&mut has)?;
        }
        let mut body = Vec::new();
        if let Ok(content) = req.Content() {
            let stream: IStream = content.cast()?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut n = 0u32;
                stream.Read(buf.as_mut_ptr() as _, buf.len() as u32, Some(&mut n)).ok()?;
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n as usize]);
            }
        }
        Ok(b.body(body).expect("request"))
    }
}

pub fn attach_scheme_handler(
    webview: &ICoreWebView2,
    env: ICoreWebView2Environment,
    tokens: Tokens,
    seq: Arc<AtomicU64>,
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) -> windows::core::Result<()> {
    ENV.with(|e| *e.borrow_mut() = Some(env));
    unsafe {
        let filter = HSTRING::from("boson://*");
        match webview.cast::<ICoreWebView2_22>() {
            Ok(w22) => w22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                &filter,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
            )?,
            Err(_) => webview.AddWebResourceRequestedFilter(&filter, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?,
        }
        let mut token = Default::default();
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let req = to_http(&args.Request()?)?;
                let deferral = args.GetDeferral()?;
                let id = NEXT.fetch_add(1, Ordering::SeqCst);
                PENDING.with(|p| p.borrow_mut().insert(id, (args, deferral)));
                let p2 = proxy.clone();
                handle(req, Box::new(move |resp| { let _ = p2.send_event(UserEvent::Respond(id, resp)); }), tokens.clone(), seq.clone(), proxy.clone());
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

pub fn complete(id: u64, resp: Response<Cow<'static, [u8]>>) {
    let Some((args, deferral)) = PENDING.with(|p| p.borrow_mut().remove(&id)) else { return };
    ENV.with(|e| {
        let env = e.borrow();
        let env = env.as_ref().expect("env");
        let mut headers = String::new();
        for (k, v) in resp.headers() {
            let _ = write!(headers, "{}: {}\r\n", k, v.to_str().unwrap_or(""));
        }
        let status = resp.status();
        unsafe {
            let body = resp.body();
            let stream = if body.is_empty() { None } else { SHCreateMemStream(Some(body)) };
            if let Ok(r) = env.CreateWebResourceResponse(
                stream.as_ref(),
                status.as_u16() as i32,
                &HSTRING::from(status.canonical_reason().unwrap_or("OK")),
                &HSTRING::from(headers),
            ) {
                let _ = args.SetResponse(&r);
            }
            let _ = deferral.Complete();
        }
    });
}
