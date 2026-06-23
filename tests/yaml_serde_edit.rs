//! Integration tests for `yaml-serde-edit`, exercising only the public API.
//!
//! The example types are a deliberately generic subset of what a
//! `docker compose` reader would use for `compose.yml`, so the tests cover
//! realistic nesting: a top-level map, a map of service mappings, scalar
//! sequences, optional fields, and free-form (`Value`) sections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_norway::Value;
use yaml_serde_edit::{YamlFile, YamlObject, YamlValue};

// ── Docker-compose-ish example schema ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Compose {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    services: BTreeMap<String, Service>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    volumes: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    networks: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Service {
    image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    container_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    environment: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restart: Option<String>,
}

const COMPOSE: &str = "\
# Project compose file — keep me!
version: \"3.9\"

services:
  web:
    image: nginx:1.25      # pin the version
    ports:
      - \"80:80\"
      - \"443:443\"
    depends_on:
      - api
  api:
    # the backend
    image: ghcr.io/acme/api:v2
    environment:
      - LOG_LEVEL=info
    restart: unless-stopped

volumes:
  db-data: {}   # named volume
";

fn write_temp(yaml: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compose.yml");
    std::fs::write(&path, yaml).unwrap();
    (dir, path)
}

/// Deserialize the document text back to a `Value` for exact comparisons.
fn as_value(text: &str) -> Value {
    serde_norway::from_str(text).unwrap()
}

// ── YamlFile<T> (typed, on disk) ──────────────────────────────────────────────

#[test]
fn reads_compose_into_typed_value() {
    let (_d, path) = write_temp(COMPOSE);
    let file: YamlFile<Compose> = YamlFile::open(&path).unwrap();
    let c = file.get();
    assert_eq!(c.version.as_deref(), Some("3.9"));
    assert_eq!(c.services.len(), 2);
    assert_eq!(c.services["web"].image, "nginx:1.25");
    assert_eq!(c.services["web"].ports, vec!["80:80", "443:443"]);
    assert_eq!(c.services["api"].restart.as_deref(), Some("unless-stopped"));
    assert!(c.volumes.contains_key("db-data"));
}

#[test]
fn edit_one_service_keeps_other_services_comments() {
    let (_d, path) = write_temp(COMPOSE);
    let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();

    let mut c = file.get().clone();
    c.services.get_mut("web").unwrap().image = "nginx:1.27".to_string();
    file.set(c.clone()).unwrap();

    let text = file.get_string();
    assert!(text.contains("image: nginx:1.27"));
    // Untouched comments and formatting survive.
    assert!(text.contains("# Project compose file — keep me!"));
    assert!(text.contains("# the backend"));
    assert!(text.contains("# named volume"));
    // Exactly the requested value landed on disk.
    assert_eq!(as_value(text), serde_norway::to_value(&c).unwrap());
    assert_eq!(file.get(), &c);
}

#[test]
fn edit_scalar_sequence_is_value_correct() {
    // Rewriting a sequence nested several levels deep can exceed what the
    // in-place differ applies cleanly, so it may fall back to a rebuild. Either
    // way the result must be exactly the requested value, and the leading
    // comment block (re-prepended unconditionally) always survives.
    let (_d, path) = write_temp(COMPOSE);
    let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();

    let mut c = file.get().clone();
    c.services.get_mut("web").unwrap().ports = vec!["8080:80".to_string()];
    file.set(c.clone()).unwrap();

    let text = file.get_string();
    assert!(text.contains("8080:80"));
    assert!(!text.contains("443:443"));
    assert!(text.contains("# Project compose file — keep me!"));
    assert_eq!(as_value(text), serde_norway::to_value(&c).unwrap());
    assert_eq!(file.get(), &c);
}

/// A *root-level* scalar sequence reconciles in place, keeping comments.
#[test]
fn edit_top_level_sequence_preserves_comments() {
    let src = "# tags list\ntags:\n  - a   # first\n  - b\n  - c\nname: web  # service\n";
    let (_d, path) = write_temp(src);

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Tagged {
        tags: Vec<String>,
        name: String,
    }

    let mut file: YamlFile<Tagged> = YamlFile::open(&path).unwrap();
    let mut c = file.get().clone();
    c.tags = vec!["a".into(), "B".into(), "c".into(), "d".into()];
    file.set(c.clone()).unwrap();

    let text = file.get_string();
    assert!(text.contains("# tags list"));
    assert!(text.contains("# service"));
    assert!(text.contains("- d")); // appended tail element
    assert_eq!(as_value(text), serde_norway::to_value(&c).unwrap());
}

