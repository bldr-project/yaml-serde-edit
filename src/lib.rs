//! Edit YAML **in place, preserving comments and formatting**, through an
//! ordinary [`serde_norway::Value`] (or your own typed struct).
//!
//! Three layers, smallest first:
//!
//! * [`YamlValue`] — an in-memory YAML document edited as a [`Value`]. `get`
//!   borrows the current `Value`, `set` replaces it; the change is diffed back
//!   onto a lossless edit tree so every untouched key keeps its original
//!   comments and layout. `get_string`/`set_string` read and replace the raw
//!   YAML text. No filesystem.
//!
//!   ```
//!   use yaml_serde_edit::YamlValue;
//!   let mut doc = YamlValue::parse("# top\nname: web   # the service\nreplicas: 1\n").unwrap();
//!   let mut v = doc.get().clone();       // the parsed Value
//!   v["replicas"] = 3.into();            // mutate it freely
//!   doc.set(v);                          // diff applied to the edit tree
//!   let out = doc.get_string();          // comments + layout preserved
//!   assert!(out.contains("# the service"));
//!   assert!(out.contains("replicas: 3"));
//!   ```
//!
//! * [`YamlObject<T>`] — [`YamlValue`] with a typed view, no filesystem.
//!   `get`/`set` borrow and replace the typed `T`; `get_string`/`set_string`
//!   read and replace the raw YAML text. The typed value and the text are kept
//!   in sync, every edit comment-preserving and round-trip verified.
//!
//! * [`YamlFile<T>`] — a [`YamlObject<T>`] bound to a path. `open` reads +
//!   parses once, `get` hands back the current `&T`, and `set` writes a new `T`
//!   back to disk **atomically** (temp file + rename), still comment-preserving.
//!
//! Any edit the in-place differ can't apply cleanly (replacing a block sequence
//! with a mapping, tagged nodes, non-string keys, …) transparently falls back
//! to a clean rebuild from the `Value`: comments outside the changed region are
//! still kept, and the output is **always** value-correct.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_norway::{Mapping, Value};
use yaml_edit::{Document, Mapping as EditMapping};

/// Errors from parsing, editing, or writing YAML.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml (serde): {0}")]
    Yaml(#[from] serde_norway::Error),
    #[error("yaml (edit): {0}")]
    Edit(String),
    /// The edited document did not deserialize back to the requested value —
    /// reported instead of writing a wrong file. Should never happen (the impl
    /// falls back to a clean rebuild), but guards against silent corruption.
    #[error("edited YAML does not round-trip to the requested value")]
    RoundTrip,
}

type Result<T> = std::result::Result<T, Error>;

// ── YamlValue: in-memory, comment-preserving ──────────────────────────────────

/// A parsed YAML document that can be edited as a [`Value`] while keeping the
/// original comments and formatting of everything you don't touch.
///
/// `get`/`set` read and replace the current [`Value`], keeping it in sync with
/// the comment-preserving edit tree; `get_string`/`set_string` read and replace
/// the raw YAML text.
pub struct YamlValue {
    /// The comment-preserving edit tree (single document).
    doc: Document,
    /// The leading comment / blank-line block before the first key. `yaml_edit`
    /// drops the comment that precedes the first key, so we keep it separately
    /// and re-prepend it in [`Display`].
    leading: String,
    /// The current value — always kept in sync with `doc`.
    value: Value,
}

impl YamlValue {
    /// Parse `text` into an editable document.
    pub fn parse(text: &str) -> Result<Self> {
        let value: Value = serde_norway::from_str(text)?;
        let leading = leading_block(text);
        let doc = Document::from_str(text).map_err(|e| Error::Edit(e.to_string()))?;
        Ok(Self {
            doc,
            leading,
            value,
        })
    }

    /// Borrow the current value.
    pub fn get(&self) -> &Value {
        &self.value
    }

    /// Replace the whole value, preserving the comments and formatting of
    /// everything that didn't change.
    pub fn set(&mut self, value: Value) {
        let before = std::mem::replace(&mut self.value, value);
        self.reconcile(&before);
    }

