//! Build script: on Windows, link the application manifest into the shell (grafida/grafida#91).
//!
//! The manifest declares Windows 10/11 support, per-monitor (v2) DPI awareness, long paths and
//! Common Controls 6, so native alerts and dialogs draw in the current style rather than Windows
//! 95's. It is written here as a compiled resource file (`.res`, the documented RESOURCEHEADER
//! format, one `RT_MANIFEST` entry) and handed to the linker as an input, which MSVC's `link.exe`
//! and `lld-link` both accept: no `rc.exe`, no `mt.exe`, so `cargo xwin` on Linux or macOS builds
//! the same binary as MSVC on Windows. An app's packager adds its icon and version resources next
//! to it and must keep it.

use std::{env, fs, path::PathBuf};

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Grafida.Shell" version="1.0.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

/// One resource entry: a header naming its type and name by ordinal, then the data, 4-aligned.
fn entry(out: &mut Vec<u8>, kind: u16, name: u16, language: u16, data: &[u8]) {
    let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    let u16le = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_le_bytes());
    u32le(out, data.len() as u32); // DataSize
    u32le(out, 32); // HeaderSize: these 8 bytes, TYPE and NAME (4 each), then 16 more
    u16le(out, 0xFFFF);
    u16le(out, kind); // TYPE, by ordinal
    u16le(out, 0xFFFF);
    u16le(out, name); // NAME, by ordinal
    u32le(out, 0); // DataVersion
    u16le(out, if data.is_empty() { 0 } else { 0x1030 }); // MemoryFlags: MOVEABLE | PURE | DISCARDABLE
    u16le(out, language);
    u32le(out, 0); // Version
    u32le(out, 0); // Characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let mut res = Vec::new();
    entry(&mut res, 0, 0, 0, &[]); // the empty entry every .res file starts with
    entry(&mut res, 24, 1, 0x0409, MANIFEST.as_bytes()); // RT_MANIFEST, CREATEPROCESS_MANIFEST_RESOURCE_ID
    let path = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("manifest.res");
    fs::write(&path, res).expect("write manifest.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
