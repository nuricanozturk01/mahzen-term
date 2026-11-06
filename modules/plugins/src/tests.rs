use super::*;

/// A plugin written in WebAssembly text: answers a fixed card.
const HELLO: &str = r#"
(module
  (import "mz" "now_ms" (func $now (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 1024) "title=Hello\nbadge=v1\nrow=good|status|ok\nrow=warn|load|high\nrow=weird|x|y")
  (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
  (func (export "mz_render") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const 72))))
"#;

fn write_plugin(root: &Path, name: &str, wat_text: &str, manifest_extra: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!("name = \"{name}\"\ntitle = \"Hello\"\n{manifest_extra}"),
    )
    .unwrap();
    std::fs::write(dir.join("plugin.wasm"), wat::parse_str(wat_text).unwrap()).unwrap();
    dir
}

#[test]
fn a_plugin_draws_a_card() {
    let root = tempfile::tempdir().unwrap();
    let dir = write_plugin(root.path(), "hello", HELLO, "");
    let p = Plugin::load(&dir).unwrap();
    let card = p.render(40).unwrap();
    assert_eq!((card.title.as_str(), card.badge.as_str()), ("Hello", "v1"));
    assert_eq!(card.rows.len(), 3);
    assert_eq!(
        (card.rows[0].label.as_str(), card.rows[0].tone),
        ("status", InfoTone::Good)
    );
    assert_eq!(
        card.rows[2].tone,
        InfoTone::Normal,
        "unknown tones fall back to normal"
    );
    // Calling again reuses the instance.
    assert!(p.render(40).is_ok());
}

#[test]
fn only_allowed_plugins_are_loaded() {
    let root = tempfile::tempdir().unwrap();
    write_plugin(root.path(), "hello", HELLO, "");
    write_plugin(root.path(), "other", HELLO, "");
    let none = scan(root.path(), &Settings::default());
    assert!(none.loaded.is_empty());
    assert_eq!(none.not_allowed, ["hello", "other"]);
    let some = scan(
        root.path(),
        &Settings {
            allow: vec!["hello".into(), "missing".into(), "../x".into()],
        },
    );
    assert_eq!(some.loaded.len(), 1);
    assert_eq!(some.not_allowed, ["other"]);
    assert_eq!(some.problems.len(), 2);
}

#[test]
fn bad_manifests_are_refused_with_a_reason() {
    let root = tempfile::tempdir().unwrap();
    let d = write_plugin(root.path(), "caps", HELLO, "capabilities = [\"net\"]\n");
    assert!(Plugin::load(&d).unwrap_err().0.contains("capabilities"));
    let d = write_plugin(root.path(), "path", HELLO, "wasm = \"../evil.wasm\"\n");
    assert!(Plugin::load(&d).unwrap_err().0.contains("plain .wasm"));
    let d = write_plugin(root.path(), "extra", HELLO, "bogus = 1\n");
    assert!(
        Plugin::load(&d).is_err(),
        "unknown manifest fields are errors"
    );
    let d = write_plugin(root.path(), "hello", HELLO, "");
    std::fs::rename(&d, root.path().join("renamed")).unwrap();
    assert!(Plugin::load(&root.path().join("renamed"))
        .unwrap_err()
        .0
        .contains("folder name"));
    let bad = root.path().join("junk");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("plugin.toml"), "name = \"junk\"\n").unwrap();
    std::fs::write(bad.join("plugin.wasm"), b"not wasm").unwrap();
    assert!(Plugin::load(&bad).unwrap_err().0.contains("valid wasm"));
}

#[test]
fn an_endless_loop_runs_out_of_fuel_and_the_plugin_recovers() {
    let spin = r#"
    (module
      (memory (export "memory") 1)
      (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "mz_render") (param i32 i32) (result i64) (loop $l (br $l)) (i64.const 0)))
    "#;
    let root = tempfile::tempdir().unwrap();
    let p = Plugin::load(&write_plugin(root.path(), "spin", spin, "")).unwrap();
    let e = p.render(10).unwrap_err();
    assert!(e.0.contains("mz_render"), "{e}");
    assert!(p.render(10).is_err(), "still bounded on the next call");
}

#[test]
fn memory_hogs_and_oversized_answers_are_stopped() {
    let hog = r#"
    (module
      (memory (export "memory") 1)
      (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "mz_render") (param i32 i32) (result i64)
        (drop (memory.grow (i32.const 100000)))
        (i64.const 0)))
    "#;
    let root = tempfile::tempdir().unwrap();
    // memory.grow beyond the limit just fails (-1); the call still ends normally with an empty card.
    let p = Plugin::load(&write_plugin(root.path(), "hog", hog, "")).unwrap();
    assert_eq!(p.render(10).unwrap().empty, "nothing to show");
    let big = r#"
    (module
      (memory (export "memory") 2)
      (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "mz_render") (param i32 i32) (result i64) (i64.const 70000)))
    "#;
    let p = Plugin::load(&write_plugin(root.path(), "big", big, "")).unwrap();
    assert!(p.render(10).unwrap_err().0.contains("16 KiB"));
    let wild = r#"
    (module
      (memory (export "memory") 1)
      (func (export "mz_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "mz_render") (param i32 i32) (result i64) (i64.or (i64.shl (i64.const 70000) (i64.const 32)) (i64.const 100))))
    "#;
    let p = Plugin::load(&write_plugin(root.path(), "wild", wild, "")).unwrap();
    assert!(p.render(10).unwrap_err().0.contains("bad pointer"));
}

#[test]
fn the_answer_text_is_parsed_defensively() {
    let c = parse_card(
        "title=T\u{7}x\nrow=good|a|b\nrow=bad|only-two\nnoise\nrow=dim|l|v|with|pipes",
        "d",
    );
    assert_eq!(c.title, "Tx");
    assert_eq!(c.rows.len(), 2);
    assert_eq!(c.rows[1].value, "v|with|pipes");
    let many: String = (0..40).map(|i| format!("row=good|{i}|x\n")).collect();
    assert_eq!(parse_card(&many, "d").rows.len(), MAX_ROWS);
    assert_eq!(parse_card("", "d").empty, "nothing to show");
}

#[test]
fn module_is_valid() {
    mahzenterm_core::module::validate(&PluginsModule).unwrap();
}

#[test]
fn the_rust_sample_plugin_works_when_it_has_been_built() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/clock");
    let wasm = src.join("target/wasm32-unknown-unknown/release/clock.wasm");
    let Ok(bytes) = std::fs::read(&wasm) else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("clock");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(src.join("plugin.toml"), dir.join("plugin.toml")).unwrap();
    std::fs::write(dir.join("plugin.wasm"), bytes).unwrap();
    let card = Plugin::load(&dir).unwrap().render(40).unwrap();
    assert_eq!((card.title.as_str(), card.badge.as_str()), ("Clock", "UTC"));
    assert_eq!(card.rows[0].label, "time");
    assert_eq!(card.rows[0].value.len(), 8, "HH:MM:SS");
}
