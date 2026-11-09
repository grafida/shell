//! Grafida Shell — skeleton. See docs/adr/0001-shell-stack.md and grafida/grafida#82.

/// Wire protocol version; must equal `Grafida\Shell\Protocol::VERSION` in grafida/shell-php.
pub const PROTOCOL_VERSION: u32 = 1;

fn main() {
    println!(
        "grafida-shell {} (protocol {})",
        env!("CARGO_PKG_VERSION"),
        PROTOCOL_VERSION
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn protocol_version_is_one() {
        assert_eq!(super::PROTOCOL_VERSION, 1);
    }
}
