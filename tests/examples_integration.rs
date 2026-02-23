use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value as JsonValue};

use super_yaml::{
    compile_document, compile_document_from_path, from_json_schema_path,
    validate_document_from_path, MapEnvProvider,
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
        "type_extension",
        "imported_types",
        "typed_dict",
        "color_constructors",
        "vm_resource",
        "template_service",
        "rest_api",
        "versioned_fields",
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
        "type_extension",
        "imported_types",
        "typed_dict",
        "color_constructors",
        "vm_resource",
        "template_service",
        "rest_api",
        "versioned_fields",
    ];

    for case in cases {
        validate_document_from_path(format!("examples/{case}.syaml"), &env_provider(&[])).unwrap();
    }
}

#[test]
fn generate_from_examples_match_expected() {
    // Walk examples/generate-from/<source-type>/ directories.
    // Each subdirectory name determines the conversion to apply.
    // Convention:
    //   - json-schema/  → from_json_schema_path(<name>.json) vs <name>.expected.syaml
    //
    // To add a new source type, create a new subdirectory and add a match arm below.

    let generate_from = Path::new("examples/generate-from");
    let mut entries: Vec<_> = fs::read_dir(generate_from)
        .expect("examples/generate-from directory not found")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    assert!(
        !entries.is_empty(),
        "examples/generate-from has no subdirectories"
    );

    for dir_entry in entries {
        let source_type = dir_entry.file_name();
        let source_type = source_type.to_string_lossy();
        let dir = dir_entry.path();

        // Collect input files for this source type.
        let mut inputs: Vec<_> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                !name.contains(".expected.")
            })
            .collect();
        inputs.sort_by_key(|e| e.file_name());

        for input_entry in inputs {
            let input_path = input_entry.path();
            let stem = input_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let actual = match source_type.as_ref() {
                "json-schema" => {
                    assert_eq!(
                        input_path.extension().and_then(|e| e.to_str()),
                        Some("json"),
                        "json-schema inputs must be .json files, got: {}",
                        input_path.display()
                    );
                    from_json_schema_path(&input_path).unwrap_or_else(|e| {
                        panic!(
                            "from_json_schema_path failed for {}: {e}",
                            input_path.display()
                        )
                    })
                }
                other => panic!(
                    "unknown generate-from source type '{other}'; add a match arm in the test"
                ),
            };

            let expected_path = dir.join(format!("{stem}.expected.syaml"));
            let expected = fs::read_to_string(&expected_path).unwrap_or_else(|_| {
                panic!(
                    "missing expected file: {} (run the conversion and save the output there)",
                    expected_path.display()
                )
            });

            assert_eq!(
                actual, expected,
                "generate-from/{source_type}/{stem}: output does not match {}",
                expected_path.display()
            );
        }
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