#[test]
fn add_and_remove_service_is_value_correct() {
    let (_d, path) = write_temp(COMPOSE);
    let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();

    let mut c = file.get().clone();
    c.services.insert(
        "cache".to_string(),
        Service {
            image: "redis:7".to_string(),
            container_name: None,
            ports: vec!["6379:6379".to_string()],
            environment: Vec::new(),
            depends_on: Vec::new(),
            restart: None,
        },
    );
    file.set(c.clone()).unwrap();
    assert!(file.get_string().contains("redis:7"));
    assert_eq!(
        as_value(file.get_string()),
        serde_norway::to_value(&c).unwrap()
    );

    let mut c2 = file.get().clone();
    c2.services.remove("cache");
    file.set(c2.clone()).unwrap();
    assert!(!file.get_string().contains("redis:7"));
    assert_eq!(
        as_value(file.get_string()),
        serde_norway::to_value(&c2).unwrap()
    );
}

#[test]
fn no_op_set_keeps_bytes_identical() {
    let (_d, path) = write_temp(COMPOSE);
    let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();
    let same = file.get().clone();
    file.set(same).unwrap();
    assert_eq!(file.get_string(), COMPOSE);
}

#[test]
fn set_persists_to_disk_and_reopens() {
    let (_d, path) = write_temp(COMPOSE);
    {
        let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();
        let mut c = file.get().clone();
        c.version = Some("3.10".to_string());
        c.services.get_mut("api").unwrap().restart = Some("always".to_string());
        file.set(c).unwrap();
    }
    let reopened: YamlFile<Compose> = YamlFile::open(&path).unwrap();
    assert_eq!(reopened.get().version.as_deref(), Some("3.10"));
    assert_eq!(
        reopened.get().services["api"].restart.as_deref(),
        Some("always")
    );
    assert!(
        reopened
            .get_string()
            .contains("# Project compose file — keep me!")
    );
}

#[test]
fn many_sequential_edits_round_trip() {
    let (_d, path) = write_temp(COMPOSE);
    let mut file: YamlFile<Compose> = YamlFile::open(&path).unwrap();
    for i in 0..5u32 {
        let mut c = file.get().clone();
        c.services.get_mut("web").unwrap().image = format!("nginx:1.{i}");
        c.services.get_mut("api").unwrap().environment = vec![format!("LOG_LEVEL=lvl{i}")];
        file.set(c.clone()).unwrap();
        assert_eq!(file.get(), &c);
        assert_eq!(
            as_value(file.get_string()),
            serde_norway::to_value(&c).unwrap()
        );
    }
    assert!(
        file.get_string()
            .contains("# Project compose file — keep me!")
    );
}

// ── YamlValue (in-memory, no filesystem) ──────────────────────────────────────

#[test]
fn value_set_applies_change() {
    let mut doc = YamlValue::parse("# top\nname: web   # svc\nreplicas: 1\n").unwrap();
    let mut v = doc.get().clone();
    v["replicas"] = 3.into();
    doc.set(v);
    let out = doc.get_string();
    assert!(out.contains("replicas: 3"));
    assert!(out.contains("# top"));
    assert!(out.contains("# svc"));
    // The value tracks the edit too.
    assert_eq!(doc.get()["replicas"], Value::from(3));
}

#[test]
fn value_set_replaces_whole_value() {
    let mut doc = YamlValue::parse("a: 1  # one\nb: 2\n").unwrap();
    let mut m = serde_norway::Mapping::new();
    m.insert("a".into(), 1.into()); // unchanged
    m.insert("b".into(), 20.into()); // changed
    doc.set(Value::Mapping(m));
    let out = doc.get_string();
    assert!(out.contains("b: 20"));
    assert!(out.contains("# one")); // comment on untouched `a` preserved
}

#[test]
fn value_get_string_is_lossless_without_edits() {
    let src = "# h\nfoo: bar  # inline\nlist:\n  - 1\n  - 2\n";
    let doc = YamlValue::parse(src).unwrap();
    assert_eq!(doc.get_string(), src);
}

#[test]
fn value_from_str_trait() {
    let doc: YamlValue = "x: 1\n".parse().unwrap();
    assert_eq!(doc.get()["x"], Value::from(1));
}

// ── Tags and nested data-type combinations ────────────────────────────────────

#[test]
fn preserves_yaml_tags_when_editing_siblings() {
    // A custom tag on one value; editing another key must keep the tag intact
    // and value-correct.
    let src = "secret: !vault \"abc123\"\nname: web\n";
    let mut doc = YamlValue::parse(src).unwrap();
    let original_secret = doc.get()["secret"].clone();
    assert!(matches!(original_secret, Value::Tagged(_)));

    let mut v = doc.get().clone();
    v["name"] = "api".into();
    doc.set(v);
    let out = doc.get_string();
    assert!(out.contains("!vault"), "tag must survive: {out}");
    assert!(out.contains("api"));
    // Round-trips: the tagged node is still present and equal.
    assert_eq!(as_value(&out)["secret"], original_secret);
}

#[test]
fn replacing_a_tagged_value_is_value_correct() {
    let src = "node: !Color { r: 1, g: 2, b: 3 }\n";
    let mut doc = YamlValue::parse(src).unwrap();
    // Build a new tagged value and set it.
    let new_val: Value = serde_norway::from_str("!Color { r: 9, g: 9, b: 9 }").unwrap();
    let mut v = doc.get().clone();
    v["node"] = new_val.clone();
    doc.set(v);
    assert_eq!(doc.get()["node"], new_val);
    assert_eq!(as_value(&doc.get_string())["node"], new_val);
}

