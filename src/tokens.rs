//! File tokens: what a native dialog hands the page instead of a path.
//!
//! A token is `<session>-<secret>`: the session part is random per shell process, and the secret
//! part is 128 random bits per pick. The page can only use a token to name a file the user picked
//! in this session; a token from an earlier session is recognisably stale rather than merely
//! unknown, which makes the error honest without making it any more usable.

use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
    /// A save-dialog destination; the file may not exist yet.
    SaveTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Not a token this shell ever issued.
    Unknown,
    /// Shaped like a token, but issued by an earlier session.
    Stale,
}

impl TokenError {
    pub fn code(self) -> &'static str {
        match self {
            TokenError::Unknown => "unknown_file_token",
            TokenError::Stale => "stale_file_token",
        }
    }
}

pub struct TokenStore {
    session: String,
    picked: HashMap<String, Picked>,
}

impl Default for TokenStore {
    fn default() -> Self {
        TokenStore::new()
    }
}

impl TokenStore {
    pub fn new() -> TokenStore {
        TokenStore {
            session: random_hex(8),
            picked: HashMap::new(),
        }
    }

    pub fn issue(&mut self, path: PathBuf, kind: Kind) -> String {
        let token = format!("{}-{}", self.session, random_hex(16));
        self.picked.insert(token.clone(), Picked { path, kind });
        token
    }

    pub fn resolve(&self, token: &str) -> Result<&Picked, TokenError> {
        if let Some(picked) = self.picked.get(token) {
            return Ok(picked);
        }
        match token.split_once('-') {
            Some((session, secret))
                if session != self.session && is_hex(session, 16) && is_hex(secret, 32) =>
            {
                Err(TokenError::Stale)
            }
            _ => Err(TokenError::Unknown),
        }
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `bytes` random bytes from the OS, as lower-case hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the OS random number generator");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_resolves_in_its_own_session_only() {
        let mut first = TokenStore::new();
        let token = first.issue("/home/u/a.png".into(), Kind::File);
        assert_eq!(
            first.resolve(&token).unwrap().path,
            PathBuf::from("/home/u/a.png")
        );

        let second = TokenStore::new();
        assert_eq!(second.resolve(&token), Err(TokenError::Stale));
    }

    #[test]
    fn forged_tokens_are_unknown() {
        let mut store = TokenStore::new();
        let real = store.issue("/x".into(), Kind::File);
        let (session, _) = real.split_once('-').unwrap();
        let forged = format!("{session}-{}", "0".repeat(32));
        for t in ["", "x", "/etc/passwd", &forged, &real[..real.len() - 1]] {
            assert_eq!(store.resolve(t), Err(TokenError::Unknown), "{t}");
        }
    }

    #[test]
    fn tokens_are_unique() {
        let mut store = TokenStore::new();
        let a = store.issue("/x".into(), Kind::File);
        let b = store.issue("/x".into(), Kind::File);
        assert_ne!(a, b);
        assert_eq!(a.len(), 16 + 1 + 32);
    }
}
