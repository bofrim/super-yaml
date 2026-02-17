use std::collections::{BTreeMap, HashMap};

use serde_json::json;

use super_yaml::ast::{EnvBinding, FrontMatter};
use super_yaml::resolve::{get_json_path, resolve_env_bindings, resolve_expressions, MapEnvProvider};

fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
    let mut map = HashMap::new();
    for (k, v) in vars {
        map.insert((*k).to_string(), (*v).to_string());
    }
    MapEnvProvider::new(map)
}

#[test]
fn resolve_env_bindings_uses_env_defaults_and_null_for_optional() {
    let mut env_defs = BTreeMap::new();
    env_defs.insert(
        "NUM".to_string(),
        EnvBinding {
            key: "NUM_KEY".to_string(),
            required: true,
            default: None,
        },
    );
    env_defs.insert(
        "FLAG".to_string(),
        EnvBinding {
            key: "FLAG_KEY".to_string(),
            required: true,
            default: None,
        },
    );
    env_defs.insert(
        "WITH_DEFAULT".to_string(),
        EnvBinding {
            key: "MISSING".to_string(),
            required: true,
            default: Some(json!("fallback")),
        },
    );
    env_defs.insert(
        "OPTIONAL".to_string(),
        EnvBinding {
            key: "NOPE".to_string(),
            required: false,
            default: None,
        },
    );

    let front_matter = FrontMatter { env: env_defs };
    let resolved = resolve_env_bindings(
        Some(&front_matter),
        &env_provider(&[("NUM_KEY", "42"), ("FLAG_KEY", "true")]),
    )
    .unwrap();

    assert_eq!(resolved["NUM"], json!(42));
    assert_eq!(resolved["FLAG"], json!(true));
    assert_eq!(resolved["WITH_DEFAULT"], json!("fallback"));
    assert_eq!(resolved["OPTIONAL"], json!(null));
}

#[test]
fn resolve_env_bindings_errors_for_missing_required_without_default() {
    let mut env_defs = BTreeMap::new();
    env_defs.insert(
        "DB".to_string(),
        EnvBinding {
            key: "DB_HOST".to_string(),
            required: true,
            default: None,
        },
    );

    let front_matter = FrontMatter { env: env_defs };
    let err = resolve_env_bindings(Some(&front_matter), &env_provider(&[])).unwrap_err();
    assert!(err.to_string().contains("missing required environment variable"));
}

#[test]
fn resolve_env_bindings_without_front_matter_returns_empty() {
    let resolved = resolve_env_bindings(None, &env_provider(&[])).unwrap();
    assert!(resolved.is_empty());
}

#[test]
fn resolve_expressions_handles_expression_interpolation_and_arrays() {
    let mut data = json!({
        "base": 5,
        "expr": "=base * 2",
        "inline": "value=${expr}",
        "full_interp": "${expr}",
        "arr": ["=base + 1", "${base}"]
    });
    let env = BTreeMap::new();

    resolve_expressions(&mut data, &env).unwrap();

    assert_eq!(data["expr"], json!(10));
    assert_eq!(data["inline"], json!("value=10"));
    assert_eq!(data["full_interp"], json!(10));
    assert_eq!(data["arr"][0], json!(6));
    assert_eq!(data["arr"][1], json!(5));
}

#[test]
fn resolve_expressions_detects_cycles() {
    let mut data = json!({
        "a": "=b + 1",
        "b": "=a + 1"
    });
    let env = BTreeMap::new();

    let err = resolve_expressions(&mut data, &env).unwrap_err();
    assert!(err.to_string().contains("possible dependency cycle"));
}

#[test]
fn resolve_expressions_reports_unknown_reference() {
    let mut data = json!({
        "a": "=missing + 1"
    });
    let env = BTreeMap::new();

    let err = resolve_expressions(&mut data, &env).unwrap_err();
    assert!(err.to_string().contains("unknown reference 'missing'"));
}

#[test]
fn resolve_expressions_supports_env_values() {
    let mut data = json!({
        "threads": "=env.CPU * 2",
        "msg": "cpu=${env.CPU}"
    });

    let mut env = BTreeMap::new();
    env.insert("CPU".to_string(), json!(6));

    resolve_expressions(&mut data, &env).unwrap();

    assert_eq!(data["threads"], json!(12));
    assert_eq!(data["msg"], json!("cpu=6"));
}

#[test]
fn get_json_path_supports_root_object_and_arrays() {
    let data = json!({
        "name": "svc",
        "items": [
            {"id": 1},
            {"id": 2}
        ]
    });

    assert_eq!(get_json_path(&data, "$").unwrap(), &data);
    assert_eq!(get_json_path(&data, "$.name").unwrap(), &json!("svc"));
    assert_eq!(get_json_path(&data, "$.items[1].id").unwrap(), &json!(2));
    assert!(get_json_path(&data, "$.items[5]").is_none());
    assert!(get_json_path(&data, "name").is_none());
}

#[test]
fn resolve_expressions_updates_nested_array_paths() {
    let mut data = json!({
        "nums": [1, 2],
        "nested": [
            {"v": "=nums[0] + nums[1]"}
        ]
    });
    let env = BTreeMap::new();

    let err = resolve_expressions(&mut data, &env).unwrap_err();
    assert!(err.to_string().contains("unexpected character '['"));
}

#[test]
fn resolve_expressions_leaves_plain_strings_untouched() {
    let mut data = json!({
        "a": "hello",
        "b": "${not_closed"
    });
    let env = BTreeMap::new();

    resolve_expressions(&mut data, &env).unwrap();

    assert_eq!(data["a"], json!("hello"));
    assert_eq!(data["b"], json!("${not_closed"));
}

#[test]
fn resolve_expressions_supports_multi_pass_dependencies() {
    let mut data = json!({
        "a": 2,
        "b": "=a + 1",
        "c": "=b + 1",
        "d": "=c + 1"
    });
    let env = BTreeMap::new();

    resolve_expressions(&mut data, &env).unwrap();

    assert_eq!(data["b"], json!(3));
    assert_eq!(data["c"], json!(4));
    assert_eq!(data["d"], json!(5));
}