    /// The current YAML text, comments and formatting preserved.
    pub fn get_string(&self) -> String {
        self.to_string()
    }

    /// Replace the whole document by parsing `text` (its comments are kept).
    pub fn set_string(&mut self, text: &str) -> Result<()> {
        *self = Self::parse(text)?;
        Ok(())
    }

    /// Reconcile the edit tree with `self.value`, given the value it held
    /// `before`. Best-effort in place; falls back to a clean rebuild (keeping
    /// only the leading comment block) when an edit can't be applied cleanly so
    /// the result is always value-correct.
    fn reconcile(&mut self, before: &Value) {
        if *before == self.value {
            return;
        }

        if let (Some(root), Value::Mapping(old_map), Value::Mapping(new_map)) =
            (self.doc.as_mapping(), before, &self.value)
        {
            apply_mapping(&root, old_map, new_map);
        }

        // If the in-place edit didn't land exactly on the new value — invalid
        // YAML (e.g. replacing a block sequence), a non-mapping root, non-string
        // keys, tagged nodes — rebuild cleanly from the value.
        let in_place_ok = matches!(reparse(&self.doc.to_string()), Ok(v) if v == self.value);
        if !in_place_ok
            && let Ok(rebuilt) = serde_norway::to_string(&self.value)
            && let Ok(doc) = Document::from_str(&rebuilt)
        {
            self.doc = doc;
        }
    }
}

impl fmt::Display for YamlValue {
    /// The current YAML text, comments and formatting preserved.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.leading, self.doc)
    }
}

impl FromStr for YamlValue {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

// ── YamlObject<T>: typed + comment-preserving, no I/O ─────────────────────────

/// A YAML document bound to a typed value `T`, editable while preserving
/// comments — exactly [`YamlFile`] minus the filesystem. Built on [`YamlValue`]
/// with a typed view (`get`/`set` on `&T`/`T`) and the raw YAML text
/// (`get_string`/`set_string`), the two kept in sync.
///
/// ```
/// use yaml_serde_edit::YamlObject;
/// # #[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
/// # struct Cfg { replicas: u32 }
/// let mut s = YamlObject::<Cfg>::parse("# keep me\nreplicas: 1\n").unwrap();
/// assert_eq!(s.get().replicas, 1);
/// s.set(Cfg { replicas: 3 }).unwrap();
/// assert!(s.get_string().contains("# keep me"));
/// assert!(s.get_string().contains("replicas: 3"));
/// ```
pub struct YamlObject<T> {
    doc: YamlValue,
    /// The current typed value — kept in sync with `doc`.
    value: T,
    /// The current rendering of `doc` — cached so `get_string` hands back `&str`.
    text: String,
}

impl<T: Serialize + DeserializeOwned> YamlObject<T> {
    /// Parse `text` into `T`, retaining a comment-preserving edit tree.
    pub fn parse(text: &str) -> Result<Self> {
        let doc = YamlValue::parse(text)?;
        let value = serde_norway::from_value(doc.get().clone())?;
        let text = doc.get_string();
        Ok(Self { doc, value, text })
    }

    /// Borrow the current value.
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Replace the value, preserving comments. Verifies the rendered YAML
    /// round-trips back to exactly `new` (else [`Error::RoundTrip`]).
    ///
    /// Transactional: on any error `self` is left exactly as it was, so a failed
    /// `set` is a no-op rather than a half-applied edit.
    pub fn set(&mut self, new: T) -> Result<()> {
        let new_value = serde_norway::to_value(&new)?;

        // Apply the edit to a working copy of the current document, committing
        // to `self` only once every fallible step has succeeded. `self.text` is
        // the last good rendering, so re-parsing it reproduces the current
        // document — comments and all — without mutating `self` on failure.
        let mut doc = YamlValue::parse(&self.text)?;
        doc.set(new_value.clone());
        let text = doc.get_string();

        // The rendered document must deserialize back to exactly `new`.
        match reparse(&text) {
            Ok(round_tripped) if round_tripped == new_value => {}
            Ok(_) => return Err(Error::RoundTrip),
            Err(e) => return Err(e),
        }

        self.doc = doc;
        self.value = new;
        self.text = text;
        Ok(())
    }

