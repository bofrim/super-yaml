use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value as JsonValue};

use super_yaml::expr::eval::{evaluate, EvalContext, EvalError};
use super_yaml::expr::parse_expression;

fn eval_with(
    expr_src: &str,
    data: &JsonValue,
    env: &BTreeMap<String, JsonValue>,
    unresolved: &HashSet<String>,
    current_value: Option<&JsonValue>,
) -> Result<JsonValue, EvalError> {
    let expr = parse_expression(expr_src).unwrap();
    let imports = BTreeMap::new();
    let ctx = EvalContext {
        data,
        imports: &imports,
        env,
        unresolved_paths: unresolved,
        current_value,
        current_scope: None,
    };
    evaluate(&expr, &ctx)
}

#[test]
fn evaluates_arithmetic_precedence() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let out = eval_with("1 + 2 * 3", &data, &env, &unresolved, None).unwrap();
    assert_eq!(out, json!(7));
}

#[test]
fn evaluates_parentheses_and_unary() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let out = eval_with("-(1 + 2)", &data, &env, &unresolved, None).unwrap();
    assert_eq!(out, json!(-3));

    let out = eval_with("!false", &data, &env, &unresolved, None).unwrap();
    assert_eq!(out, json!(true));
}

#[test]
fn evaluates_boolean_logic_precedence() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let out = eval_with("true || false && false", &data, &env, &unresolved, None).unwrap();
    assert_eq!(out, json!(true));
}

#[test]
fn resolves_data_and_env_variables() {
    let data = json!({"price": 12, "qty": 3});
    let mut env = BTreeMap::new();
    env.insert("TAX".to_string(), json!(2));
    let unresolved = HashSet::new();

    let out = eval_with("price * qty + env.TAX", &data, &env, &unresolved, None).unwrap();
    assert_eq!(out, json!(38));
}

#[test]
fn supports_value_symbol_in_constraint_context() {
    let data = json!({"replicas": 3});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();
    let current = json!({"min": 1, "max": 4});

    let out = eval_with(
        "value.min < value.max",
        &data,
        &env,
        &unresolved,
        Some(&current),
    )
    .unwrap();
    assert_eq!(out, json!(true));
}

#[test]
fn supports_numeric_and_string_functions() {
    let data = json!({"nums": [1, 2, 3], "name": "abc"});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    assert_eq!(
        eval_with("max(1, 9, 4)", &data, &env, &unresolved, None).unwrap(),
        json!(9)
    );
    assert_eq!(
        eval_with("min(1, 9, 4)", &data, &env, &unresolved, None).unwrap(),
        json!(1)
    );
    assert_eq!(
        eval_with("round(2.6)", &data, &env, &unresolved, None).unwrap(),
        json!(3)
    );
    assert_eq!(
        eval_with("len(nums)", &data, &env, &unresolved, None).unwrap(),
        json!(3)
    );
    assert_eq!(
        eval_with("len(name)", &data, &env, &unresolved, None).unwrap(),
        json!(3)
    );
    assert_eq!(
        eval_with("coalesce(null, null, 5)", &data, &env, &unresolved, None).unwrap(),
        json!(5)
    );
}

#[test]
fn parse_expression_rejects_trailing_tokens() {
    let err = parse_expression("1 2").unwrap_err();
    assert!(err
        .to_string()
        .contains("unexpected token after expression"));
}

#[test]
fn parse_expression_rejects_single_equals() {
    let err = parse_expression("a = 1").unwrap_err();
    assert!(err.to_string().contains("use '==' for equality"));
}

#[test]
fn parse_expression_rejects_dot_without_identifier() {
    let err = parse_expression("a.(1)").unwrap_err();
    assert!(err.to_string().contains("expected identifier after '.'"));
}

#[test]
fn parse_expression_rejects_excessive_token_count() {
    let mut expr = "1".to_string();
    for _ in 0..1100 {
        expr.push_str("+1");
    }
    let err = parse_expression(&expr).unwrap_err();
    assert!(err
        .to_string()
        .contains("expression exceeds max token count"));
}

#[test]
fn evaluation_reports_unknown_env_binding() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let err = eval_with("env.MISSING", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e.to_string().contains("unknown env binding")),
        other => panic!("expected fatal error, got {other:?}"),
    }
}

#[test]
fn evaluation_reports_unresolved_dependency() {
    let data = json!({"a": 10});
    let env = BTreeMap::new();
    let mut unresolved = HashSet::new();
    unresolved.insert("$.a".to_string());

    let err = eval_with("a + 1", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Unresolved(path) => assert_eq!(path, "$.a"),
        other => panic!("expected unresolved error, got {other:?}"),
    }
}

#[test]
fn evaluation_reports_runtime_failures() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let err = eval_with("10 / 0", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e.to_string().contains("division by zero")),
        other => panic!("expected fatal error, got {other:?}"),
    }

    let err = eval_with("5 % 0", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e.to_string().contains("modulo by zero")),
        other => panic!("expected fatal error, got {other:?}"),
    }
}

#[test]
fn evaluation_reports_function_errors() {
    let data = json!({});
    let env = BTreeMap::new();
    let unresolved = HashSet::new();

    let err = eval_with("unknown_fn(1)", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e.to_string().contains("unknown function")),
        other => panic!("expected fatal error, got {other:?}"),
    }

    let err = eval_with("abs(1, 2)", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e.to_string().contains("abs expects 1 arguments")),
        other => panic!("expected fatal error, got {other:?}"),
    }

    let err = eval_with("len(123)", &data, &env, &unresolved, None).unwrap_err();
    match err {
        EvalError::Fatal(e) => assert!(e
            .to_string()
            .contains("len() expects string, array, or object")),
        other => panic!("expected fatal error, got {other:?}"),
    }
}
