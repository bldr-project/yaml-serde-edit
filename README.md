# yaml-serde-edit

[![CI](https://github.com/bldr-project/yaml-serde-edit/actions/workflows/ci.yml/badge.svg)](https://github.com/bldr-project/yaml-serde-edit/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/yaml-serde-edit.svg)](https://crates.io/crates/yaml-serde-edit)
[![docs.rs](https://img.shields.io/docsrs/yaml-serde-edit)](https://docs.rs/yaml-serde-edit)
[![license](https://img.shields.io/crates/l/yaml-serde-edit.svg)](#license)

Edit YAML **in place, preserving comments and formatting**, through an ordinary
[`serde_norway::Value`] (or your own typed struct).

You read a YAML document into a value, change the value however you like, and
write it back — and every key, comment, and bit of layout you *didn't* touch
stays byte-for-byte the same. Edits are applied as a diff onto a lossless edit
tree; anything the in-place differ can't apply cleanly transparently falls back
to a clean rebuild, so the output is **always** value-correct.

```toml
[dependencies]
yaml-serde-edit = "0.1"
```

## Three layers

The crate is three layers, smallest first. Pick the one that matches how much
structure you want.

### `YamlValue` — in-memory, untyped

An editable YAML document viewed as a [`serde_norway::Value`]. `get`/`set` read
and replace the value; `get_string`/`set_string` read and replace the raw YAML
text. No filesystem.

```rust
use yaml_serde_edit::YamlValue;

let mut doc = YamlValue::parse("# top\nname: web   # the service\nreplicas: 1\n").unwrap();

let mut v = doc.get().clone();   // the parsed Value
v["replicas"] = 3.into();        // mutate it freely
doc.set(v);                      // diff applied to the edit tree

let out = doc.get_string();      // comments + layout preserved
assert!(out.contains("# the service"));
assert!(out.contains("replicas: 3"));
```

### `YamlObject<T>` — in-memory, typed

`YamlValue` with a typed view. `get`/`set` borrow and replace your `T`;
`get_string`/`set_string` read and replace the raw text. The typed value and the
text are kept in sync, every edit comment-preserving and round-trip verified
(an edit that wouldn't deserialize back to exactly your `T` is reported as an
error instead of written).

```rust
use yaml_serde_edit::YamlObject;

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct Cfg { replicas: u32 }

let mut s = YamlObject::<Cfg>::parse("# keep me\nreplicas: 1\n").unwrap();
assert_eq!(s.get().replicas, 1);

s.set(Cfg { replicas: 3 }).unwrap();
assert!(s.get_string().contains("# keep me"));
assert!(s.get_string().contains("replicas: 3"));
```

### `YamlFile<T>` — typed, on disk

A `YamlObject<T>` bound to a path. `open` reads + parses once, `get` hands back
the current `&T`, and `set` writes a new `T` back to disk **atomically** (temp
file + rename), still comment-preserving. `get_string`/`set_string` read and
replace the raw text, the replacement also written through to disk.

```rust,no_run
use yaml_serde_edit::YamlFile;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Cfg { replicas: u32 }

let mut file = YamlFile::<Cfg>::open("config.yml").unwrap();

let mut cfg = file.get().clone();
cfg.replicas = 3;
file.set(cfg).unwrap();          // written atomically, comments preserved
```

## What "comment-preserving" means

Edits are diffed against a lossless edit tree, so untouched keys keep their
original comments and layout exactly. Changed sub-mappings are recursed into so
their inner comments survive; scalar sequences are reconciled element-wise.

Some edits can't be applied in place — replacing a block sequence with a
mapping, tagged nodes, non-string keys, and so on. When that happens the
document is rebuilt cleanly from the value: comments outside the changed region
(notably the leading comment block) are still kept, and the result is guaranteed
to equal the value you asked for.

## Testing

```sh
cargo test
```

### Snapshot tests

`tests/snapshots.rs` drives a set of golden files in `tests/snapshots/`. Each is
a 3-document YAML stream separated by the standard `---` marker:

```yaml
---
# original — parsed into a YamlValue
version: "3.9"
replicas: 1   # how many
---
# update — parsed into the Value passed to set()
version: "3.9"
replicas: 3
---
# output — expected rendering after set()
version: "3.9"
replicas: 3   # how many
```

The test parses the first document, calls `set` with the value from the second,
and asserts the rendered document equals the third.

To add a case, drop a `.yml` file in `tests/snapshots/` with just the first two
documents, then regenerate the output. To update existing snapshots after an
intentional change, the same command rewrites every output document:

```sh
UPDATE_SNAPSHOTS=1 cargo test --test snapshots
```

Review the regenerated `output` documents in your diff before committing.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

[`serde_norway::Value`]: https://docs.rs/serde_norway/latest/serde_norway/enum.Value.html
