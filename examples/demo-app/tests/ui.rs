//! Minimal trybuild-style harness (adapted from the stq carrier prototype;
//! `trybuild` could not be fetched in the sandbox this was built in).
//!
//! Every `tests/ui/<name>.rs` (or directory `tests/ui/<name>/` holding a
//! `lib.rs` plus other files, for cases that need several files) is wrapped
//! in a throwaway crate that depends on the workspace's example crates and the
//! UI support bundles, built with cargo, and its normalized stderr must match
//! `tests/ui/<name>.stderr`. A case with no `.stderr` must compile with no
//! output at all. A case with a `.stderr` that mentions no `error` must
//! compile (that is how warnings are checked).
//!
//! Run with `UI_OVERWRITE=1 cargo test -p demo-app --test ui` to (re)generate
//! the expected output, then read the diff.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().to_path_buf()
}

fn normalize(stderr: &str, case_dir: &Path) -> String {
    let case = case_dir.to_string_lossy();
    let root = workspace_root();
    let root = root.to_string_lossy();
    stderr
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("Compiling ")
                || t.starts_with("Finished ")
                || t.starts_with("Blocking ")
                || t.starts_with("Locking ")
                || t.starts_with("Updating ")
                || t.starts_with("Adding ")
                || t.starts_with("warning: build failed")
                || t.starts_with("error: could not compile")
                || t.starts_with("warning: `ui-"))
        })
        .map(|l| l.replace(&*case, "$DIR").replace(&*root, "$WORKSPACE"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
        + "\n"
}

fn run_case(src: &Path) {
    let name = src.file_stem().unwrap().to_string_lossy().to_string();
    let root = workspace_root();
    let case_dir = root.join("target").join("ui").join(&name);
    let _ = fs::remove_dir_all(case_dir.join("src"));
    fs::create_dir_all(case_dir.join("src")).unwrap();
    let dep = |p: &str| format!("{{ path = \"{}/{p}\" }}", root.display());
    fs::write(
        case_dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "ui-{name}"
version = "0.0.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
plenary = {plenary}
demo-macros = {demo_macros}
demo-upstream = {demo_upstream}
stq-lite = {stq_lite}
registry = {registry}
panic-bundle = {panic_bundle}
cycle-bundle = {cycle_bundle}
missing-bundle = {missing_bundle}
export-bundle = {export_bundle}

[workspace]
"#,
            plenary = dep("crates/plenary"),
            demo_macros = dep("examples/demo-macros"),
            demo_upstream = dep("examples/demo-upstream"),
            stq_lite = dep("examples/stq-lite"),
            registry = dep("examples/registry"),
            panic_bundle = dep("examples/ui-support/panic-bundle"),
            cycle_bundle = dep("examples/ui-support/cycle-bundle"),
            missing_bundle = dep("examples/ui-support/missing-bundle"),
            export_bundle = dep("examples/ui-support/export-bundle"),
        ),
    )
    .unwrap();
    if src.is_dir() {
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), case_dir.join("src").join(entry.file_name())).unwrap();
        }
    } else {
        fs::copy(src, case_dir.join("src").join("lib.rs")).unwrap();
    }

    let out = Command::new(env!("CARGO"))
        .arg("build")
        .arg("--quiet")
        .arg("--color=never")
        .current_dir(&case_dir)
        .env("CARGO_TARGET_DIR", root.join("target").join("ui-target"))
        .env_remove("RUSTFLAGS")
        .output()
        .expect("run cargo");
    let actual = normalize(&String::from_utf8_lossy(&out.stderr), &case_dir);

    let expected_path = src.with_extension("stderr");
    if std::env::var_os("UI_OVERWRITE").is_some() {
        if actual.trim().is_empty() {
            let _ = fs::remove_file(&expected_path);
        } else {
            fs::write(&expected_path, &actual).unwrap();
        }
        return;
    }
    if expected_path.exists() {
        let expected = fs::read_to_string(&expected_path).unwrap();
        let expects_error = expected.lines().any(|l| l.starts_with("error"));
        assert_eq!(
            expects_error,
            !out.status.success(),
            "\n{name}: expected {} but the build {}.\n--- stderr ---\n{actual}",
            if expects_error { "a compile error" } else { "a clean build with warnings" },
            if out.status.success() { "succeeded" } else { "failed" }
        );
        assert_eq!(
            expected.trim(),
            actual.trim(),
            "\n{name}: stderr mismatch\n--- expected ---\n{expected}\n--- actual ---\n{actual}"
        );
    } else {
        assert!(
            out.status.success() && actual.trim().is_empty(),
            "{name}: expected a clean compile with no output.\n--- stderr ---\n{actual}"
        );
    }
}

fn cases() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("ui");
    let mut v: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() || p.extension().map(|e| e == "rs").unwrap_or(false))
        .collect();
    v.sort();
    v
}

#[test]
fn ui() {
    let mut failures = Vec::new();
    for case in cases() {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        match std::panic::catch_unwind(|| run_case(&case)) {
            Ok(()) => eprintln!("ui: {name} ... ok"),
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                eprintln!("ui: {name} ... FAILED\n{msg}");
                failures.push(name);
            }
        }
    }
    assert!(failures.is_empty(), "ui cases failed: {failures:?}");
}