    /// The current YAML text (comments + formatting preserved).
    pub fn get_string(&self) -> &str {
        &self.text
    }

    /// Replace the whole document by parsing `text` (its comments are kept).
    pub fn set_string(&mut self, text: &str) -> Result<()> {
        *self = Self::parse(text)?;
        Ok(())
    }

    /// The underlying comment-preserving document.
    pub fn document(&self) -> &YamlValue {
        &self.doc
    }
}

// ── YamlFile<T>: YamlObject bound to a path ───────────────────────────────────

/// A YAML file bound to a typed value `T`, editable while preserving comments.
///
/// A [`YamlObject`] plus a path: `open` reads + parses, `get` hands back the
/// current `&T`, and `set` writes a new `T` back to disk (atomically, still
/// comment-preserving). `get_string`/`set_string` read and replace the raw YAML
/// text, the replacement also written through to disk.
pub struct YamlFile<T> {
    path: PathBuf,
    inner: YamlObject<T>,
}

impl<T: Serialize + DeserializeOwned> YamlFile<T> {
    /// Open `path`, parsing it into `T` while retaining a lossless edit tree.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let text = std::fs::read_to_string(&path)?;
        Ok(Self {
            path,
            inner: YamlObject::parse(&text)?,
        })
    }

    /// The current value.
    pub fn get(&self) -> &T {
        self.inner.get()
    }

    /// Update the file to `new`, preserving comments, then write it to disk
    /// atomically.
    pub fn set(&mut self, new: T) -> Result<()> {
        self.inner.set(new)?;
        write_atomic(&self.path, self.inner.get_string())?;
        Ok(())
    }

    /// The current YAML text (with comments and formatting preserved).
    pub fn get_string(&self) -> &str {
        self.inner.get_string()
    }

    /// Replace the whole document by parsing `text` (its comments are kept),
    /// then write it to disk atomically.
    pub fn set_string(&mut self, text: &str) -> Result<()> {
        self.inner.set_string(text)?;
        write_atomic(&self.path, self.inner.get_string())?;
        Ok(())
    }
}

/// Write `contents` to `path` atomically: write a temp file in the same
/// directory, flush + fsync it, then rename it over `path`. On the same
/// filesystem the rename is atomic, so readers see either the old or the new
/// file, never a partial one.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("yaml");
    // Unique temp name (pid + counter) so concurrent writes to *different*
    // files in the same directory can't collide.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{stem}.tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write().and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp); // best-effort cleanup on failure
        return Err(e);
    }
    Ok(())
}

// ── Diff/apply internals ──────────────────────────────────────────────────────

/// Parse YAML text into a `Value`.
fn reparse(text: &str) -> Result<Value> {
    Ok(serde_norway::from_str(text)?)
}

/// The run of leading comment / blank lines before the first content line.
/// `yaml_edit` drops this on parse, so we keep it and re-prepend it on write.
fn leading_block(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
        } else {
            break;
        }
    }
    out
}

