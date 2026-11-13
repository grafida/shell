//! The Linux launcher (grafida/grafida#86): the executable a packaged app's menu entry starts.
//!
//! The shell links WebKitGTK 4.1, GTK 3 and glibc ≥ 2.35. When one of them is missing or too old,
//! the dynamic loader refuses the shell before `main()`, and stderr is all the user would get, which
//! a desktop launch does not show. So this launcher is a separate, **statically linked** binary that
//! links nothing but std. It asks the system loader whether the shell beside it would load (what
//! `ldd -r` does: `LD_TRACE_LOADED_OBJECTS` + `LD_BIND_NOW` + `LD_WARN`, which prints the libraries
//! and the unresolved names without running the program), and either execs the shell with the same
//! arguments or shows an alert that says what to install, and asks the user to read the app's
//! documentation for its minimum requirements.
//!
//! The check **fails open**: output it does not recognise lets the shell start, because refusing to
//! start on a system we merely could not read is worse than the crash we are trying to explain.
//!
//! Nothing here is app-specific: the alert's title is the manifest's `title`, and the libraries
//! and floors are the shell's own.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The shell's file name, beside the launcher.
pub const SHELL_BINARY: &str = "grafida-shell";

/// The oldest glibc the shell is built for (Ubuntu 22.04; ADR 0001).
pub const GLIBC_FLOOR: &str = "2.35";

/// The oldest WebKitGTK the shell runs on (`webkit_uri_scheme_request_get_http_body`; ADR 0001).
pub const WEBKITGTK_FLOOR: &str = "2.40";

/// What stops the shell from loading.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Problems {
    /// Shared libraries the loader could not find (sonames).
    pub missing: Vec<String>,
    /// Symbol versions not found, e.g. `GLIBC_2.35`.
    pub versions: Vec<String>,
    /// Symbols the shell itself needs that no loaded library provides.
    pub symbols: Vec<String>,
}

impl Problems {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.versions.is_empty() && self.symbols.is_empty()
    }
}

/// The outcome of asking the loader.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    /// The shell would load.
    Loads,
    /// It would not, for these reasons.
    Refused(Problems),
    /// We could not tell (unrecognised output, no loader); start the shell anyway.
    Unknown(String),
}

/// Runs the shell in the loader's trace mode and reads the verdict.
pub fn check(shell: &Path) -> Check {
    let output = Command::new(shell)
        .env("LD_TRACE_LOADED_OBJECTS", "1")
        .env("LD_BIND_NOW", "1")
        .env("LD_WARN", "1")
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(out) => classify(
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
            shell,
            out.status.success(),
        ),
        // ENOENT for a file that exists is the kernel failing to find the program interpreter
        // (no glibc loader at all: a musl system).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && shell.is_file() => {
            Check::Refused(Problems {
                missing: vec![format!("the GNU C Library's program loader ({e})")],
                ..Problems::default()
            })
        }
        Err(e) => Check::Unknown(format!("could not run {}: {e}", shell.display())),
    }
}

