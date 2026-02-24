use std::collections::BTreeMap;

use serde_json::json;

use super_yaml::schema::{
    parse_field_version_meta, parse_schema, resolve_type_schema, validate_json_against_schema,
    validate_json_against_schema_with_types, validate_schema_type_references,
};
use super_yaml::validate::{
    build_effective_constraints, validate_constraints, validate_type_hints,
};
use super_yaml::{compile_document, MapEnvProvider};

#[test]
fn parse_schema_treats_top_level_keys_as_types() {
    let raw = json!({
        "Port": { "type": "integer", "minimum": 1 },
        "Replicas": { "type": "integer", "minimum": 1 }
    });

    let schema = parse_schema(&raw).unwrap();
    assert!(schema.types.contains_key("Port"));
    assert!(schema.types.contains_key("Replicas"));
}

#[test]
fn parse_schema_collects_type_local_constraints() {
    let raw = json!({
        "types": {
            "SessionConfig": {
                "type": "object",
                "properties": {
                    "min_attendees": {
                        "type": "integer",
                        "constraints": ["value >= 1", "value <= 1000000"]
                    },
                    "max_attendees": {
                        "type": "integer",
                        "constraints": "value >= 1"
                    }
                },
                "constraints": ["min_attendees <= max_attendees"]
            }
        }
    });

    let schema = parse_schema(&raw).unwrap();
    let by_type = schema.type_constraints.get("SessionConfig").unwrap();
    assert_eq!(
        by_type.get("$.min_attendees").unwrap(),
        &vec!["value >= 1".to_string(), "value <= 1000000".to_string()]
    );
    assert_eq!(
        by_type.get("$.max_attendees").unwrap(),
        &vec!["value >= 1".to_string()]
    );
    assert_eq!(
        by_type.get("$").unwrap(),
        &vec!["min_attendees <= max_attendees".to_string()]
    );
}

#[test]
fn parse_schema_collects_type_local_constraint_path_map() {
    let raw = json!({
        "types": {
            "SessionConfig": {
                "type": "object",
                "properties": {
                    "min_attendees": { "type": "integer" },
                    "max_attendees": { "type": "integer" }
                },
                "constraints": {
                    "min_attendees": [
                        "value >= 1",
                        "value <= 1000000"
                    ],
                    "max_attendees": "value >= 1"
                }
            }
        }
    });

    let schema = parse_schema(&raw).unwrap();
    let by_type = schema.type_constraints.get("SessionConfig").unwrap();
    assert_eq!(
        by_type.get("$.min_attendees").unwrap(),
        &vec!["value >= 1".to_string(), "value <= 1000000".to_string()]
    );
    assert_eq!(
        by_type.get("$.max_attendees").unwrap(),
        &vec!["value >= 1".to_string()]
    );
}

#[test]
fn parse_schema_normalizes_string_type_shorthand_for_nested_nodes() {
    let schema = parse_schema(&json!({
        "types": {
            "BoundsConfig": {
                "type": "object",
                "properties": {
                    "x_min": "number",
                    "x_max": "number?",
                    "y_min": { "type": "number?" },
                    "tags": {
                        "type": "array",
                        "items": "string"
                    }
                }
            }
        }
    }))
    .unwrap();

    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["x_min"]["type"],
        json!("number")
    );
    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["x_max"]["type"],
        json!("number")
    );
    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["x_max"]["optional"],
        json!(true)
    );
    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["y_min"]["type"],
        json!("number")
    );
    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["y_min"]["optional"],
        json!(true)
    );
    assert_eq!(
        schema.types["BoundsConfig"]["properties"]["tags"]["items"]["type"],
        json!("string")
    );

    validate_json_against_schema_with_types(
        &json!({"x_min": 1.5, "tags": ["a", "b"]}),
        schema.types.get("BoundsConfig").unwrap(),
        "$.bounds",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn parse_schema_normalizes_string_type_shorthand_for_object_values() {
    let schema = parse_schema(&json!({
        "types": {
            "ServiceInstances": {
                "type": "object",
                "values": "integer"
            }
        }
    }))
    .unwrap();

    assert_eq!(
        schema.types["ServiceInstances"]["values"]["type"],
        json!("integer")
    );

    validate_json_against_schema_with_types(
        &json!({"api": 3, "worker": 5}),
        schema.types.get("ServiceInstances").unwrap(),
        "$.instances",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn parse_schema_normalizes_string_enum_shorthand_for_nested_nodes() {
    let schema = parse_schema(&json!({
        "types": {
            "DerivedMetricSpec": {
                "type": "object",
                "properties": {
                    "operator": ["ema", "derivative", "rolling_mean", "rolling_var", "rolling_min", "rolling_max"]
                }
            }
        }
    }))
    .unwrap();

    assert_eq!(
        schema.types["DerivedMetricSpec"]["properties"]["operator"]["type"],
        json!("string")
    );
    assert_eq!(
        schema.types["DerivedMetricSpec"]["properties"]["operator"]["enum"],
        json!([
            "ema",
            "derivative",
            "rolling_mean",
            "rolling_var",
            "rolling_min",
            "rolling_max"
        ])
    );

    validate_json_against_schema_with_types(
        &json!({"operator": "ema"}),
        schema.types.get("DerivedMetricSpec").unwrap(),
        "$.spec",
        &schema.types,
    )
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!({"operator": "not_supported"}),
        schema.types.get("DerivedMetricSpec").unwrap(),
        "$.spec",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("enum mismatch"));
}

#[test]
fn validate_type_hints_accepts_shorthand_property_type_reference() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 },
            "Service": {
                "type": "object",
                "properties": {
                    "port": "Port"
                }
            }
        }
    }))
    .unwrap();

    let data = json!({"service": {"port": 8080}});
    let mut hints = BTreeMap::new();
    hints.insert("$.service".to_string(), "Service".to_string());
    hints.insert("$.service.port".to_string(), "Port".to_string());

    validate_type_hints(&data, &hints, &schema).unwrap();
}

#[test]
fn parse_schema_rejects_non_string_constraint_entries() {
    let raw = json!({
        "types": {
            "Port": {
                "type": "integer",
                "constraints": [true]
            }
        }
    });

    let err = parse_schema(&raw).unwrap_err();
    assert!(err.to_string().contains("entries must be strings"));
}

#[test]
fn parse_schema_rejects_constraint_reference_outside_type_scope() {
    let raw = json!({
        "types": {
            "RequestedSeats": {
                "type": "integer",
                "constraints": ["value <= seat_limit"]
            }
        }
    });

    let err = parse_schema(&raw).unwrap_err();
    assert!(err
        .to_string()
        .contains("outside the constrained type scope"));
    assert!(err.to_string().contains("seat_limit"));
}

