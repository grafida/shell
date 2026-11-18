//! The check under a non-English locale: glibc translates the loader's messages, so the probe must
//! pin the `C` locale or `classify` would not recognise what it reads and would fail open.
//!
//! Needs a C compiler (`cc`); skipped with a note when there is none. It lives in its own test
//! binary because it sets environment variables for the whole process.
//!
//! A regression pin, not a proven red/green: on glibc 2.35 (Ubuntu 22.04, German catalogue
//! installed) the loader's start-up messages came out in English even without `LC_ALL=C`. The pin is
//! for a glibc, or a message, that does translate them. Without a German locale installed it proves
//! less, but still passes.

#![cfg(target_os = "linux")]

use std::{path::Path, process::Command};

use grafida_launcher::{Check, check};

fn cc(dir: &Path, args: &[&str]) -> bool {
    Command::new("cc")
        .current_dir(dir)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn the_probe_reads_english_whatever_the_users_locale() {
    let dir = std::env::temp_dir().join(format!("grafida-launcher-locale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.c"), "int present(void){return 1;}\n").unwrap();
    std::fs::write(
        dir.join("main.c"),
        "int present(void);\nint main(void){return present();}\n",
    )
    .unwrap();
    if !cc(
        &dir,
        &[
            "-shared",
            "-fPIC",
            "-o",
            "libdemo.so.1",
            "-Wl,-soname,libdemo.so.1",
            "lib.c",
        ],
    ) {
        eprintln!("skipped: no working C compiler");
        return;
    }
    std::os::unix::fs::symlink("libdemo.so.1", dir.join("libdemo.so")).unwrap();
    assert!(cc(
        &dir,
        &[
            "-o",
            "grafida-shell",
            "main.c",
            "-L.",
            "-ldemo",
            "-Wl,-rpath,$ORIGIN",
            "-Wl,-z,now"
        ]
    ));
    let shell = dir.join("grafida-shell");
    std::fs::remove_file(dir.join("libdemo.so.1")).unwrap();

    // What a German desktop sets. `LANGUAGE` is what gettext consults first.
    // SAFETY: this is the only test in this binary, so no other thread reads the environment.
    unsafe {
        std::env::set_var("LC_ALL", "de_DE.UTF-8");
        std::env::set_var("LANGUAGE", "de");
        std::env::set_var("LANG", "de_DE.UTF-8");
    }

    let Check::Refused(p) = check(&shell) else {
        panic!("the missing library went unrecognised: {:?}", check(&shell))
    };
    assert_eq!(p.missing, ["libdemo.so.1"]);

    let _ = std::fs::remove_dir_all(&dir);
}
