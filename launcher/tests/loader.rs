#![cfg(target_os = "linux")]

//! The check against the real system loader: tiny C programs whose libraries are then broken.
//! Needs a C compiler (`cc`); skipped with a note when there is none.

use std::{path::Path, process::Command};

use grafida_launcher::{Check, check};

fn cc(dir: &Path, args: &[&str]) -> bool {
    Command::new("cc")
        .current_dir(dir)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn fixture(name: &str) -> Option<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("grafida-launcher-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("full.c"),
        "int present(void){return 1;}\nint gone(void){return 2;}\n",
    )
    .unwrap();
    std::fs::write(dir.join("part.c"), "int present(void){return 1;}\n").unwrap();
    std::fs::write(
        dir.join("main.c"),
        "int present(void); int gone(void);\nint main(void){return present()+gone();}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("v2.map"),
        "DEMO_1 { global: present; local: *; };\nDEMO_2 { global: gone; } DEMO_1;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("v1.map"),
        "DEMO_1 { global: present; local: *; };\n",
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
            "-Wl,--version-script=v2.map",
            "full.c",
        ],
    ) {
        eprintln!("skipped: no working C compiler");
        return None;
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
    Some(dir)
}

#[test]
fn the_real_loader() {
    let Some(dir) = fixture("loader") else { return };
    let shell = dir.join("grafida-shell");

    assert_eq!(check(&shell), Check::Loads);

    // A library that lost a symbol version and a symbol: an older WebKitGTK, say.
    assert!(cc(
        &dir,
        &[
            "-shared",
            "-fPIC",
            "-o",
            "libdemo.so.1",
            "-Wl,-soname,libdemo.so.1",
            "-Wl,--version-script=v1.map",
            "part.c"
        ]
    ));
    let Check::Refused(p) = check(&shell) else {
        panic!("{:?}", check(&shell))
    };
    assert_eq!(p.versions, ["DEMO_2"]);
    assert_eq!(p.symbols, ["gone"]);
    assert!(p.missing.is_empty());

    // A library that is not there at all.
    std::fs::remove_file(dir.join("libdemo.so.1")).unwrap();
    let Check::Refused(p) = check(&shell) else {
        panic!("{:?}", check(&shell))
    };
    assert_eq!(p.missing, ["libdemo.so.1"]);

    // A program interpreter that does not exist (a musl system has no glibc loader): rename the
    // loader in PT_INTERP, keeping its length, on either architecture.
    let elf = std::fs::read(&shell).unwrap();
    let needle = b"/ld-linux-";
    let at = elf
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("a glibc program names its loader");
    let mut broken = elf;
    broken[at + 1..at + 3].copy_from_slice(b"xx");
    let path = dir.join("noloader");
    std::fs::write(&path, broken).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert!(
        matches!(check(&path), Check::Refused(_)),
        "{:?}",
        check(&path)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