#[test]
fn resolve_type_schema_supports_custom_and_builtin_types() {
    let raw = json!({
        "types": {
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 }
        }
    });
    let schema = parse_schema(&raw).unwrap();

    let custom = resolve_type_schema(&schema, "Port").unwrap();
    assert_eq!(custom["type"], json!("integer"));

    let builtin = resolve_type_schema(&schema, "string").unwrap();
    assert_eq!(builtin, json!({"type": "string"}));

    let err = resolve_type_schema(&schema, "MissingType").unwrap_err();
    assert!(err.to_string().contains("unknown type 'MissingType'"));
}

#[test]
fn validate_json_against_schema_supports_nested_object_array_rules() {
    let value = json!({
        "name": "service",
        "count": 3,
        "tags": ["a", "b"]
    });
    let schema = json!({
        "type": "object",
        "required": ["name", "count"],
        "properties": {
            "name": { "type": "string", "minLength": 3, "pattern": "^[a-z]+$" },
            "count": { "type": "integer", "minimum": 1, "maximum": 10 },
            "tags": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string" }
            }
        }
    });

    validate_json_against_schema(&value, &schema, "$").unwrap();
}

#[test]
fn validate_json_against_schema_catches_numeric_and_string_violations() {
    let err = validate_json_against_schema(
        &json!(1),
        &json!({"type": "integer", "exclusiveMinimum": 1}),
        "$.x",
    )
    .unwrap_err();
    assert!(err.to_string().contains("exclusiveMinimum violation"));

    let err = validate_json_against_schema(
        &json!("ABC"),
        &json!({"type": "string", "pattern": "^[a-z]+$"}),
        "$.name",
    )
    .unwrap_err();
    assert!(err.to_string().contains("pattern violation"));

    let err = validate_json_against_schema(
        &json!("abc"),
        &json!({"type": "string", "maxLength": 2}),
        "$.name",
    )
    .unwrap_err();
    assert!(err.to_string().contains("maxLength violation"));
}

#[test]
fn validate_json_against_schema_catches_object_array_enum_violations() {
    let err = validate_json_against_schema(
        &json!({"name": "svc"}),
        &json!({
            "type": "object",
            "required": ["name", "port"]
        }),
        "$",
    )
    .unwrap_err();
    assert!(err.to_string().contains("required property missing"));

    let err = validate_json_against_schema(
        &json!([]),
        &json!({"type": "array", "minItems": 1}),
        "$.arr",
    )
    .unwrap_err();
    assert!(err.to_string().contains("minItems violation"));

    let err = validate_json_against_schema(
        &json!("dev"),
        &json!({"enum": ["prod", "staging"]}),
        "$.env",
    )
    .unwrap_err();
    assert!(err.to_string().contains("enum mismatch"));
}

#[test]
fn validate_json_against_schema_reports_invalid_pattern() {
    let err = validate_json_against_schema(
        &json!("abc"),
        &json!({"type": "string", "pattern": "[a-z"}),
        "$.name",
    )
    .unwrap_err();
    assert!(err.to_string().contains("invalid pattern"));
}

#[test]
fn validate_type_hints_success_and_failures() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 }
        }
    }))
    .unwrap();

    let data = json!({"port": 8080, "name": "svc"});
    let mut hints = BTreeMap::new();
    hints.insert("$.port".to_string(), "Port".to_string());
    hints.insert("$.name".to_string(), "string".to_string());

    validate_type_hints(&data, &hints, &schema).unwrap();

    let mut missing_path_hints = BTreeMap::new();
    missing_path_hints.insert("$.missing".to_string(), "string".to_string());
    let err = validate_type_hints(&data, &missing_path_hints, &schema).unwrap_err();
    assert!(err.to_string().contains("missing path"));

    let mut unknown_type_hints = BTreeMap::new();
    unknown_type_hints.insert("$.name".to_string(), "Nope".to_string());
    let err = validate_type_hints(&data, &unknown_type_hints, &schema).unwrap_err();
    assert!(err.to_string().contains("unknown type"));
}

#[test]
fn validate_constraints_supports_paths_env_and_failures() {
    let data = json!({
        "replicas": 3,
        "workers": 6,
        "env_name": "prod"
    });
    let mut env = BTreeMap::new();
    env.insert("EXPECTED_ENV".to_string(), json!("prod"));

    let mut constraints = BTreeMap::new();
    constraints.insert("replicas".to_string(), vec!["value >= 1".to_string()]);
    constraints.insert(
        "$.workers".to_string(),
        vec!["value == replicas * 2".to_string()],
    );
    constraints.insert(
        "env_name".to_string(),
        vec!["value == env.EXPECTED_ENV".to_string()],
    );

    validate_constraints(&data, &env, &constraints).unwrap();

    let mut bad_constraints = BTreeMap::new();
    bad_constraints.insert("replicas".to_string(), vec!["value > 10".to_string()]);
    let err = validate_constraints(&data, &env, &bad_constraints).unwrap_err();
    assert!(err.to_string().contains("constraint failed"));

    let mut non_bool = BTreeMap::new();
    non_bool.insert("replicas".to_string(), vec!["value + 1".to_string()]);
    let err = validate_constraints(&data, &env, &non_bool).unwrap_err();
    assert!(err.to_string().contains("must evaluate to boolean"));

    let mut missing_path = BTreeMap::new();
    missing_path.insert("$.missing".to_string(), vec!["value == 1".to_string()]);
    let err = validate_constraints(&data, &env, &missing_path).unwrap_err();
    assert!(err.to_string().contains("path '$.missing' not found"));
}

#[test]
fn validate_constraints_rejects_impossible_variable_ordering() {
    let data = json!({
        "window": { "min": 1, "max": 5 }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "window".to_string(),
        vec![
            "value.min < value.max".to_string(),
            "value.min > value.max".to_string(),
        ],
    );

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("impossible constraints"));
    assert!(err.to_string().contains("value.min < value.max"));
    assert!(err.to_string().contains("value.min > value.max"));
}

#[test]
fn validate_constraints_rejects_impossible_ordering_inside_single_and_expression() {
    let data = json!({
        "window": { "min": 1, "max": 5 }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "window".to_string(),
        vec!["value.min < value.max && value.min > value.max".to_string()],
    );

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("impossible constraints"));
}

#[test]
fn validate_constraints_allows_consistent_variable_ordering() {
    let data = json!({
        "window": { "min": 1, "max": 5 }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "window".to_string(),
        vec![
            "value.min <= value.max".to_string(),
            "value.min != value.max".to_string(),
        ],
    );

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_constraints_rejects_impossible_numeric_range() {
    let data = json!({"count": 7});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "count".to_string(),
        vec!["value < 5".to_string(), "value > 10".to_string()],
    );

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("impossible constraints"));
    assert!(err.to_string().contains("value < 5"));
    assert!(err.to_string().contains("value > 10"));
}

