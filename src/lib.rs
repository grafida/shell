//! Grafida Shell — library half of the crate. See docs/adr/0001-shell-stack.md and
//! grafida/grafida#82.

// Here, not in the module: the module itself is only compiled in debug builds.
#[cfg(all(feature = "automation", not(debug_assertions)))]
compile_error!("the automation channel must never be compiled into a release build");

#[cfg(all(feature = "automation", debug_assertions))]
pub mod automation;
pub mod bridge;
pub mod cli;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod dialog;
pub mod http;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod log;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod manifest;
pub mod menu;
pub mod native;
pub mod navigation;
pub mod protocol;
pub mod spawn;
pub mod supervisor;
pub mod tokens;
#[cfg(windows)]
pub mod windows;