/// Parses the loader's trace output. Pure, so every format is testable without a broken system.
///
/// glibc prints, in trace mode (and exits 0 even then):
/// - stdout `\tlibfoo.so.1 => not found` for a library it cannot find;
/// - stderr `<obj>: <lib>: version `GLIBC_2.35' not found (required by <obj>)`;
/// - stderr `undefined symbol: name[, version V]\t(<obj>)`.
///
/// An undefined symbol counts only when the shell itself needs it. The shell is linked with
/// `-z now`, so only its own references are bound at start; a library's unresolved references are
/// bound lazily and may never be used.
pub fn classify(stdout: &str, stderr: &str, shell: &Path, success: bool) -> Check {
    let mut problems = Problems::default();

    for line in stdout.lines() {
        if let Some((name, rest)) = line.trim().split_once("=>")
            && rest.trim() == "not found"
        {
            push_unique(&mut problems.missing, name.trim());
        }
    }

    let shell_ref = format!("({})", shell.display());
    for line in stderr.lines() {
        if let Some(rest) = line.split_once("version `").map(|(_, r)| r)
            && let Some((version, tail)) = rest.split_once('\'')
            && tail.trim_start().starts_with("not found")
        {
            push_unique(&mut problems.versions, version);
        } else if let Some(rest) = line.strip_prefix("undefined symbol: ")
            && line.trim_end().ends_with(&shell_ref)
        {
            let name = rest.split(['\t', ',']).next().unwrap_or(rest).trim();
            push_unique(&mut problems.symbols, name);
        }
    }

    if !problems.is_empty() {
        Check::Refused(problems)
    } else if success && stdout.contains("=>") {
        Check::Loads
    } else {
        Check::Unknown(format!(
            "unrecognised loader output (success: {success}):\n{stdout}{stderr}"
        ))
    }
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !item.is_empty() && !list.iter().any(|i| i == item) {
        list.push(item.to_owned());
    }
}

/// The distribution family, from `/etc/os-release`'s `ID` and `ID_LIKE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Debian,
    Fedora,
    Arch,
    Suse,
    Other,
}

pub fn family(os_release: &str) -> Family {
    let mut ids = Vec::new();
    for line in os_release.lines() {
        if let Some(value) = line
            .strip_prefix("ID=")
            .or_else(|| line.strip_prefix("ID_LIKE="))
        {
            ids.extend(
                value
                    .trim_matches(['"', '\''])
                    .split_whitespace()
                    .map(str::to_lowercase),
            );
        }
    }
    let has = |names: &[&str]| ids.iter().any(|id| names.contains(&id.as_str()));
    if has(&["debian", "ubuntu"]) {
        Family::Debian
    } else if has(&["fedora", "rhel", "centos"]) {
        Family::Fedora
    } else if has(&["arch"]) {
        Family::Arch
    } else if has(&["suse", "opensuse"]) {
        Family::Suse
    } else {
        Family::Other
    }
}

/// The command that installs the shell's runtime dependencies on this family, if we know it.
pub fn install_command(family: Family) -> Option<&'static str> {
    match family {
        Family::Debian => Some("sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0"),
        Family::Fedora => Some("sudo dnf install webkit2gtk4.1 gtk3"),
        Family::Arch => Some("sudo pacman -S webkit2gtk-4.1 gtk3"),
        Family::Suse => Some("sudo zypper install libwebkit2gtk-4_1-0 libgtk-3-0"),
        Family::Other => None,
    }
}

/// The alert: a one-line message and the detail under it. Hard-coded English, like every message
/// shown before the app exists (there is nothing to translate with yet).
pub fn explain(title: &str, problems: &Problems, family: Family) -> (String, String) {
    let mut detail = Vec::new();
    let old_glibc = problems.versions.iter().any(|v| v.starts_with("GLIBC_"));

    let message = if !problems.missing.is_empty() {
        detail.push(format!(
            "{title} needs WebKitGTK 4.1 ({WEBKITGTK_FLOOR} or later) and GTK 3. These libraries were not found:\n  {}",
            problems.missing.join("\n  ")
        ));
        match install_command(family) {
            Some(command) => detail.push(format!("Install them with:\n  {command}")),
            None => detail.push(
                "Install WebKitGTK 4.1 and GTK 3 from your distribution's package manager.".into(),
            ),
        }
        format!("{title} cannot start: system libraries are missing.")
    } else if old_glibc {
        detail.push(format!(
            "{title} needs the GNU C Library {GLIBC_FLOOR} or later (for example Ubuntu 22.04, \
             Debian 12 or Fedora 36, or newer). This system's is older."
        ));
        format!("{title} cannot start: this version of Linux is too old.")
    } else {
        detail.push(format!(
            "{title} needs WebKitGTK {WEBKITGTK_FLOOR} or later, and newer system libraries than \
             this system has. Update your system's packages and try again."
        ));
        format!("{title} cannot start: system libraries are too old.")
    };

    // With a library missing, every symbol it would have provided is "undefined" too: noise.
    let mut missing_names: Vec<&str> = problems.versions.iter().map(String::as_str).collect();
    missing_names.extend(problems.symbols.iter().map(String::as_str));
    if problems.missing.is_empty() && !missing_names.is_empty() {
        detail.push(format!("Not found: {}", missing_names.join(", ")));
    }

    // The launcher knows the shell's floors, not the app's: an app may need more (Grafida needs
    // `secret-tool`), so the last word is always its own documentation.
    detail.push(format!(
        "Please read {title}'s documentation for its minimum system requirements."
    ));

    (message, detail.join("\n\n"))
}

