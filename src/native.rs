//! The native endpoints, `<scheme>://app/_native/*`, answered by the shell itself (grafida/grafida#82,
//! "Native endpoints"). JSON in, JSON out, never cached.
//!
//! A dialog answers with **tokens, never paths** ([`crate::tokens`]). Only one dialog is open at a
//! time; a second request while one is open gets `409 dialog_busy`.

use std::{path::Path, sync::Arc};

use serde_json::{Value, json};

use crate::{
    bridge::{Bridge, FileFilter, Picked},
    http::{Request, Response},
    manifest::Theme,
    protocol::PROTOCOL_VERSION,
    supervisor::Responder,
    tokens::{Kind, TokenStore},
};

pub fn handle(bridge: &Bridge, request: Request, respond: Responder) {
    let endpoint = request.path().trim_start_matches("/_native/").to_string();
    let method = request.method.to_ascii_uppercase();

    let expected = match endpoint.as_str() {
        "info" => "GET",
        "dialog/open-file"
        | "dialog/select-folder"
        | "dialog/save-file"
        | "open-url"
        | "reveal"
        | "window/theme"
        | "window/title"
        | "quit" => "POST",
        _ => {
            respond(Response::error(404, "not_found"));
            return;
        }
    };
    if method != expected {
        let mut r = Response::error(405, "method_not_allowed");
        crate::http::append(&mut r.headers, "allow", expected);
        respond(r);
        return;
    }

    let body: Value = if expected == "POST" {
        match serde_json::from_slice(&request.body) {
            Ok(v @ Value::Object(_)) => v,
            _ if request.body.is_empty() => json!({}),
            _ => {
                respond(Response::error(400, "bad_request"));
                return;
            }
        }
    } else {
        Value::Null
    };

    let host = bridge.host.clone();
    let ok = || Response::json(200, &json!({"ok": true}));

    match endpoint.as_str() {
        "info" => respond(Response::json(
            200,
            &json!({
                "shell": env!("CARGO_PKG_VERSION"),
                "protocol": PROTOCOL_VERSION,
                "platform": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "webview": host.webview_version(),
            }),
        )),
        "dialog/open-file" => {
            let Some(filters) = filters(&body) else {
                return respond(Response::error(400, "bad_request"));
            };
            let multiple = body
                .get("multiple")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            with_dialog(bridge, respond, Kind::File, move |done| {
                host.open_file(filters, multiple, done)
            });
        }
        "dialog/select-folder" => {
            with_dialog(bridge, respond, Kind::Folder, move |done| {
                host.select_folder(done)
            });
        }
        "dialog/save-file" => {
            let Some(filters) = filters(&body) else {
                return respond(Response::error(400, "bad_request"));
            };
            let name = body
                .get("suggestedName")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            with_dialog(bridge, respond, Kind::SaveTarget, move |done| {
                host.save_file(name, filters, done)
            });
        }
        "open-url" => {
            let url = body.get("url").and_then(Value::as_str).unwrap_or("");
            let lower = url.to_ascii_lowercase();
            // The prefix check keeps out what the parser would complete (`https:x`); the host is
            // only ever handed the re-serialised form (security.md L3).
            let url = if lower.starts_with("http://") || lower.starts_with("https://") {
                crate::navigation::external_url(url)
            } else {
                None
            };
            let Some(url) = url else {
                return respond(Response::error(400, "unsupported_url"));
            };
            match host.open_url(&url) {
                Ok(()) => respond(ok()),
                Err(e) => respond(Response::json(
                    500,
                    &json!({"error": "open_failed", "detail": e}),
                )),
            }
        }
        "reveal" => {
            let token = body.get("token").and_then(Value::as_str).unwrap_or("");
            let path = match bridge.tokens.lock().unwrap().resolve(token) {
                Ok(picked) => picked.path.clone(),
                Err(e) => return respond(Response::error(400, e.code())),
            };
            match host.reveal(&path) {
                Ok(()) => respond(ok()),
                Err(e) => respond(Response::json(
                    500,
                    &json!({"error": "reveal_failed", "detail": e}),
                )),
            }
        }
        "window/theme" => match body
            .get("theme")
            .and_then(Value::as_str)
            .and_then(Theme::parse)
        {
            Some(theme) => {
                host.set_window_theme(theme);
                respond(ok());
            }
            None => respond(Response::error(400, "bad_request")),
        },
        "window/title" => match body.get("title").and_then(Value::as_str) {
            Some(title) => {
                host.set_window_title(title);
                respond(ok());
            }
            None => respond(Response::error(400, "bad_request")),
        },
        "quit" => {
            respond(ok());
            host.quit();
        }
        _ => unreachable!("endpoints are matched above"),
    }
}