#[test]
fn validate_constraints_rejects_impossible_numeric_range_inside_and_expression() {
    let data = json!({"count": 7});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "count".to_string(),
        vec!["value >= 3 && value < 3".to_string()],
    );

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("impossible constraints"));
}

#[test]
fn validate_constraints_rejects_exact_value_that_is_disallowed() {
    let data = json!({"count": 4});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "count".to_string(),
        vec!["value == 4".to_string(), "value != 4".to_string()],
    );

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("impossible constraints"));
}

#[test]
fn validate_constraints_allows_consistent_numeric_range() {
    let data = json!({"count": 7});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "count".to_string(),
        vec![
            "value >= 5".to_string(),
            "value <= 10".to_string(),
            "value != 8".to_string(),
        ],
    );

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn build_effective_constraints_expands_type_local_paths() {
    let schema = parse_schema(&json!({
        "types": {
            "SessionConfig": {
                "type": "object",
                "properties": {
                    "min_attendees": {
                        "type": "integer",
                        "constraints": "value >= 1"
                    },
                    "max_attendees": {
                        "type": "integer"
                    }
                },
                "constraints": ["min_attendees <= max_attendees"]
            }
        }
    }))
    .unwrap();

    let mut hints = BTreeMap::new();
    hints.insert("$.session".to_string(), "SessionConfig".to_string());

    let effective = build_effective_constraints(&hints, &schema);
    assert_eq!(
        effective.get("$.session.min_attendees").unwrap(),
        &vec!["value >= 1".to_string()]
    );
    assert_eq!(
        effective.get("$.session").unwrap(),
        &vec!["min_attendees <= max_attendees".to_string()]
    );
}

#[test]
fn validate_constraints_reports_unresolved_dependency() {
    let data = json!({"a": 1, "b": 2});
    let env = BTreeMap::new();

    let mut constraints = BTreeMap::new();
    constraints.insert("a".to_string(), vec!["value > c".to_string()]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("unknown reference 'c'"));
}

#[test]
fn validate_constraints_reports_invalid_expression_syntax() {
    let data = json!({"a": 1});
    let env = BTreeMap::new();

    let mut constraints = BTreeMap::new();
    constraints.insert("a".to_string(), vec!["value = 1".to_string()]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("use '==' for equality"));
}

#[test]
fn type_schema_validates_nested_item_type_mismatch() {
    let value = json!([1, "two", 3]);
    let schema = json!({"type": "array", "items": {"type": "integer"}});

    let err = validate_json_against_schema(&value, &schema, "$.arr").unwrap_err();
    assert!(err.to_string().contains("type mismatch"));
}

#[test]
fn validate_json_against_schema_with_types_allows_nested_custom_type_reference() {
    let schema = parse_schema(&json!({
        "types": {
            "PositiveNumber": {
                "type": "number",
                "exclusiveMinimum": 0
            },
            "DisplayConfig": {
                "type": "object",
                "properties": {
                    "scale_factor": {
                        "type": "PositiveNumber"
                    }
                }
            }
        }
    }))
    .unwrap();

    let value = json!({"scale_factor": 10.0});
    let display_schema = schema.types.get("DisplayConfig").unwrap();

    validate_json_against_schema_with_types(
        &value,
        display_schema,
        "$.device.display.monitor",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn validate_json_against_schema_with_types_reports_nested_custom_type_violation() {
    let schema = parse_schema(&json!({
        "types": {
            "PositiveNumber": {
                "type": "number",
                "exclusiveMinimum": 0
            },
            "DisplayConfig": {
                "type": "object",
                "properties": {
                    "scale_factor": {
                        "type": "PositiveNumber"
                    }
                }
            }
        }
    }))
    .unwrap();

    let value = json!({"scale_factor": 0});
    let display_schema = schema.types.get("DisplayConfig").unwrap();

    let err = validate_json_against_schema_with_types(
        &value,
        display_schema,
        "$.device.display.monitor",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("exclusiveMinimum violation"));
}

#[test]
fn validate_json_against_schema_with_types_composes_local_and_referenced_constraints() {
    let schema = parse_schema(&json!({
        "types": {
            "PositiveNumber": {
                "type": "number",
                "exclusiveMinimum": 0
            }
        }
    }))
    .unwrap();

    let composed = json!({
        "type": "PositiveNumber",
        "maximum": 10
    });

    validate_json_against_schema_with_types(&json!(5), &composed, "$.radius", &schema.types)
        .unwrap();

    let err =
        validate_json_against_schema_with_types(&json!(11), &composed, "$.radius", &schema.types)
            .unwrap_err();
    assert!(err.to_string().contains("maximum violation"));
}

#[test]
fn validate_type_hints_reports_unknown_nested_custom_type_reference() {
    let schema = parse_schema(&json!({
        "types": {
            "Container": {
                "type": "object",
                "properties": {
                    "radius": { "type": "MissingType" }
                }
            }
        }
    }))
    .unwrap();

    let data = json!({"container": {"radius": 1}});
    let mut hints = BTreeMap::new();
    hints.insert("$.container".to_string(), "Container".to_string());

    let err = validate_type_hints(&data, &hints, &schema).unwrap_err();
    assert!(err.to_string().contains("unknown type reference"));
    assert!(err.to_string().contains("MissingType"));
}

#[test]
fn validate_type_hints_reports_cyclic_type_references() {
    let schema = parse_schema(&json!({
        "types": {
            "A": { "type": "B" },
            "B": { "type": "A" }
        }
    }))
    .unwrap();

    let data = json!({"x": 1});
    let mut hints = BTreeMap::new();
    hints.insert("$.x".to_string(), "A".to_string());

    let err = validate_type_hints(&data, &hints, &schema).unwrap_err();
    assert!(err.to_string().contains("cyclic type reference"));
    assert!(err.to_string().contains("A -> B -> A"));
}

#[test]
fn validate_type_hints_reports_nested_hint_type_mismatch_against_parent_schema() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 },
            "Service": {
                "type": "object",
                "properties": {
                    "port": { "type": "Port" }
                }
            }
        }
    }))
    .unwrap();

    let data = json!({"service": {"port": 8080}});
    let mut hints = BTreeMap::new();
    hints.insert("$.service".to_string(), "Service".to_string());
    hints.insert("$.service.port".to_string(), "integer".to_string());

    let err = validate_type_hints(&data, &hints, &schema).unwrap_err();
    assert!(err.to_string().contains("type hint mismatch"));
    assert!(err.to_string().contains("$.service.port"));
    assert!(err.to_string().contains("Port"));
}

