use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value as JsonValue};

use super_yaml::{
    compile_document, compile_document_from_path, validate_document_from_path, MapEnvProvider,
};

fn read_fixture(path: &str) -> String {
    fs::read_to_string(Path::new(path)).unwrap()
}

fn env_provider(vars: &[(&str, &str)]) -> MapEnvProvider {
    let mut map = HashMap::new();
    for (k, v) in vars {
        map.insert((*k).to_string(), (*v).to_string());
    }
    MapEnvProvider::new(map)
}

#[test]
fn all_examples_compile_to_expected_json() {
    let cases = [
        "basic",
        "service_scaling",
        "pricing_engine",
        "inventory_policy",
        "alert_rules",
        "type_composition",
        "imported_types",
        "typed_dict",
        "color_constructors",
        "vm_resource",
        "template_service",
    ];

    for case in cases {
        let path = format!("examples/{case}.syaml");
        let expected_raw = read_fixture(&format!("examples/{case}.expected.json"));
        let expected: JsonValue = serde_json::from_str(&expected_raw).unwrap();

        let compiled = compile_document_from_path(Path::new(&path), &env_provider(&[])).unwrap();
        assert_eq!(&compiled.value, &expected, "compiled mismatch for {case}");

        let json_out = compiled.to_json_string(true).unwrap();
        let rendered: JsonValue = serde_json::from_str(&json_out).unwrap();
        assert_eq!(rendered, expected, "json output mismatch for {case}");
    }
}

#[test]
fn all_examples_validate_successfully() {
    let cases = [
        "basic",
        "service_scaling",
        "pricing_engine",
        "inventory_policy",
        "alert_rules",
        "type_composition",
        "imported_types",
        "typed_dict",
        "color_constructors",
        "vm_resource",
        "template_service",
    ];

    for case in cases {
        validate_document_from_path(format!("examples/{case}.syaml"), &env_provider(&[])).unwrap();
    }
}

#[test]
fn env_overrides_change_compiled_values() {
    let basic = read_fixture("examples/basic.syaml");
    let compiled = compile_document(
        &basic,
        &env_provider(&[("DB_HOST", "db.internal"), ("CPU_CORES", "16")]),
    )
    .unwrap();

    assert_eq!(compiled.value["host"], json!("db.internal"));
    assert_eq!(compiled.value["worker_threads"], json!(32));
    assert_eq!(compiled.value["max_connections"], json!(2400));

    let scaling = read_fixture("examples/service_scaling.syaml");
    let compiled = compile_document(
        &scaling,
        &env_provider(&[
            ("REGION", "eu-west-1"),
            ("CPU_CORES", "2"),
            ("BASE_PORT", "9100"),
        ]),
    )
    .unwrap();

    assert_eq!(compiled.value["region"], json!("eu-west-1"));
    assert_eq!(compiled.value["worker_threads"], json!(4));
    assert_eq!(compiled.value["grpc_port"], json!(9100));
    assert_eq!(compiled.value["http_port"], json!(9101));
    assert_eq!(
        compiled.value["public_url"],
        json!("https://eu-west-1.example.internal:9101")
    );
}
