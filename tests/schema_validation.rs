use std::collections::BTreeMap;

use serde_json::json;

use super_yaml::schema::{
    parse_schema, resolve_type_schema, validate_json_against_schema,
    validate_json_against_schema_with_types,
};
use super_yaml::validate::{
    build_effective_constraints, validate_constraints, validate_type_hints,
};

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
            "EpisodeConfig": {
                "type": "object",
                "properties": {
                    "initial_population_size": {
                        "type": "integer",
                        "constraints": ["value >= 1", "value <= max_agents"]
                    },
                    "max_agents": {
                        "type": "integer",
                        "constraints": "value >= 1"
                    }
                },
                "constraints": ["initial_population_size <= max_agents"]
            }
        }
    });

    let schema = parse_schema(&raw).unwrap();
    let by_type = schema.type_constraints.get("EpisodeConfig").unwrap();
    assert_eq!(
        by_type.get("$.initial_population_size").unwrap(),
        &vec!["value >= 1".to_string(), "value <= max_agents".to_string()]
    );
    assert_eq!(
        by_type.get("$.max_agents").unwrap(),
        &vec!["value >= 1".to_string()]
    );
    assert_eq!(
        by_type.get("$").unwrap(),
        &vec!["initial_population_size <= max_agents".to_string()]
    );
}

#[test]
fn parse_schema_collects_type_local_constraint_path_map() {
    let raw = json!({
        "types": {
            "EpisodeConfig": {
                "type": "object",
                "constraints": {
                    "initial_population_size": [
                        "value >= 1",
                        "value <= max_agents"
                    ],
                    "max_agents": "value >= 1"
                }
            }
        }
    });

    let schema = parse_schema(&raw).unwrap();
    let by_type = schema.type_constraints.get("EpisodeConfig").unwrap();
    assert_eq!(
        by_type.get("$.initial_population_size").unwrap(),
        &vec!["value >= 1".to_string(), "value <= max_agents".to_string()]
    );
    assert_eq!(
        by_type.get("$.max_agents").unwrap(),
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
            "EpisodeConfig": {
                "type": "object",
                "properties": {
                    "initial_population_size": {
                        "type": "integer",
                        "constraints": "value >= 1"
                    }
                },
                "constraints": ["initial_population_size <= max_agents"]
            }
        }
    }))
    .unwrap();

    let mut hints = BTreeMap::new();
    hints.insert("$.episode".to_string(), "EpisodeConfig".to_string());

    let effective = build_effective_constraints(&hints, &schema);
    assert_eq!(
        effective.get("$.episode.initial_population_size").unwrap(),
        &vec!["value >= 1".to_string()]
    );
    assert_eq!(
        effective.get("$.episode").unwrap(),
        &vec!["initial_population_size <= max_agents".to_string()]
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
            "VisionConfig": {
                "type": "object",
                "properties": {
                    "agent_physical_radius": {
                        "type": "PositiveNumber"
                    }
                }
            }
        }
    }))
    .unwrap();

    let value = json!({"agent_physical_radius": 10.0});
    let vision_schema = schema.types.get("VisionConfig").unwrap();

    validate_json_against_schema_with_types(
        &value,
        vision_schema,
        "$.agent.sensors.stereo_vision.eye",
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
            "VisionConfig": {
                "type": "object",
                "properties": {
                    "agent_physical_radius": {
                        "type": "PositiveNumber"
                    }
                }
            }
        }
    }))
    .unwrap();

    let value = json!({"agent_physical_radius": 0});
    let vision_schema = schema.types.get("VisionConfig").unwrap();

    let err = validate_json_against_schema_with_types(
        &value,
        vision_schema,
        "$.agent.sensors.stereo_vision.eye",
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
        "episode": {
            "initial_population_size": 2,
            "max_agents": 5
        }
    });
    let env = BTreeMap::new();
    let mut constraints = BTreeMap::new();
    constraints.insert(
        "episode.initial_population_size".to_string(),
        vec!["value <= max_agents".to_string()],
    );
    constraints.insert(
        "episode".to_string(),
        vec!["initial_population_size <= max_agents".to_string()],
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