/// `filters: [{name, extensions[]}]`; absent means no filter. `None` when malformed.
fn filters(body: &Value) -> Option<Vec<FileFilter>> {
    let Some(list) = body.get("filters") else {
        return Some(Vec::new());
    };
    list.as_array()?
        .iter()
        .map(|f| {
            let name = f.get("name")?.as_str()?.to_string();
            let extensions = f
                .get("extensions")?
                .as_array()?
                .iter()
                .map(|e| e.as_str().map(|e| e.trim_start_matches('.').to_string()))
                .collect::<Option<Vec<_>>>()?;
            Some(FileFilter { name, extensions })
        })
        .collect()
}

fn with_dialog(
    bridge: &Bridge,
    respond: Responder,
    kind: Kind,
    open: impl FnOnce(Box<dyn FnOnce(Picked) + Send>),
) {
    {
        let mut busy = bridge.dialog_open.lock().unwrap();
        if *busy {
            return respond(Response::error(409, "dialog_busy"));
        }
        *busy = true;
    }
    let tokens = bridge.tokens.clone();
    let busy = Arc::clone(&bridge.dialog_open);

    open(Box::new(move |picked| {
        *busy.lock().unwrap() = false;
        let Some(paths) = picked.filter(|p| !p.is_empty()) else {
            return respond(Response::json(200, &json!({"cancelled": true})));
        };
        let mut tokens = tokens.lock().unwrap();
        let mut entries: Vec<Value> = paths
            .into_iter()
            .map(|path| file_entry(&mut tokens, path, kind))
            .collect();
        let body = match kind {
            Kind::File => json!({"files": entries}),
            Kind::Folder => json!({"folder": entries.swap_remove(0)}),
            Kind::SaveTarget => json!({"file": entries.swap_remove(0)}),
        };
        respond(Response::json(200, &body));
    }));
}

