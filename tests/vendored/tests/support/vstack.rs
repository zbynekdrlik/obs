//! Issue 1487: the pinned vstack source. In obs, `vstack.pin` (`tag=`, `commit=`) names the tag and
//! the CI job checks it out (as the strih build does) and hands its path in `VSTACK_SRC`. The
//! checkout must be the pinned commit. A missing or wrong checkout FAILS, never skips.
//! Use with `#[path = "support/vstack.rs"] mod vstack;`.

#![allow(dead_code)] // each test binary uses part of this file

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repo root of this checkout.
fn repo() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// The pin file's value of KEY (`tag` or `commit`).
pub fn pin(key: &str) -> String {
    let p = repo().join("vstack.pin");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    text.lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key} in {}", p.display()))
        .to_string()
}

/// The vstack checkout in `VSTACK_SRC`, verified to be the pinned commit.
pub fn root() -> PathBuf {
    let commit = pin("commit");
    let dir = PathBuf::from(std::env::var("VSTACK_SRC").unwrap_or_else(|_| {
        panic!(
            "VSTACK_SRC is not set: check out zbynekdrlik/vstack at {} ({commit}) and point \
             VSTACK_SRC at it (linux-genlock.yml's vendored-tests job does)",
            pin("tag")
        )
    }));
    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|e| panic!("git rev-parse in {}: {e}", dir.display()));
    assert!(
        out.status.success(),
        "VSTACK_SRC={} is not a git checkout: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        head,
        commit,
        "VSTACK_SRC={} is at {head}, vstack.pin says {commit}",
        dir.display()
    );
    dir
}

/// A path inside the pinned tag.
pub fn path(rel: &str) -> PathBuf {
    root().join(rel)
}

/// The text of a file inside the pinned tag.
pub fn read(rel: &str) -> String {
    let p: PathBuf = path(rel);
    read_path(&p)
}

fn read_path(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}