#[test]
fn deeply_nested_maps_and_sequences_round_trip() {
    let src = "\
root:
  # a list of records
  records:
    - id: 1
      tags: [a, b]
      meta:
        owner: alice
    - id: 2
      tags: [c]
      meta:
        owner: bob
";
    let mut doc = YamlValue::parse(src).unwrap();
    // Reach deep into root.records[0].meta.owner and change it.
    let mut v = doc.get().clone();
    v["root"]["records"][0]["meta"]["owner"] = "carol".into();
    doc.set(v);
    let v = as_value(&doc.get_string());
    assert_eq!(
        v["root"]["records"][0]["meta"]["owner"],
        Value::from("carol")
    );
    // Untouched neighbour preserved.
    assert_eq!(v["root"]["records"][1]["meta"]["owner"], Value::from("bob"));
}

#[test]
fn core_scalar_types_round_trip() {
    let src = "s: hello\ni: -7\nu: 42\nf: 1.5\nb: true\nn: null\n";
    let mut doc = YamlValue::parse(src).unwrap();
    let mut e = doc.get().clone();
    e["i"] = (-8).into();
    e["f"] = 2.5.into();
    e["b"] = false.into();
    doc.set(e);
    let v = as_value(&doc.get_string());
    assert_eq!(v["s"], Value::from("hello"));
    assert_eq!(v["i"], Value::from(-8));
    assert_eq!(v["u"], Value::from(42));
    assert_eq!(v["f"], Value::from(2.5));
    assert_eq!(v["b"], Value::from(false));
    assert!(v["n"].is_null());
}

#[test]
fn btreemap_root_with_comments() {
    let src = "# counts\na: 1\nb: 2  # two\n";
    let (_d, path) = write_temp(src);
    let mut file: YamlFile<BTreeMap<String, i64>> = YamlFile::open(&path).unwrap();
    let mut m = file.get().clone();
    m.insert("c".to_string(), 3);
    *m.get_mut("a").unwrap() = 10;
    file.set(m.clone()).unwrap();

    let text = file.get_string();
    assert!(text.contains("a: 10"));
    assert!(text.contains("c: 3"));
    assert!(text.contains("# counts"));
    assert!(text.contains("# two"));
    let rt: BTreeMap<String, i64> = serde_norway::from_str(text).unwrap();
    assert_eq!(rt, m);
}

// ── YamlObject<T> (typed, no I/O) ─────────────────────────────────────────────

#[test]
fn yaml_object_get_set_typed_preserves_comments() {
    let mut s: YamlObject<Compose> = YamlObject::parse(COMPOSE).unwrap();

    // get() borrows the typed value.
    let mut c = s.get().clone();
    assert_eq!(c.version.as_deref(), Some("3.9"));

    c.version = Some("3.10".to_string());
    c.services.get_mut("web").unwrap().restart = Some("always".to_string());
    s.set(c.clone()).unwrap();

    // get_string() reflects the edit while keeping comments, and the value/text
    // stay in sync.
    let text = s.get_string();
    assert!(text.contains("version: \"3.10\"") || text.contains("version: '3.10'"));
    assert!(text.contains("restart: always"));
    assert!(text.contains("# Project compose file — keep me!"));
    assert!(text.contains("# pin the version"));
    assert_eq!(s.get(), &c);
    assert_eq!(
        as_value(s.get_string()),
        serde_norway::to_value(&c).unwrap()
    );
}

#[test]
fn yaml_object_get_string_and_set_string() {
    let mut s: YamlObject<Compose> = YamlObject::parse(COMPOSE).unwrap();
    // get_string() round-trips the original text (no edits).
    assert_eq!(s.get_string(), COMPOSE);

    // set_string() replaces the whole document from raw YAML, reparsing the
    // typed value and keeping the new text's comments.
    let replacement = "# fresh\nversion: \"4.0\"\nservices: {}\n";
    s.set_string(replacement).unwrap();
    assert_eq!(s.get().version.as_deref(), Some("4.0"));
    assert!(s.get().services.is_empty());
    assert!(s.get_string().contains("# fresh"));
    assert_eq!(s.get_string(), replacement);

    // Invalid YAML is rejected without disturbing the current contents.
    assert!(s.set_string("key: : : nope").is_err());
    assert_eq!(s.get().version.as_deref(), Some("4.0"));
}

#[test]
fn yaml_object_backs_yaml_file() {
    // YamlFile is a YamlObject + path: its text and typed value match the file
    // contents once opened.
    let (_dir, path) = write_temp(COMPOSE);
    let file: YamlFile<Compose> = YamlFile::open(&path).unwrap();
    assert_eq!(file.get_string(), COMPOSE);
    assert_eq!(file.get().version.as_deref(), Some("3.9"));
}