/// One picked file or folder as the page sees it: a fresh token and the name, never the path.
pub(crate) fn file_entry(tokens: &mut TokenStore, path: std::path::PathBuf, kind: Kind) -> Value {
    let name = file_name(&path);
    let size = std::fs::metadata(&path).map(|m| m.len()).ok();
    let token = tokens.issue(path, kind);
    match kind {
        Kind::File => json!({"token": token, "name": name, "size": size.unwrap_or(0)}),
        Kind::Folder | Kind::SaveTarget => json!({"token": token, "name": name}),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::bridge::{
        FILE_PATH_HEADER,
        tests::{bridge, call, json},
    };

    #[test]
    fn info_describes_the_shell() {
        let (b, _, _) = bridge();
        let r = call(&b, "GET", "/_native/info", &[], "");
        let v = json(&r);
        assert_eq!(v["protocol"], 1);
        assert_eq!(v["webview"], "TestKit 1.0");
        assert_eq!(v["platform"], std::env::consts::OS);
        assert_eq!(r.headers["cache-control"], vec!["no-store"]);
    }

    #[test]
    fn methods_and_bodies_are_checked() {
        let (b, _, _) = bridge();
        let r = call(&b, "GET", "/_native/quit", &[], "");
        assert_eq!(r.status, 405);
        assert_eq!(r.headers["allow"], vec!["POST"]);
        assert_eq!(
            call(&b, "POST", "/_native/window/title", &[], "[1]").status,
            400
        );
        assert_eq!(
            call(&b, "POST", "/_native/window/title", &[], "{}").status,
            400
        );
    }

    #[test]
    fn the_open_file_token_round_trip() {
        let (b, host, echo) = bridge();
        let dir = std::env::temp_dir();
        let file = dir.join(format!("shell-native-test-{}.png", std::process::id()));
        std::fs::write(&file, b"PNGDATA").unwrap();
        *host.pick.lock().unwrap() = Some(vec![file.clone()]);

        let r = call(
            &b,
            "POST",
            "/_native/dialog/open-file",
            &[],
            r#"{"filters":[{"name":"Images","extensions":["png",".jpg"]}],"multiple":false}"#,
        );
        let v = json(&r);
        let token = v["files"][0]["token"].as_str().unwrap().to_string();
        assert_eq!(v["files"][0]["size"], 7);
        assert!(v["files"][0]["name"].as_str().unwrap().ends_with(".png"));
        assert!(
            !r.body.windows(5).any(|w| w == b"/tmp/"),
            "no path in the answer"
        );
        assert!(host.calls.lock().unwrap()[0].contains(r#"extensions: ["png", "jpg"]"#));

        call(
            &b,
            "POST",
            "/api/dialog/open-file",
            &[("x-shell-file", &token)],
            "",
        );
        assert_eq!(
            echo.seen.lock().unwrap()[0].headers[FILE_PATH_HEADER],
            vec![file.to_str().unwrap()]
        );
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn cancelling_a_dialog() {
        let (b, host, _) = bridge();
        *host.pick.lock().unwrap() = None;
        for endpoint in [
            "dialog/open-file",
            "dialog/select-folder",
            "dialog/save-file",
        ] {
            let r = call(&b, "POST", &format!("/_native/{endpoint}"), &[], "{}");
            assert_eq!(
                json(&r),
                serde_json::json!({"cancelled": true}),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn folder_and_save_targets() {
        let (b, host, _) = bridge();
        *host.pick.lock().unwrap() = Some(vec!["/home/u/Exports".into()]);
        let v = json(&call(&b, "POST", "/_native/dialog/select-folder", &[], ""));
        assert_eq!(v["folder"]["name"], "Exports");
        assert!(v["folder"]["token"].is_string());

        *host.pick.lock().unwrap() = Some(vec!["/home/u/log.json".into()]);
        let v = json(&call(
            &b,
            "POST",
            "/_native/dialog/save-file",
            &[],
            r#"{"suggestedName":"log.json"}"#,
        ));
        assert_eq!(v["file"]["name"], "log.json");
        assert!(
            host.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("save_file log.json"))
        );
    }

    #[test]
    fn only_web_urls_are_opened() {
        let (b, host, _) = bridge();
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:a@b",
            "",
            "https://a b",
        ] {
            let body = serde_json::json!({ "url": url }).to_string();
            assert_eq!(
                call(&b, "POST", "/_native/open-url", &[], &body).status,
                400,
                "{url}"
            );
        }
        let r = call(
            &b,
            "POST",
            "/_native/open-url",
            &[],
            r#"{"url":"https://grafida.app/"}"#,
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            host.calls.lock().unwrap().as_slice(),
            ["open_url https://grafida.app/"]
        );
    }

    #[test]
    fn the_host_is_handed_a_normalised_url() {
        // `ShellExecuteW` substitutes the URL into the handler's `"%1"` (security.md L3).
        let (b, host, _) = bridge();
        for (url, handed) in [
            (r#"https://x/a"b?q="c#"f"#, r"https://x/a%22b?q=%22c#%22f"),
            (r"https://x/a\b?q=c\", r"https://x/a/b?q=c%5C"),
            (r"https://x/?q=c\#f\", r"https://x/?q=c%5C#f%5C"),
            ("https://x/%41?q=%22&r=100%", "https://x/%41?q=%22&r=100%"),
            ("https://grafida.app", "https://grafida.app/"),
            ("HTTPS://Grafida.App/Docs", "https://grafida.app/Docs"),
        ] {
            host.calls.lock().unwrap().clear();
            let body = serde_json::json!({ "url": url }).to_string();
            assert_eq!(
                call(&b, "POST", "/_native/open-url", &[], &body).status,
                200,
                "{url}"
            );
            assert_eq!(
                host.calls.lock().unwrap().as_slice(),
                [format!("open_url {handed}")],
                "{url}"
            );
        }
        for url in [
            "https://user:pw@x/",
            "https://user@x/",
            "http://",
            "https://",
            "https:x",
            "https://a/\nb",
            "https://a/\tb",
            "https://a/\u{0}b",
            "https://[::1/",
        ] {
            host.calls.lock().unwrap().clear();
            let body = serde_json::json!({ "url": url }).to_string();
            assert_eq!(
                call(&b, "POST", "/_native/open-url", &[], &body).status,
                400,
                "{url:?}"
            );
            assert!(host.calls.lock().unwrap().is_empty(), "{url:?}");
        }
    }

    #[test]
    fn reveal_needs_a_token() {
        let (b, host, _) = bridge();
        assert_eq!(
            call(&b, "POST", "/_native/reveal", &[], r#"{"token":"/etc"}"#).status,
            400
        );
        let token = b
            .tokens
            .lock()
            .unwrap()
            .issue("/home/u/x".into(), crate::tokens::Kind::File);
        let body = serde_json::json!({ "token": token }).to_string();
        assert_eq!(call(&b, "POST", "/_native/reveal", &[], &body).status, 200);
        assert_eq!(host.calls.lock().unwrap().as_slice(), ["reveal /home/u/x"]);
    }

    #[test]
    fn window_theme_title_and_quit() {
        let (b, host, _) = bridge();
        assert_eq!(
            call(
                &b,
                "POST",
                "/_native/window/theme",
                &[],
                r#"{"theme":"dark"}"#
            )
            .status,
            200
        );
        assert_eq!(
            call(
                &b,
                "POST",
                "/_native/window/theme",
                &[],
                r#"{"theme":"pink"}"#
            )
            .status,
            400
        );
        assert_eq!(
            call(
                &b,
                "POST",
                "/_native/window/title",
                &[],
                r#"{"title":"Draft — Grafida"}"#
            )
            .status,
            200
        );
        assert_eq!(call(&b, "POST", "/_native/quit", &[], "").status, 200);
        assert_eq!(
            host.calls.lock().unwrap().as_slice(),
            ["theme Dark", "title Draft — Grafida", "quit"]
        );
    }

    #[test]
    fn one_dialog_at_a_time() {
        use crate::bridge::{Bridge, DialogDone, Host};
        use std::sync::{Arc, Mutex};

        /// Holds the dialog open until the test lets go.
        #[derive(Default)]
        struct Slow(Mutex<Option<DialogDone>>);
        impl Host for Slow {
            fn open_file(&self, _: Vec<crate::bridge::FileFilter>, _: bool, done: DialogDone) {
                *self.0.lock().unwrap() = Some(done);
            }
            fn select_folder(&self, done: DialogDone) {
                *self.0.lock().unwrap() = Some(done);
            }
            fn save_file(&self, _: String, _: Vec<crate::bridge::FileFilter>, done: DialogDone) {
                *self.0.lock().unwrap() = Some(done);
            }
            fn open_url(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn reveal(&self, _: &std::path::Path) -> Result<(), String> {
                Ok(())
            }
            fn set_window_theme(&self, _: crate::manifest::Theme) {}
            fn set_window_title(&self, _: &str) {}
            fn quit(&self) {}
            fn read_clipboard(&self, done: crate::bridge::ClipboardDone) {
                done(None);
            }
            fn webview_version(&self) -> String {
                String::new()
            }
        }

        let host = Arc::new(Slow::default());
        let b = Bridge::new(
            "shell://app".into(),
            1 << 20,
            vec![],
            host.clone(),
            Arc::new(crate::bridge::tests::Echo::default()),
        );
        let (tx, first) = std::sync::mpsc::channel();
        b.handle(
            crate::http::Request {
                method: "POST".into(),
                url: "shell://app/_native/dialog/open-file".into(),
                headers: Default::default(),
                body: vec![],
            },
            Box::new(move |r| tx.send(r).unwrap()),
        );
        assert_eq!(
            call(&b, "POST", "/_native/dialog/select-folder", &[], "").status,
            409
        );
        host.0.lock().unwrap().take().unwrap()(None);
        assert_eq!(json(&first.recv().unwrap())["cancelled"], true);
        // Free again: the next dialog opens (the slow host holds on to it).
        b.handle(
            crate::http::Request {
                method: "POST".into(),
                url: "shell://app/_native/dialog/select-folder".into(),
                headers: Default::default(),
                body: vec![],
            },
            Box::new(|_| {}),
        );
        assert!(host.0.lock().unwrap().is_some());
    }
}