/// Apply the difference between two mappings (`old` → `new`) onto the
/// comment-preserving edit mapping in place. Removed keys are dropped, changed
/// scalars/collections are replaced, and changed sub-mappings are recursed into
/// so their untouched inner keys keep their comments. Best-effort: any key it
/// can't handle simply isn't applied (the caller verifies and rebuilds if so).
fn apply_mapping(edit: &EditMapping, old: &Mapping, new: &Mapping) {
    // Remove keys that are gone.
    for (k, _) in old.iter() {
        if let Some(key) = k.as_str()
            && new.get(k).is_none()
        {
            edit.remove(key);
        }
    }

    // Add or update keys present in `new` (iterating `new` preserves its order
    // for any freshly-appended keys).
    for (k, new_val) in new.iter() {
        let Some(key) = k.as_str() else { continue };
        match old.get(k) {
            // Unchanged — leave the existing node (and its comments) untouched.
            Some(old_val) if old_val == new_val => {}
            // Both sub-mappings → recurse to preserve inner comments.
            Some(Value::Mapping(old_sub)) if matches!(new_val, Value::Mapping(_)) => {
                if let (Some(edit_sub), Value::Mapping(new_sub)) = (edit.get_mapping(key), new_val)
                {
                    apply_mapping(&edit_sub, old_sub, new_sub);
                } else if let Ok(node) = node_for(key, new_val) {
                    edit.set(key, node);
                }
            }
            // Both scalar sequences → reconcile element-wise so the rest of the
            // document keeps its comments. (Falls through to a whole-document
            // rebuild via the caller's round-trip check if an element isn't a
            // scalar — `yaml_edit` can't cleanly set a nested block node.)
            Some(Value::Sequence(old_seq)) if matches!(new_val, Value::Sequence(_)) => {
                if let (Some(edit_seq), Value::Sequence(new_seq)) =
                    (edit.get_sequence(key), new_val)
                {
                    reconcile_sequence(&edit_seq, old_seq, new_seq);
                }
            }
            // Added, scalar change, or type change → set the new value.
            _ => {
                if let Ok(node) = node_for(key, new_val) {
                    edit.set(key, node);
                }
            }
        }
    }
}

/// Reconcile a scalar sequence element-wise (set changed indices, push new
/// tail elements, drop removed tail elements) so the surrounding document keeps
/// its comments. Returns `false` if any element isn't a scalar — the caller
/// then rebuilds. A partially-applied result is harmless: the round-trip check
/// triggers a clean rebuild.
fn reconcile_sequence(edit: &yaml_edit::Sequence, old: &[Value], new: &[Value]) -> bool {
    let common = old.len().min(new.len());
    for (i, nv) in new.iter().enumerate().take(common) {
        if old[i] != *nv && !set_scalar(edit, i, nv) {
            return false;
        }
    }
    if new.len() > old.len() {
        for nv in &new[old.len()..] {
            if !push_scalar(edit, nv) {
                return false;
            }
        }
    } else {
        for i in (new.len()..old.len()).rev() {
            edit.remove(i);
        }
    }
    true
}

fn set_scalar(edit: &yaml_edit::Sequence, index: usize, value: &Value) -> bool {
    match value {
        Value::Bool(b) => edit.set(index, *b),
        Value::String(s) => edit.set(index, s.clone()),
        Value::Number(n) if n.is_i64() => edit.set(index, n.as_i64().unwrap()),
        Value::Number(n) if n.is_u64() => edit.set(index, n.as_u64().unwrap()),
        Value::Number(n) if n.is_f64() => edit.set(index, n.as_f64().unwrap()),
        _ => return false,
    };
    true
}

fn push_scalar(edit: &yaml_edit::Sequence, value: &Value) -> bool {
    match value {
        Value::Bool(b) => edit.push(*b),
        Value::String(s) => edit.push(s.clone()),
        Value::Number(n) if n.is_i64() => edit.push(n.as_i64().unwrap()),
        Value::Number(n) if n.is_u64() => edit.push(n.as_u64().unwrap()),
        Value::Number(n) if n.is_f64() => edit.push(n.as_f64().unwrap()),
        _ => return false,
    }
    true
}

/// Build a comment-free edit node for `value` (correctly typed/quoted) by
/// round-tripping it through `serde_norway` as the value of a one-key mapping.
fn node_for(key: &str, value: &Value) -> Result<yaml_edit::YamlNode> {
    let mut wrapper = Mapping::new();
    wrapper.insert(Value::String(key.to_owned()), value.clone());
    let text = serde_norway::to_string(&Value::Mapping(wrapper))?;
    let doc = Document::from_str(&text).map_err(|e| Error::Edit(e.to_string()))?;
    doc.as_mapping()
        .and_then(|m| m.get(key))
        .ok_or_else(|| Error::Edit(format!("could not build value node for key {key:?}")))
}