/// Reads the manifest's `title` beside the launcher; `None` if there is none we can read.
pub fn manifest_title(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("title")?.as_str().map(str::to_owned)
}

/// The name every alert uses when the real title is missing or unusable.
pub const FALLBACK_TITLE: &str = "The application";

/// The title as the alert tools may see it. It comes from `shell.json` or the launcher's file
/// name, and every message starts with it, so a title like `--help` would be read as an option by
/// the tools that take their text positionally (`notify-send`, `xmessage`; X Toolkit options may
/// also start with `+`). Leading `-`, `+` and whitespace are dropped; nothing left means
/// [`FALLBACK_TITLE`].
pub fn safe_title(title: &str) -> String {
    let kept = title.trim_start_matches(|c: char| c == '-' || c == '+' || c.is_whitespace());
    if kept.is_empty() {
        FALLBACK_TITLE.to_owned()
    } else {
        kept.to_owned()
    }
}

/// `&`, `<` and `>` as entities, for a tool that renders markup (a notification body).
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// kdialog's text with its first line (up to the first `\n`) unable to look like rich text.
///
/// kdialog shows the text in a `QLabel` with `Qt::AutoText` and external links allowed
/// (`KMessageBox::createKMessageBox`, kwidgetsaddons, called with `AllowLink` from kdialog's
/// `kdialog.cpp`). `Qt::mightBeRichText` (qtbase, `qtextdocument.cpp`) looks only at the text before
/// the first `<` or `\n`, says "rich" on `&lt;` on the way, and otherwise if that `<` opens a known
/// tag. So only the first line, which starts with the manifest's title, can switch rich text (and
/// links) on; later lines are always plain and are left alone. Escaping would show `&lt;` literally
/// in plain mode, so instead `<` becomes `U+2039` and `&lt;` is broken with a word joiner.
fn neutralise_first_line(text: &str) -> String {
    let (first, rest) = match text.find('\n') {
        Some(i) => text.split_at(i),
        None => (text, ""),
    };
    let first = first
        .replace('<', "\u{2039}")
        .replace("&lt;", "&\u{2060}lt;");
    format!("{first}{rest}")
}

/// The alert tools tried in order, and the arguments each takes. Each is spawned and must have run
/// (exit 0 or 1, i.e. dismissed); a tool that is absent or cannot start itself (127, a signal)
/// falls through to the next. stderr always gets the text first.
pub fn alert_commands(
    title: &str,
    message: &str,
    detail: &str,
) -> Vec<(&'static str, Vec<OsString>)> {
    let text = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}\n\n{detail}")
    };
    let os = |s: &str| OsString::from(s);
    vec![
        (
            "zenity",
            vec![
                os("--error"),
                os("--no-markup"),
                os("--width=520"),
                os(&format!("--title={title}")),
                os(&format!("--text={text}")),
            ],
        ),
        (
            "kdialog",
            vec![
                os("--title"),
                os(title),
                os("--error"),
                os(&neutralise_first_line(&text)),
            ],
        ),
        (
            "notify-send",
            vec![
                os("--urgency=critical"),
                os(&format!("--app-name={title}")),
                os("--"),
                os(message),
                // The summary is plain text (Desktop Notifications spec); the body may be markup.
                os(&escape_markup(detail)),
            ],
        ),
        ("xmessage", vec![os("-center"), os(&text)]),
    ]
}

