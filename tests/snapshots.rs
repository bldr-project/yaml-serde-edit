//! Snapshot tests for [`YamlValue::set`]'s comment-preserving rendering.
//!
//! Every file in `tests/snapshots/*.yml` is a 3-document YAML stream, the
//! documents separated by the standard `---` marker:
//!
//! ```yaml
//! ---
//! # original: YAML with comments, parsed into a YamlValue
//! ---
//! # update: YAML parsed into the Value passed to set()
//! ---
//! # output: expected rendering of the document after set()
//! ```
//!
//! The test parses the first document, calls `set` with the `Value` from the
//! second, and checks that the rendered document equals the third.
//!
//! To (re)generate the output document of every file — including files that
//! only have the first two documents and no output yet — run:
//!
//! ```sh
//! UPDATE_SNAPSHOTS=1 cargo test --test snapshots
//! ```

use std::path::{Path, PathBuf};

use serde_norway::Value;
use yaml_serde_edit::YamlValue;

/// The standard YAML document separator, used to split the three sections.
const SEP: &str = "---";

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

/// Read a boolean env flag, treating only explicit truthy values as set — so
/// `UPDATE_SNAPSHOTS=0` (or `false`, or empty) does *not* enable update mode.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[test]
fn snapshots() {
    let update_mode = env_flag("UPDATE_SNAPSHOTS");

    let mut files: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .unwrap_or_else(|e| panic!("read {}: {e}", data_dir().display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "yml"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no `.yml` files in {}",
        data_dir().display()
    );

    let mut failures = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).unwrap();
        let docs = split_documents(&text);
        assert!(
            docs.len() >= 2,
            "{name}: expected at least `original` and `update` documents, found {}",
            docs.len()
        );
        let (original, update) = (&docs[0], &docs[1]);

        let mut doc = YamlValue::parse(original)
            .unwrap_or_else(|e| panic!("{name}: `original` failed to parse: {e}"));
        let new_value: Value = serde_norway::from_str(update)
            .unwrap_or_else(|e| panic!("{name}: `update` failed to parse: {e}"));
        doc.set(new_value);
        let actual = doc.get_string();

        if update_mode {
            std::fs::write(path, render_file(original, update, &actual)).unwrap();
            continue;
        }

        match docs.get(2) {
            Some(expected) if *expected == actual => {}
            Some(expected) => failures.push(format!(
                "── {name} ──\nexpected output:\n{}\nactual output:\n{}",
                indent(expected),
                indent(&actual)
            )),
            None => failures.push(format!("── {name} ──\nno output document yet")),
        }
    }

    if update_mode {
        eprintln!("updated {} snapshot(s)", files.len());
        return;
    }
    assert!(
        failures.is_empty(),
        "\n{}\n\nre-run with `UPDATE_SNAPSHOTS=1 cargo test --test snapshots` to update snapshots",
        failures.join("\n\n")
    );
}

/// Split a multi-document `.yml` file on its `---` separators. Each document's
/// body has trailing blank lines trimmed and a single trailing newline, so a
/// decorative blank line before a separator is ignored.
fn split_documents(text: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut started = false;

    for line in text.lines() {
        if line.trim_end() == SEP {
            if started {
                docs.push(finish(&buf));
            }
            buf.clear();
            started = true;
        } else {
            buf.push(line);
        }
    }
    if started {
        docs.push(finish(&buf));
    }
    docs
}

/// Join document lines, dropping trailing blank lines and ending with a single
/// newline (matching how the renderer always ends its output).
fn finish(lines: &[&str]) -> String {
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    if end == 0 {
        String::new()
    } else {
        let mut s = lines[..end].join("\n");
        s.push('\n');
        s
    }
}

/// Render a full `.yml` file from its three documents.
fn render_file(original: &str, update: &str, output: &str) -> String {
    format!("{SEP}\n{original}{SEP}\n{update}{SEP}\n{output}")
}

/// Indent a block by two spaces so it reads as a nested quote in failure output.
fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
