//! Runs every conformance fixture in `protocol/fixtures/` against the Rust codec
//! (protocol/v1.md §8). A failing fixture is a question about which side is wrong — the codec,
//! the PHP worker or the spec — never a file to regenerate.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use base64::Engine;
use grafida_shell::protocol::{DEFAULT_MAX_BODY_BYTES, Frame, encode_frame, read_frame};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/fixtures")
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".json").map(str::to_string)
        })
        .collect();
    names.sort();
    assert!(names.len() > 50, "fixtures missing: {}", names.len());
    names
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The outcome of decoding a whole stream: every frame, then "eof" or an error code.
fn decode_all<R: Read>(mut reader: R) -> (Vec<Frame>, String) {
    let mut frames = Vec::new();
    loop {
        match read_frame(&mut reader, DEFAULT_MAX_BODY_BYTES) {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => return (frames, "eof".into()),
            Err(e) => {
                let code = e
                    .code()
                    .unwrap_or_else(|| panic!("i/o error, not a protocol error: {e}"));
                return (frames, code.into());
            }
        }
    }
}

fn check(name: &str, frames: &[Frame], end: &str, expected: &Value) {
    assert_eq!(end, expected["end"].as_str().unwrap(), "{name}: end");
    let want = expected["frames"].as_array().unwrap();
    assert_eq!(frames.len(), want.len(), "{name}: frame count");

    for (i, (got, want)) in frames.iter().zip(want).enumerate() {
        assert_eq!(
            got.header.to_value(),
            want["header"],
            "{name}: frame {i} header"
        );

        let body = &want["body"];
        assert_eq!(
            got.body.len() as u64,
            body["length"].as_u64().unwrap(),
            "{name}: frame {i} body length"
        );
        assert_eq!(
            sha256_hex(&got.body),
            body["sha256"].as_str().unwrap(),
            "{name}: frame {i} body sha256"
        );
        if let Some(text) = body.get("text") {
            assert_eq!(
                got.body,
                text.as_str().unwrap().as_bytes(),
                "{name}: frame {i} body text"
            );
        }
        if let Some(b64) = body.get("base64") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.as_str().unwrap())
                .unwrap();
            assert_eq!(got.body, bytes, "{name}: frame {i} body base64");
        }
    }
}

fn load(name: &str) -> (Vec<u8>, Value) {
    let dir = fixtures_dir();
    let bin = fs::read(dir.join(format!("{name}.bin"))).unwrap();
    let json =
        serde_json::from_slice(&fs::read(dir.join(format!("{name}.json"))).unwrap()).unwrap();
    (bin, json)
}

#[test]
fn every_fixture_decodes_as_specified() {
    for name in fixture_names() {
        let (bin, expected) = load(&name);
        let (frames, end) = decode_all(bin.as_slice());
        check(&name, &frames, &end, &expected);
    }
}

/// A reader that hands out one byte per `read` call: the decoder must assemble frames from short
/// reads (wry's Linux body reader stopped at the first one; ADR 0001).
struct OneByte<'a>(&'a [u8]);

impl Read for OneByte<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match (self.0.split_first(), buf.is_empty()) {
            (Some((b, rest)), false) => {
                buf[0] = *b;
                self.0 = rest;
                Ok(1)
            }
            _ => Ok(0),
        }
    }
}

#[test]
fn every_fixture_decodes_the_same_from_one_byte_reads() {
    for name in fixture_names() {
        let (bin, expected) = load(&name);
        let (frames, end) = decode_all(OneByte(&bin));
        check(&name, &frames, &end, &expected);
    }
}

/// A pipe whose writer never closes: reading past the data would block forever, so here it panics.
struct OpenPipe<'a>(&'a [u8]);

impl Read for OpenPipe<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        assert!(
            !self.0.is_empty() || buf.is_empty(),
            "the decoder waited for bytes that would never come"
        );
        let n = buf.len().min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

/// "A clean protocol error, never a hang": every error other than `truncated` (which is defined
/// by EOF) must be reported from the bytes already received, even if the writer keeps the pipe
/// open.
#[test]
fn errors_are_reported_without_waiting_for_more_input() {
    for name in fixture_names() {
        let (bin, expected) = load(&name);
        let end = expected["end"].as_str().unwrap();
        if end == "eof" || end == "truncated" {
            continue;
        }
        let (frames, got) = decode_all(OpenPipe(&bin));
        check(&name, &frames, &got, &expected);
    }
}

#[test]
fn every_valid_frame_round_trips_through_the_encoder() {
    for name in fixture_names() {
        let (bin, _) = load(&name);
        let (frames, _) = decode_all(bin.as_slice());
        for (i, frame) in frames.iter().enumerate() {
            let encoded = encode_frame(frame).unwrap_or_else(|e| panic!("{name}: frame {i}: {e}"));
            let back = read_frame(&mut encoded.as_slice(), DEFAULT_MAX_BODY_BYTES)
                .unwrap()
                .unwrap();
            assert_eq!(&back, frame, "{name}: frame {i} round trip");
        }
    }
}

/// `SHA256SUMS` lists exactly the fixture files, with their current hashes. shell-php vendors the
/// directory and checks its copy against the same manifest.
#[test]
fn checksum_manifest_matches_the_fixtures() {
    let dir = fixtures_dir();
    let manifest = fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    let mut listed = Vec::new();
    for line in manifest.lines() {
        let (digest, file) = line.split_once("  ").unwrap();
        let bytes =
            fs::read(dir.join(file)).unwrap_or_else(|_| panic!("{file} is listed but missing"));
        assert_eq!(sha256_hex(&bytes), digest, "{file} differs from SHA256SUMS");
        listed.push(file.to_string());
    }

    let mut present: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|f| f.ends_with(".bin") || f.ends_with(".json"))
        .collect();
    present.sort();
    listed.sort();
    assert_eq!(
        present, listed,
        "SHA256SUMS does not list exactly the fixture files"
    );
}