pub fn alert(title: &str, message: &str, detail: &str) {
    eprintln!("{title}: {message}\n{detail}");
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    for (program, args) in alert_commands(title, message, detail) {
        let shown = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| matches!(s.code(), Some(0 | 1)));
        if shown {
            return;
        }
    }
}

/// The shell beside the launcher, or `None` unless `launcher` is an absolute path with a file name.
/// `current_exe()` resolves `/proc/self/exe`, so a symlink to the launcher (say
/// `~/.local/bin/grafida`) still finds the real directory. A bare or relative path would make
/// `Command` search `PATH` for the shell, both for the probe and for the exec: never do that.
pub fn shell_beside(launcher: &Path) -> Option<PathBuf> {
    if !launcher.is_absolute() {
        return None;
    }
    Some(launcher.parent()?.join(SHELL_BINARY))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHELL: &str = "/opt/app/grafida-shell";

    fn classify_err(stdout: &str, stderr: &str) -> Check {
        classify(stdout, stderr, Path::new(SHELL), true)
    }

    // Captured from glibc 2.42's loader in trace mode (scratch C programs, grafida/grafida#86).
    const HEALTHY: &str = "\tlinux-vdso.so.1 (0x00007f455bf4e000)\n\tlibdemo.so.1 => /x/libdemo.so.1 (0x00007f455befc000)\n\tlibc.so.6 => /usr/lib/libc.so.6 (0x00007f455bc00000)\n\t/lib64/ld-linux-x86-64.so.2 (0x00007f455bf50000)\n";

    #[test]
    fn a_healthy_trace_loads() {
        assert_eq!(classify_err(HEALTHY, ""), Check::Loads);
    }

    #[test]
    fn a_missing_library_is_named() {
        let out = "\tlinux-vdso.so.1 (0x1)\n\tlibwebkit2gtk-4.1.so.0 => not found\n\tlibgtk-3.so.0 => not found\n\tlibc.so.6 => /usr/lib/libc.so.6 (0x2)\n";
        let err = format!("undefined symbol: webkit_web_view_new\t({SHELL})\n");
        let Check::Refused(p) = classify_err(out, &err) else {
            panic!()
        };
        assert_eq!(p.missing, ["libwebkit2gtk-4.1.so.0", "libgtk-3.so.0"]);
    }

    #[test]
    fn an_old_glibc_is_a_missing_version() {
        let err = format!(
            "{SHELL}: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.35' not found (required by {SHELL})\n\
             {SHELL}: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.34' not found (required by {SHELL})\n"
        );
        let Check::Refused(p) = classify_err(HEALTHY, &err) else {
            panic!()
        };
        assert_eq!(p.versions, ["GLIBC_2.35", "GLIBC_2.34"]);
        let (message, _) = explain("Grafida", &p, Family::Debian);
        assert!(message.contains("too old"), "{message}");
    }

    #[test]
    fn only_the_shells_own_undefined_symbols_count() {
        let err = format!(
            "undefined symbol: g_lazy_never_used\t(/usr/lib/libsomething.so.1)\n\
             undefined symbol: webkit_uri_scheme_request_get_http_body\t({SHELL})\n"
        );
        let Check::Refused(p) = classify_err(HEALTHY, &err) else {
            panic!()
        };
        assert_eq!(p.symbols, ["webkit_uri_scheme_request_get_http_body"]);

        let lib_only = "undefined symbol: g_lazy_never_used\t(/usr/lib/libsomething.so.1)\n";
        assert_eq!(classify_err(HEALTHY, lib_only), Check::Loads);
    }

    #[test]
    fn a_versioned_undefined_symbol_keeps_only_its_name() {
        let err = format!("undefined symbol: gone, version DEMO_2\t({SHELL})\n");
        let Check::Refused(p) = classify_err(HEALTHY, &err) else {
            panic!()
        };
        assert_eq!(p.symbols, ["gone"]);
    }

    #[test]
    fn unrecognised_output_fails_open() {
        assert!(matches!(classify_err("", ""), Check::Unknown(_)));
        assert!(matches!(
            classify("not a trace", "", Path::new(SHELL), false),
            Check::Unknown(_)
        ));
    }

    #[test]
    fn distribution_families() {
        assert_eq!(
            family("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n"),
            Family::Debian
        );
        assert_eq!(
            family("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            Family::Debian
        );
        assert_eq!(family("ID=fedora\n"), Family::Fedora);
        assert_eq!(
            family("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
            Family::Fedora
        );
        assert_eq!(family("ID=arch\n"), Family::Arch);
        assert_eq!(family("ID=endeavouros\nID_LIKE=arch\n"), Family::Arch);
        assert_eq!(
            family("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            Family::Suse
        );
        assert_eq!(family("ID=nixos\n"), Family::Other);
        assert_eq!(family(""), Family::Other);
    }

    #[test]
    fn the_alert_names_the_packages_to_install() {
        let p = Problems {
            missing: vec!["libwebkit2gtk-4.1.so.0".into()],
            ..Problems::default()
        };
        let (message, detail) = explain("Grafida", &p, Family::Fedora);
        assert!(message.starts_with("Grafida cannot start"), "{message}");
        assert!(detail.contains("libwebkit2gtk-4.1.so.0"), "{detail}");
        assert!(
            detail.contains("sudo dnf install webkit2gtk4.1 gtk3"),
            "{detail}"
        );
        let (_, detail) = explain("Grafida", &p, Family::Other);
        assert!(!detail.contains("sudo"), "{detail}");
    }

    #[test]
    fn every_alert_ends_by_pointing_at_the_apps_documentation() {
        let ask = "Please read Grafida's documentation for its minimum system requirements.";
        let cases = [
            Problems {
                missing: vec!["libgtk-3.so.0".into()],
                ..Problems::default()
            },
            Problems {
                versions: vec!["GLIBC_2.35".into()],
                ..Problems::default()
            },
            Problems {
                symbols: vec!["webkit_uri_scheme_request_get_http_body".into()],
                ..Problems::default()
            },
        ];
        for p in cases {
            for family in [Family::Debian, Family::Other] {
                let (_, detail) = explain("Grafida", &p, family);
                assert!(detail.ends_with(ask), "{detail}");
            }
        }
    }

    #[test]
    fn zenity_is_told_the_text_is_not_markup() {
        let commands = alert_commands("A & B", "x < y", "");
        assert_eq!(commands[0].0, "zenity");
        assert!(commands[0].1.iter().any(|a| a == "--no-markup"));
    }

    #[test]
    fn the_shell_is_never_a_bare_or_relative_path() {
        for launcher in [
            "grafida-launcher",
            "./grafida-launcher",
            "bin/grafida-launcher",
            "",
        ] {
            assert_eq!(shell_beside(Path::new(launcher)), None, "{launcher:?}");
        }
        assert_eq!(shell_beside(Path::new("/")), None);
        assert_eq!(
            shell_beside(Path::new("/opt/app/grafida-launcher")),
            Some(PathBuf::from("/opt/app/grafida-shell"))
        );
    }

    fn last_text(args: &[OsString], n: usize) -> Vec<String> {
        let n = args.len() - n;
        args[n..]
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_title_that_looks_like_an_option_never_starts_a_text_argument() {
        for raw in ["-x", "--help", "+rv", "  - --version", "-"] {
            let title = safe_title(raw);
            let (message, detail) = explain(&title, &Problems::default(), Family::Other);
            let commands = alert_commands(&title, &message, &detail);
            let find = |name: &str| commands.iter().find(|c| c.0 == name).unwrap().1.clone();

            // notify-send: summary and body; xmessage: the text; kdialog: --error's value.
            let ns = find("notify-send");
            let mut texts = last_text(&ns, 2);
            texts.extend(last_text(&find("xmessage"), 1));
            texts.extend(last_text(&find("kdialog"), 1));
            for t in texts {
                assert!(!t.starts_with(['-', '+']), "{raw:?}: {t:?}");
            }
            // ...and notify-send is told the positionals begin.
            assert_eq!(ns[ns.len() - 3], "--", "{ns:?}");
        }
    }

    #[test]
    fn a_title_with_nothing_left_is_the_fallback() {
        for raw in ["", "-", "--", " - ", "+", "\t-+ "] {
            assert_eq!(safe_title(raw), "The application", "{raw:?}");
        }
        assert_eq!(safe_title("--Grafida"), "Grafida");
        assert_eq!(safe_title("Grafida"), "Grafida");
        assert_eq!(safe_title("My-App"), "My-App");
    }

    #[test]
    fn the_notification_body_is_escaped_but_not_its_summary_or_other_tools() {
        let title = "A <b>&</b>";
        let message = "<a href=\"https://example.com\">click</a> & go";
        let detail = "Not found:\n  <i>libx.so</i> & more";
        let commands = alert_commands(title, message, detail);
        let find = |name: &str| commands.iter().find(|c| c.0 == name).unwrap().1.clone();

        let ns = find("notify-send");
        let n = ns.len();
        // The summary is plain text per the Desktop Notifications spec: untouched.
        assert_eq!(ns[n - 2], message);
        // The body may carry markup: `&`, `<` and `>` are escaped, line breaks stay.
        assert_eq!(
            ns[n - 1],
            "Not found:\n  &lt;i&gt;libx.so&lt;/i&gt; &amp; more"
        );

        // zenity is told `--no-markup`, xmessage shows plain text: both get the raw text.
        let text = format!("{message}\n\n{detail}");
        assert!(find("zenity").contains(&OsString::from(format!("--text={text}"))));
        assert_eq!(find("xmessage").last().unwrap(), &OsString::from(&text));
    }

    /// A port of the part of Qt's `Qt::mightBeRichText` (qtbase, `qtextdocument.cpp`) that matters
    /// here: only the text before the first `<` or `\n` is examined, `&lt;` on the way says rich
    /// text, and a `<` that opens a known tag says rich text. Anything with a `<` or `&lt;` in the
    /// first line is treated as "might be".
    fn might_be_rich_text(text: &str) -> bool {
        let first = text.trim_start().split('\n').next().unwrap_or("");
        first.contains('<') || first.contains("&lt;")
    }

    #[test]
    fn kdialog_cannot_be_made_to_render_rich_text_by_the_title() {
        let title = "<a href=\"https://example.com\">x</a> &lt;b&gt;";
        let message = format!("{title} cannot start.");
        let detail = "Not found:\n  <i>libx.so</i> & more";
        let commands = alert_commands(title, &message, detail);
        let kd = &commands.iter().find(|c| c.0 == "kdialog").unwrap().1;
        let text = kd.last().unwrap().to_string_lossy().into_owned();

        assert!(!might_be_rich_text(&text), "{text:?}");
        let (first, rest) = text.split_once('\n').unwrap();
        assert!(!first.contains('<') && !first.contains("&lt;"), "{first:?}");
        // The detail is plain text for Qt (it is past the first line): shown raw, never escaped.
        assert!(
            rest.ends_with("Not found:\n  <i>libx.so</i> & more"),
            "{rest:?}"
        );

        // A normal title is passed through byte for byte.
        let plain = alert_commands("Grafida", "Grafida cannot start.", "Detail: a & b");
        let kd = &plain.iter().find(|c| c.0 == "kdialog").unwrap().1;
        assert_eq!(
            kd.last().unwrap(),
            &OsString::from("Grafida cannot start.\n\nDetail: a & b")
        );
    }
}