#[test]
fn validate_type_hints_reports_nested_hint_field_missing_from_parent_schema() {
    let schema = parse_schema(&json!({
        "types": {
            "Service": {
                "type": "object",
                "properties": {
                    "port": { "type": "integer" }
                }
            }
        }
    }))
    .unwrap();

    let data = json!({"service": {"port": 8080, "mode": "prod"}});
    let mut hints = BTreeMap::new();
    hints.insert("$.service".to_string(), "Service".to_string());
    hints.insert("$.service.mode".to_string(), "string".to_string());

    let err = validate_type_hints(&data, &hints, &schema).unwrap_err();
    assert!(err.to_string().contains("not declared in schema"));
    assert!(err.to_string().contains("$.service.mode"));
}

#[test]
fn validate_constraints_allows_prefixed_equals_sign() {
    let data = json!({"replicas": 2});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("replicas".to_string(), vec!["=value >= 1".to_string()]);

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn parse_schema_requires_object_shape() {
    let err = parse_schema(&json!("not-an-object")).unwrap_err();
    assert!(err.to_string().contains("schema must be a mapping/object"));
}

#[test]
fn validate_json_against_schema_requires_object_schema() {
    let err = validate_json_against_schema(&json!(1), &json!(true), "$.x").unwrap_err();
    assert!(err.to_string().contains("must be an object"));
}

#[test]
fn validate_json_against_schema_rejects_non_string_type_keyword() {
    let err = validate_json_against_schema(&json!(1), &json!({"type": 123}), "$.x").unwrap_err();
    assert!(err.to_string().contains("must be a string"));
}

#[test]
fn validate_constraints_uses_current_value_object() {
    let data = json!({
        "window": { "min": 2, "max": 5 }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "window".to_string(),
        vec!["value.max > value.min".to_string()],
    );

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_json_against_schema_checks_exclusive_maximum() {
    let err = validate_json_against_schema(
        &json!(10),
        &json!({"type": "number", "exclusiveMaximum": 10}),
        "$.x",
    )
    .unwrap_err();
    assert!(err.to_string().contains("exclusiveMaximum violation"));
}

#[test]
fn parse_schema_accepts_arbitrary_top_level_type_names() {
    let schema = parse_schema(&json!({"unknown": true})).unwrap();
    assert!(schema.types.contains_key("unknown"));
}

#[test]
fn parse_schema_rejects_bad_types_shape() {
    let err = parse_schema(&json!({"types": true})).unwrap_err();
    assert!(err.to_string().contains("schema.types must be a mapping"));
}

#[test]
fn validate_type_hints_reports_schema_violation() {
    let schema = parse_schema(&json!({
        "types": {
            "TinyInt": { "type": "integer", "maximum": 3 }
        }
    }))
    .unwrap();

    let data = json!({"count": 9});
    let mut hints = BTreeMap::new();
    hints.insert("$.count".to_string(), "TinyInt".to_string());

    let err = validate_type_hints(&data, &hints, &schema).unwrap_err();
    assert!(err.to_string().contains("maximum violation"));
}

#[test]
fn validate_constraints_can_use_nested_paths() {
    let data = json!({
        "inventory": {
            "on_hand": 9,
            "reorder_point": 5
        }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "inventory.on_hand".to_string(),
        vec!["value >= inventory.reorder_point".to_string()],
    );

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_constraints_can_use_parent_scope_for_siblings() {
    let data = json!({
        "session": {
            "min_attendees": 2,
            "max_attendees": 5
        }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "session.min_attendees".to_string(),
        vec!["value <= max_attendees".to_string()],
    );
    constraints.insert(
        "session".to_string(),
        vec!["min_attendees <= max_attendees".to_string()],
    );

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_json_against_schema_requires_properties_by_default() {
    let value = json!({"name": "svc"});
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "port": { "type": "integer" }
        }
    });

    let err = validate_json_against_schema(&value, &schema, "$").unwrap_err();
    assert!(err.to_string().contains("required property missing"));
    assert!(err.to_string().contains("port"));
}

#[test]
fn validate_json_against_schema_allows_missing_optional_properties() {
    let value = json!({"name": "svc"});
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "port": { "type": "integer", "optional": true }
        }
    });

    validate_json_against_schema(&value, &schema, "$").unwrap();
}

#[test]
fn validate_json_against_schema_rejects_invalid_optional_shape() {
    let err = validate_json_against_schema(
        &json!({"name": "svc"}),
        &json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "optional": "yes" }
            }
        }),
        "$",
    )
    .unwrap_err();

    assert!(err.to_string().contains("optional"));
    assert!(err.to_string().contains("must be a boolean"));
}

#[test]
fn validate_json_against_schema_validates_object_values_schema() {
    let schema = json!({
        "type": "object",
        "values": {
            "type": "object",
            "properties": {
                "cpu": { "type": "integer", "minimum": 1 }
            }
        }
    });

    validate_json_against_schema(
        &json!({
            "api": { "cpu": 2 },
            "worker": { "cpu": 4 }
        }),
        &schema,
        "$.services",
    )
    .unwrap();

    let err = validate_json_against_schema(
        &json!({
            "api": { "cpu": 0 }
        }),
        &schema,
        "$.services",
    )
    .unwrap_err();
    assert!(err.to_string().contains("$.services.api.cpu"));
    assert!(err.to_string().contains("minimum violation"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_values_shape() {
    let err = validate_json_against_schema(
        &json!({"a": 1}),
        &json!({
            "type": "object",
            "values": "integer"
        }),
        "$",
    )
    .unwrap_err();
    assert!(err.to_string().contains("values at $ must be an object"));
}

#[test]
fn validate_type_hints_accepts_nested_hints_under_object_values_schema() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": { "type": "integer", "minimum": 1, "maximum": 65535 },
            "ServiceConfig": {
                "type": "object",
                "properties": {
                    "port": { "type": "Port" }
                }
            },
            "ServicesByName": {
                "type": "object",
                "values": { "type": "ServiceConfig" }
            }
        }
    }))
    .unwrap();

    let data = json!({
        "services": {
            "api": { "port": 8080 }
        }
    });
    let mut hints = BTreeMap::new();
    hints.insert("$.services".to_string(), "ServicesByName".to_string());
    hints.insert("$.services.api".to_string(), "ServiceConfig".to_string());
    hints.insert("$.services.api.port".to_string(), "Port".to_string());

    validate_type_hints(&data, &hints, &schema).unwrap();
}

#[test]
fn validate_json_against_schema_type_mismatch_is_reported() {
    let err = validate_json_against_schema(&json!("1"), &json!({"type": "integer"}), "$.count")
        .unwrap_err();

    assert!(err.to_string().contains("type mismatch"));
}

