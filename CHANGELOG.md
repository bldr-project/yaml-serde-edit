# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-06-23

Initial release.

### Added

- `YamlValue` — an in-memory YAML document edited as a `serde_norway::Value`.
  `get`/`set` read and replace the value while preserving the comments and
  formatting of everything left untouched; `get_string`/`set_string` read and
  replace the raw YAML text.
- `YamlObject<T>` — `YamlValue` with a typed view. `get`/`set` borrow and
  replace your `T`, with every edit round-trip verified (`set` is transactional:
  on failure the object is left unchanged).
- `YamlFile<T>` — a `YamlObject<T>` bound to a path, with atomic
  (temp-file + rename) comment-preserving writes to disk.
- In-place diff/apply onto a lossless edit tree, with a transparent fallback to
  a clean rebuild from the value when an edit can't be applied in place, so the
  output is always value-correct.
- Snapshot test harness (`tests/snapshots/*.yml`) over multi-document YAML
  streams, regenerated with `UPDATE_SNAPSHOTS=1`.

[Unreleased]: https://github.com/bldr-project/yaml-serde-edit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/bldr-project/yaml-serde-edit/releases/tag/v0.1.0
