//! Grafida Shell — skeleton. See docs/adr/0001-shell-stack.md and grafida/grafida#82.

use grafida_shell::protocol::PROTOCOL_VERSION;

fn main() {
    println!(
        "grafida-shell {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_VERSION
    );
}