#[test]
fn validate_constraints_trims_expression_whitespace() {
    let data = json!({"replicas": 2});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("replicas".to_string(), vec!["   value >= 1   ".to_string()]);

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_constraints_reports_env_lookup_error() {
    let data = json!({"mode": "prod"});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("mode".to_string(), vec!["value == env.MODE".to_string()]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("unknown env binding"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_enum_shape() {
    let err = validate_json_against_schema(&json!(1), &json!({"enum": true}), "$.x").unwrap_err();
    assert!(err.to_string().contains("'enum'"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_required_shape() {
    let err = validate_json_against_schema(
        &json!({"a": 1}),
        &json!({"type": "object", "required": true}),
        "$",
    )
    .unwrap_err();
    assert!(err.to_string().contains("required at $ must be an array"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_required_entry_type() {
    let err = validate_json_against_schema(
        &json!({"a": 1}),
        &json!({"type": "object", "required": [1]}),
        "$",
    )
    .unwrap_err();
    assert!(err.to_string().contains("required entries"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_properties_shape() {
    let err = validate_json_against_schema(
        &json!({"a": 1}),
        &json!({"type": "object", "properties": true}),
        "$",
    )
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("properties at $ must be an object"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_numeric_keyword_types() {
    let err = validate_json_against_schema(
        &json!(1),
        &json!({"type": "integer", "minimum": "one"}),
        "$.x",
    )
    .unwrap_err();
    assert!(err.to_string().contains("minimum"));

    let err = validate_json_against_schema(
        &json!(1),
        &json!({"type": "integer", "maximum": "one"}),
        "$.x",
    )
    .unwrap_err();
    assert!(err.to_string().contains("maximum"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_array_keyword_types() {
    let err = validate_json_against_schema(
        &json!([1]),
        &json!({"type": "array", "minItems": "1"}),
        "$.arr",
    )
    .unwrap_err();
    assert!(err.to_string().contains("minItems"));

    let err = validate_json_against_schema(
        &json!([1]),
        &json!({"type": "array", "maxItems": "1"}),
        "$.arr",
    )
    .unwrap_err();
    assert!(err.to_string().contains("maxItems"));
}

#[test]
fn validate_json_against_schema_rejects_invalid_string_keyword_types() {
    let err = validate_json_against_schema(
        &json!("a"),
        &json!({"type": "string", "minLength": "1"}),
        "$.s",
    )
    .unwrap_err();
    assert!(err.to_string().contains("minLength"));

    let err = validate_json_against_schema(
        &json!("a"),
        &json!({"type": "string", "maxLength": "1"}),
        "$.s",
    )
    .unwrap_err();
    assert!(err.to_string().contains("maxLength"));
}

#[test]
fn validate_constraints_rejects_nonexistent_nested_path() {
    let data = json!({"root": {"a": 1}});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("root.missing".to_string(), vec!["value == 1".to_string()]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err
        .to_string()
        .contains("constraint path 'root.missing' not found"));
}

#[test]
fn validate_constraints_with_float_math() {
    let data = json!({"ratio": 0.5});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("ratio".to_string(), vec!["value < 1.0".to_string()]);

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn parse_schema_accepts_empty_sections() {
    let schema = parse_schema(&json!({})).unwrap();
    assert!(schema.types.is_empty());
    assert!(schema.type_constraints.is_empty());
}

#[test]
fn validate_type_hints_with_builtin_types() {
    let schema = parse_schema(&json!({})).unwrap();
    let data = json!({"ok": true, "n": 1, "s": "x"});
    let mut hints = BTreeMap::new();
    hints.insert("$.ok".to_string(), "boolean".to_string());
    hints.insert("$.n".to_string(), "integer".to_string());
    hints.insert("$.s".to_string(), "string".to_string());

    validate_type_hints(&data, &hints, &schema).unwrap();
}

#[test]
fn parse_schema_constraint_entry_must_be_string_or_list() {
    let err = parse_schema(&json!({
        "types": {
            "Port": {
                "type": "integer",
                "constraints": 123
            }
        }
    }))
    .unwrap_err();
    assert!(err
        .to_string()
        .contains("schema.Port.constraints must be string, list of strings, or mapping"));
}

#[test]
fn validate_json_against_schema_ignores_irrelevant_keywords_for_type() {
    let value = json!(true);
    let schema = json!({"type": "boolean", "minimum": 3, "minLength": 2});
    validate_json_against_schema(&value, &schema, "$.flag").unwrap();
}

#[test]
fn constraints_can_reference_root_paths() {
    let data = json!({"a": 2, "b": 4});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("$.b".to_string(), vec!["value == a * 2".to_string()]);

    validate_constraints(&data, &env, &constraints).unwrap();
}

#[test]
fn validate_constraints_rejects_too_many_expressions_for_path() {
    let data = json!({"a": 1});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("a".to_string(), vec!["value == 1".to_string(); 129]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err.to_string().contains("too many constraints for path"));
}

#[test]
fn validate_constraints_rejects_overlong_expression() {
    let data = json!({"a": 1});
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert("a".to_string(), vec!["x".repeat(5000)]);

    let err = validate_constraints(&data, &env, &constraints).unwrap_err();
    assert!(err
        .to_string()
        .contains("constraint expression at '$.a' exceeds max length"));
}

#[test]
fn validate_schema_type_references_accepts_valid_constructor_schema() {
    let schema = parse_schema(&json!({
        "Color": {
            "type": "object",
            "properties": {
                "red": "integer",
                "green": "integer",
                "blue": "integer",
                "alpha": "number?"
            },
            "constructors": {
                "rgb": {
                        "regex": "^rgb\\((?<red>\\d+),(?<green>\\d+),(?<blue>\\d+)\\)$",
                        "defaults": {
                            "alpha": 1
                        }
                    },
                "hex": {
                        "regex": "^#(?<red_hex>[0-9A-Fa-f]{2})(?<green_hex>[0-9A-Fa-f]{2})(?<blue_hex>[0-9A-Fa-f]{2})$",
                        "map": {
                            "red": { "group": "red_hex", "decode": "hex_u8" },
                            "green": { "group": "green_hex", "decode": "hex_u8" },
                            "blue": { "group": "blue_hex", "decode": "hex_u8" }
                        }
                    }
            }
        }
    }))
    .unwrap();

    validate_schema_type_references(&schema.types).unwrap();
}

#[test]
fn validate_schema_type_references_rejects_invalid_constructor_regex() {
    let schema = parse_schema(&json!({
        "Color": {
            "type": "object",
            "constructors": {
                "bad": { "regex": "[a-z" }
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(err.to_string().contains("invalid constructor regex"));
}

#[test]
fn validate_schema_type_references_rejects_constructor_decode_name() {
    let schema = parse_schema(&json!({
        "Color": {
            "type": "object",
            "properties": {
                "red": "integer"
            },
            "constructors": {
                "bad_decode": {
                        "regex": "^r(?<raw>[0-9A-Fa-f]{2})$",
                        "map": {
                            "red": { "group": "raw", "decode": "hex_byte" }
                        }
                    }
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(err.to_string().contains("unsupported decode"));
}

#[test]
fn validate_schema_type_references_rejects_constructors_for_non_object_types() {
    let schema = parse_schema(&json!({
        "ColorName": {
            "type": "string",
            "constructors": {
                "named": { "regex": "^red$" }
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(err.to_string().contains("require type: object"));
}

#[test]
fn validate_schema_type_references_accepts_constructor_from_enum() {
    let schema = parse_schema(&json!({
        "MemoryUnit": ["MiB", "GiB"],
        "MemorySpec": {
            "type": "object",
            "properties": {
                "amount": "integer",
                "unit": "MemoryUnit"
            },
            "constructors": {
                "parse": {
                        "regex": "^(?<amount>\\d+)(?<raw_unit>[A-Za-z]+)$",
                        "map": {
                            "amount": { "group": "amount", "decode": "integer" },
                            "unit": { "group": "raw_unit", "from_enum": "MemoryUnit" }
                        }
                    }
            }
        }
    }))
    .unwrap();

    validate_schema_type_references(&schema.types).unwrap();
}

#[test]
fn validate_schema_type_references_rejects_constructor_from_enum_decode_conflict() {
    let schema = parse_schema(&json!({
        "MemoryUnit": ["MiB", "GiB"],
        "MemorySpec": {
            "type": "object",
            "properties": {
                "unit": "MemoryUnit"
            },
            "constructors": {
                "bad": {
                        "regex": "^(?<raw_unit>[A-Za-z]+)$",
                        "map": {
                            "unit": { "group": "raw_unit", "decode": "string", "from_enum": "MemoryUnit" }
                        }
                    }
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(err
        .to_string()
        .contains("cannot set both 'decode' and 'from_enum'"));
}

#[test]
fn validate_schema_type_references_rejects_constructor_from_enum_unknown_type() {
    let schema = parse_schema(&json!({
        "MemorySpec": {
            "type": "object",
            "properties": {
                "unit": "string"
            },
            "constructors": {
                "bad_enum_ref": {
                        "regex": "^(?<raw_unit>[A-Za-z]+)$",
                        "map": {
                            "unit": { "group": "raw_unit", "from_enum": "MissingEnum" }
                        }
                    }
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(err.to_string().contains("unknown type reference"));
    assert!(err.to_string().contains("MissingEnum"));
}

// --- Union type tests ---

#[test]
fn parse_schema_normalizes_pipe_shorthand_to_union() {
    let schema = parse_schema(&json!({
        "types": {
            "FlexValue": "string | integer | boolean"
        }
    }))
    .unwrap();

    let flex = &schema.types["FlexValue"];
    assert_eq!(flex["type"], json!("union"));
    let options = flex["options"].as_array().unwrap();
    assert_eq!(options.len(), 3);
    assert_eq!(options[0]["type"], json!("string"));
    assert_eq!(options[1]["type"], json!("integer"));
    assert_eq!(options[2]["type"], json!("boolean"));
}

#[test]
fn validate_union_list_based_first_match_wins() {
    let schema = parse_schema(&json!({
        "types": {
            "FlexValue": {
                "type": "union",
                "options": [
                    {"type": "integer"},
                    {"type": "string"}
                ]
            }
        }
    }))
    .unwrap();

    // Integer matches first option.
    validate_json_against_schema_with_types(
        &json!(42),
        schema.types.get("FlexValue").unwrap(),
        "$.val",
        &schema.types,
    )
    .unwrap();

    // String matches second option.
    validate_json_against_schema_with_types(
        &json!("hello"),
        schema.types.get("FlexValue").unwrap(),
        "$.val",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn validate_union_tagged_dispatch() {
    let schema = parse_schema(&json!({
        "types": {
            "ApiResponse": {
                "type": "union",
                "tag": "status",
                "options": {
                    "ok": {
                        "type": "object",
                        "properties": {
                            "status": {"type": "string"},
                            "data": {"type": "string"}
                        }
                    },
                    "error": {
                        "type": "object",
                        "properties": {
                            "status": {"type": "string"},
                            "message": {"type": "string"}
                        }
                    }
                }
            }
        }
    }))
    .unwrap();

    // Tag "ok" dispatches to success option.
    validate_json_against_schema_with_types(
        &json!({"status": "ok", "data": "result"}),
        schema.types.get("ApiResponse").unwrap(),
        "$.resp",
        &schema.types,
    )
    .unwrap();

    // Tag "error" dispatches to error option.
    validate_json_against_schema_with_types(
        &json!({"status": "error", "message": "fail"}),
        schema.types.get("ApiResponse").unwrap(),
        "$.resp",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn validate_union_tag_required_missing_tag() {
    let schema = parse_schema(&json!({
        "types": {
            "Tagged": {
                "type": "union",
                "tag": "kind",
                "tag_required": true,
                "options": {
                    "a": {"type": "object", "properties": {"kind": {"type": "string"}}}
                }
            }
        }
    }))
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!({"other": 1}),
        schema.types.get("Tagged").unwrap(),
        "$.item",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("tag field 'kind' is required"));
}

#[test]
fn validate_union_tag_required_non_object() {
    let schema = parse_schema(&json!({
        "types": {
            "Tagged": {
                "type": "union",
                "tag": "kind",
                "tag_required": true,
                "options": {
                    "a": {"type": "object", "properties": {"kind": {"type": "string"}}}
                }
            }
        }
    }))
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!("just a string"),
        schema.types.get("Tagged").unwrap(),
        "$.item",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("tag field 'kind' is required"));
}

#[test]
fn validate_union_no_match_error() {
    let schema = parse_schema(&json!({
        "types": {
            "StrictUnion": {
                "type": "union",
                "options": [
                    {"type": "integer"},
                    {"type": "boolean"}
                ]
            }
        }
    }))
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!("string value"),
        schema.types.get("StrictUnion").unwrap(),
        "$.val",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("union mismatch"));
    assert!(err.to_string().contains("did not match any option"));
}

#[test]
fn validate_union_as_property_type() {
    let schema = parse_schema(&json!({
        "types": {
            "Config": {
                "type": "object",
                "properties": {
                    "value": {
                        "type": "union",
                        "options": [
                            {"type": "string"},
                            {"type": "integer"}
                        ]
                    }
                }
            }
        }
    }))
    .unwrap();

    validate_json_against_schema_with_types(
        &json!({"value": "hello"}),
        schema.types.get("Config").unwrap(),
        "$.cfg",
        &schema.types,
    )
    .unwrap();

    validate_json_against_schema_with_types(
        &json!({"value": 42}),
        schema.types.get("Config").unwrap(),
        "$.cfg",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn validate_union_as_array_item_type() {
    let schema = parse_schema(&json!({
        "types": {
            "MixedList": {
                "type": "array",
                "items": {
                    "type": "union",
                    "options": [
                        {"type": "string"},
                        {"type": "integer"}
                    ]
                }
            }
        }
    }))
    .unwrap();

    validate_json_against_schema_with_types(
        &json!(["hello", 42, "world"]),
        schema.types.get("MixedList").unwrap(),
        "$.list",
        &schema.types,
    )
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!(["hello", true]),
        schema.types.get("MixedList").unwrap(),
        "$.list",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("union mismatch"));
}

#[test]
fn validate_union_missing_options_error() {
    let schema = parse_schema(&json!({
        "types": {
            "Bad": {
                "type": "union"
            }
        }
    }))
    .unwrap();

    let err = validate_json_against_schema_with_types(
        &json!(1),
        schema.types.get("Bad").unwrap(),
        "$.val",
        &schema.types,
    )
    .unwrap_err();
    assert!(err.to_string().contains("requires 'options'"));
}

#[test]
fn validate_union_with_named_type_references() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": {"type": "integer", "minimum": 1, "maximum": 65535},
            "Hostname": {"type": "string", "minLength": 1},
            "Target": {
                "type": "union",
                "options": [
                    {"type": "Port"},
                    {"type": "Hostname"}
                ]
            }
        }
    }))
    .unwrap();

    validate_json_against_schema_with_types(
        &json!(8080),
        schema.types.get("Target").unwrap(),
        "$.target",
        &schema.types,
    )
    .unwrap();

    validate_json_against_schema_with_types(
        &json!("localhost"),
        schema.types.get("Target").unwrap(),
        "$.target",
        &schema.types,
    )
    .unwrap();
}

#[test]
fn validate_schema_type_references_accepts_union_type() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": {"type": "integer"},
            "MyUnion": {
                "type": "union",
                "options": [
                    {"type": "Port"},
                    {"type": "string"}
                ]
            }
        }
    }))
    .unwrap();

    validate_schema_type_references(&schema.types).unwrap();
}

#[test]
fn parse_schema_normalizes_pipe_shorthand_with_named_types() {
    let schema = parse_schema(&json!({
        "types": {
            "Port": {"type": "integer"},
            "Hostname": {"type": "string"},
            "Target": "Port | Hostname"
        }
    }))
    .unwrap();

    let target = &schema.types["Target"];
    assert_eq!(target["type"], json!("union"));
    let options = target["options"].as_array().unwrap();
    assert_eq!(options[0]["type"], json!("Port"));
    assert_eq!(options[1]["type"], json!("Hostname"));

    validate_schema_type_references(&schema.types).unwrap();
}

// ── Versioned field annotation tests ─────────────────────────────────────────

fn no_env() -> MapEnvProvider {
    MapEnvProvider::new(std::collections::HashMap::new())
}

#[test]
fn versioned_field_valid_annotations() {
    let raw = json!({
        "Item": {
            "type": "object",
            "properties": {
                "id": { "type": "integer", "field_number": 1, "since": "1.0.0" },
                "name": { "type": "string", "field_number": 2, "since": "1.0.0",
                          "deprecated": "2.0.0", "optional": true },
                "old": { "type": "string", "field_number": 3, "since": "1.0.0",
                         "removed": "3.0.0", "optional": true }
            }
        }
    });
    parse_schema(&raw).unwrap();
}

#[test]
fn versioned_field_since_not_semver() {
    let raw = json!({
        "Item": {
            "type": "object",
            "properties": {
                "id": { "type": "integer", "since": "banana" }
            }
        }
    });
    let err = parse_schema(&raw).unwrap_err();
    assert!(err.to_string().contains("since"), "expected 'since' in error: {err}");
}

#[test]
fn versioned_field_ordering_violation() {
    // since > deprecated.version — violates ordering
    let raw = json!({
        "Item": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "since": "2.0.0",
                        "deprecated": "1.0.0", "optional": true }
            }
        }
    });
    let err = parse_schema(&raw).unwrap_err();
    assert!(err.to_string().contains("ordering"), "expected ordering error: {err}");
}

#[test]
fn versioned_field_removed_requires_optional() {
    let raw = json!({
        "Item": {
            "type": "object",
            "properties": {
                "gone": { "type": "string", "since": "1.0.0", "removed": "2.0.0" }
            }
        }
    });
    let err = parse_schema(&raw).unwrap_err();
    assert!(err.to_string().contains("optional"), "expected optional error: {err}");
}

#[test]
fn versioned_field_duplicate_field_number() {
    let raw = json!({
        "Item": {
            "type": "object",
            "properties": {
                "a": { "type": "string", "field_number": 1 },
                "b": { "type": "string", "field_number": 1 }
            }
        }
    });
    let err = parse_schema(&raw).unwrap_err();
    assert!(err.to_string().contains("duplicate field_number"), "expected duplicate error: {err}");
}

#[test]
fn versioned_field_deprecated_object_form() {
    let schema = json!({
        "type": "string",
        "deprecated": {
            "version": "2.0.0",
            "severity": "error",
            "message": "Use something else"
        },
        "optional": true
    });
    let meta = parse_field_version_meta(&schema).unwrap().unwrap();
    let dep = meta.deprecated.unwrap();
    assert_eq!(dep.version.to_string(), "2.0.0");
    assert_eq!(dep.severity, super_yaml::schema::DeprecationSeverity::Error);
    assert_eq!(dep.message.as_deref(), Some("Use something else"));
}

#[test]
fn versioned_field_deprecated_unknown_severity() {
    let schema = json!({
        "type": "string",
        "deprecated": { "version": "2.0.0", "severity": "fatal" },
        "optional": true
    });
    let err = parse_field_version_meta(&schema).unwrap_err();
    assert!(err.to_string().contains("severity"), "expected severity error: {err}");
}

#[test]
fn versioned_fields_since_blocks_old_version() {
    let doc = r#"
---!syaml/v0
---meta
file:
  schema_version: "1.0.0"
---schema
Item:
  type: object
  properties:
    new_field:
      type: string
      since: "2.0.0"
      optional: true
---data
item <Item>:
  new_field: hello
"#;
    let err = compile_document(doc, &no_env()).unwrap_err();
    assert!(err.to_string().contains("not available until"), "expected since error: {err}");
}

#[test]
fn versioned_fields_removed_blocks_new_version() {
    let doc = r#"
---!syaml/v0
---meta
file:
  schema_version: "3.0.0"
---schema
Item:
  type: object
  properties:
    gone:
      type: string
      since: "1.0.0"
      removed: "3.0.0"
      optional: true
---data
item <Item>:
  gone: still_here
"#;
    let err = compile_document(doc, &no_env()).unwrap_err();
    assert!(err.to_string().contains("removed in version"), "expected removed error: {err}");
}

#[test]
fn versioned_fields_deprecated_returns_warning() {
    let doc = r#"
---!syaml/v0
---meta
file:
  schema_version: "2.0.0"
---schema
Item:
  type: object
  properties:
    old_field:
      type: string
      since: "1.0.0"
      deprecated: "2.0.0"
      optional: true
---data
item <Item>:
  old_field: value
"#;
    let compiled = compile_document(doc, &no_env()).unwrap();
    assert_eq!(compiled.warnings.len(), 1);
    assert!(compiled.warnings[0].contains("deprecated"), "expected deprecation warning: {:?}", compiled.warnings);
}

#[test]
fn versioned_fields_deprecated_error_severity() {
    let doc = r#"
---!syaml/v0
---meta
file:
  schema_version: "2.0.0"
---schema
Item:
  type: object
  properties:
    bad_field:
      type: string
      deprecated:
        version: "2.0.0"
        severity: error
      optional: true
---data
item <Item>:
  bad_field: value
"#;
    let err = compile_document(doc, &no_env()).unwrap_err();
    assert!(err.to_string().contains("version field error"), "expected version field error: {err}");
}

#[test]
fn versioned_fields_no_target_version_skips_checks() {
    // No schema_version in meta.file → all version checks skipped
    let doc = r#"
---!syaml/v0
---schema
Item:
  type: object
  properties:
    old_field:
      type: string
      deprecated: "1.0.0"
      optional: true
    future_field:
      type: string
      since: "99.0.0"
      optional: true
---data
item <Item>:
  old_field: value
  future_field: value
"#;
    let compiled = compile_document(doc, &no_env()).unwrap();
    assert!(compiled.warnings.is_empty(), "expected no warnings without schema_version");
}

#[test]
fn versioned_fields_absent_deprecated_no_warning() {
    // Deprecated field is NOT present in data → no warning
    let doc = r#"
---!syaml/v0
---meta
file:
  schema_version: "2.0.0"
---schema
Item:
  type: object
  properties:
    name:
      type: string
    old_field:
      type: string
      deprecated: "2.0.0"
      optional: true
---data
item <Item>:
  name: Alice
"#;
    let compiled = compile_document(doc, &no_env()).unwrap();
    assert!(compiled.warnings.is_empty(), "expected no warning when deprecated field is absent");
}

#[test]
fn strict_field_numbers_passes_when_all_fields_have_numbers() {
    let doc = r#"---!syaml/v0
---meta
file:
  strict_field_numbers: true
---schema
Item:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
    name:
      type: string
      field_number: 2
---data
item <Item>:
  id: 42
  name: Widget
"#;
    compile_document(doc, &no_env()).unwrap();
}

#[test]
fn strict_field_numbers_errors_on_missing_field_number() {
    let doc = r#"---!syaml/v0
---meta
file:
  strict_field_numbers: true
---schema
Item:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
    name:
      type: string
---data
item <Item>:
  id: 42
  name: Widget
"#;
    let err = compile_document(doc, &no_env()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("strict_field_numbers") && msg.contains("name"),
        "expected strict_field_numbers error for 'name', got: {msg}"
    );
}

#[test]
fn strict_field_numbers_skips_check_when_flag_is_false() {
    let doc = r#"---!syaml/v0
---meta
file:
  strict_field_numbers: false
---schema
Item:
  type: object
  properties:
    id:
      type: integer
    name:
      type: string
---data
item <Item>:
  id: 42
  name: Widget
"#;
    // No field_numbers and flag is false — should compile without error.
    compile_document(doc, &no_env()).unwrap();
}

// ── extends integration tests ────────────────────────────────────────────────

#[test]
fn extends_full_document_compiles_correctly() {
    let doc = r#"---!syaml/v0
---schema
Animal:
  type: object
  properties:
    name: string
    age: integer
"Dog <Animal>":
  type: object
  properties:
    breed: string
---data
pet <Dog>:
  name: Rex
  age: 3
  breed: Labrador
"#;
    let compiled = compile_document(doc, &no_env()).unwrap();
    let val = &compiled.value;
    assert_eq!(val["pet"]["name"], "Rex");
    assert_eq!(val["pet"]["age"], 3);
    assert_eq!(val["pet"]["breed"], "Labrador");
}

#[test]
fn extends_inherited_fields_appear_in_compiled_output() {
    let doc = r#"---!syaml/v0
---schema
Base:
  type: object
  properties:
    id: string
"Child <Base>":
  type: object
  properties:
    value: integer
---data
item <Child>:
  id: abc
  value: 42
"#;
    let compiled = compile_document(doc, &no_env()).unwrap();
    assert_eq!(compiled.value["item"]["id"], "abc");
    assert_eq!(compiled.value["item"]["value"], 42);
}

#[test]
fn extends_constraint_referencing_inherited_field_works() {
    let doc = r#"---!syaml/v0
---schema
Base:
  type: object
  properties:
    min_val: integer
"Range <Base>":
  type: object
  properties:
    max_val: integer
  constraints:
    - "min_val <= max_val"
---data
r <Range>:
  min_val: 10
  max_val: 20
"#;
    compile_document(doc, &no_env()).unwrap();
}

#[test]
fn extends_child_missing_parent_field_fails_validation() {
    let doc = r#"---!syaml/v0
---schema
Base:
  type: object
  properties:
    required_field: string
"Child <Base>":
  type: object
  properties:
    extra: integer
---data
item <Child>:
  extra: 5
"#;
    // required_field is required (no optional: true) so should fail
    let err = compile_document(doc, &no_env()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("required_field"),
        "expected missing required_field error, got: {msg}"
    );
}

#[test]
fn validate_schema_type_references_accepts_valid_as_string() {
    let schema = parse_schema(&json!({
        "Version": {
            "type": "object",
            "as_string": "{{major}}.{{minor}}.{{patch}}",
            "properties": {
                "major": "integer",
                "minor": "integer",
                "patch": "integer"
            }
        }
    }))
    .unwrap();

    validate_schema_type_references(&schema.types).unwrap();
}

#[test]
fn validate_schema_type_references_rejects_as_string_on_non_object() {
    let schema = parse_schema(&json!({
        "Tag": {
            "type": "string",
            "as_string": "{{value}}"
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(
        err.to_string().contains("requires type: object"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn validate_schema_type_references_rejects_as_string_unknown_property() {
    let schema = parse_schema(&json!({
        "Version": {
            "type": "object",
            "as_string": "{{major}}.{{minor}}.{{unknown}}",
            "properties": {
                "major": "integer",
                "minor": "integer"
            }
        }
    }))
    .unwrap();

    let err = validate_schema_type_references(&schema.types).unwrap_err();
    assert!(
        err.to_string().contains("unknown property 'unknown'"),
        "unexpected error: {}",
        err
    );
}
