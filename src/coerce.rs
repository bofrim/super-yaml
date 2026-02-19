//! String constructor coercion for type-hinted object values.

use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::SyamlError;
use crate::mini_yaml;
use crate::resolve::get_json_path;

pub fn coerce_string_constructors_for_type_hints(
    data: &mut JsonValue,
    type_hints: &BTreeMap<String, String>,
    types: &BTreeMap<String, JsonValue>,
) -> Result<(), SyamlError> {
    for (path, type_name) in type_hints {
        let Some(raw_value) = get_json_path(data, path) else {
            continue;
        };
        let Some(source) = raw_value.as_str() else {
            continue;
        };
        let Some(type_schema) = types.get(type_name) else {
            continue;
        };
        let Some(constructors) = parse_constructors(type_schema, type_name, path, types)? else {
            continue;
        };

        let constructed = constructors.construct(source, path, type_name)?;
        set_json_path(data, path, constructed)?;
    }

    Ok(())
}

struct ConstructorsSpec {
    constructors: Vec<NamedConstructorSpec>,
    property_names: HashSet<String>,
}

impl ConstructorsSpec {
    fn construct(
        &self,
        source: &str,
        path: &str,
        type_name: &str,
    ) -> Result<JsonValue, SyamlError> {
        let mut ordered_matches: Vec<usize> = Vec::new();
        let mut unordered_matches: Vec<usize> = Vec::new();

        for (index, constructor) in self.constructors.iter().enumerate() {
            if !constructor.spec.regex.is_match(source) {
                continue;
            }
            if constructor.order.is_some() {
                ordered_matches.push(index);
            } else {
                unordered_matches.push(index);
            }
        }

        if !ordered_matches.is_empty() {
            let mut lowest_order = i64::MAX;
            for index in &ordered_matches {
                let order = self.constructors[*index].order.expect("ordered");
                if order < lowest_order {
                    lowest_order = order;
                }
            }

            let mut lowest_matches: Vec<usize> = ordered_matches
                .into_iter()
                .filter(|index| self.constructors[*index].order == Some(lowest_order))
                .collect();
            if lowest_matches.len() > 1 {
                let mut names: Vec<String> = lowest_matches
                    .iter()
                    .map(|index| self.constructors[*index].name.clone())
                    .collect();
                names.sort();
                return Err(SyamlError::SchemaError(format!(
                    "ambiguous ordered constructor match at {} for type '{}': multiple constructors with order {} matched ({})",
                    path,
                    type_name,
                    lowest_order,
                    names.join(", ")
                )));
            }
            let chosen = lowest_matches.pop().expect("one lowest match");
            let constructor = &self.constructors[chosen];
            let captures = constructor
                .spec
                .regex
                .captures(source)
                .expect("matched by is_match");
            return constructor
                .spec
                .construct(&captures, path, type_name, &self.property_names);
        }

        if unordered_matches.len() == 1 {
            let constructor = &self.constructors[unordered_matches[0]];
            let captures = constructor
                .spec
                .regex
                .captures(source)
                .expect("matched by is_match");
            return constructor
                .spec
                .construct(&captures, path, type_name, &self.property_names);
        }
        if unordered_matches.len() > 1 {
            let mut names: Vec<String> = unordered_matches
                .iter()
                .map(|index| self.constructors[*index].name.clone())
                .collect();
            names.sort();
            return Err(SyamlError::SchemaError(format!(
                "ambiguous unordered constructor match at {} for type '{}': multiple constructors matched ({})",
                path,
                type_name,
                names.join(", ")
            )));
        }

        Err(SyamlError::SchemaError(format!(
            "constructor pattern mismatch at {} for type '{}': value '{}' did not match any constructor",
            path, type_name, source
        )))
    }
}

struct NamedConstructorSpec {
    name: String,
    order: Option<i64>,
    spec: PatternSpec,
}

struct PatternSpec {
    regex: Regex,
    mappings: BTreeMap<String, MappingRule>,
    defaults: JsonMap<String, JsonValue>,
}

impl PatternSpec {
    fn construct(
        &self,
        captures: &regex::Captures<'_>,
        path: &str,
        type_name: &str,
        property_names: &HashSet<String>,
    ) -> Result<JsonValue, SyamlError> {
        let mut out = JsonMap::new();
        let mut mapped_destinations: HashSet<String> = HashSet::new();
        let mut mapped_source_groups: HashSet<String> = HashSet::new();

        for (destination, rule) in &self.mappings {
            mapped_destinations.insert(destination.clone());
            mapped_source_groups.insert(rule.group.clone());
            let Some(captured) = captures.name(&rule.group) else {
                continue;
            };
            let decoded = decode_capture(
                captured.as_str(),
                &rule.decode,
                path,
                type_name,
                destination,
            )?;
            out.insert(destination.clone(), decoded);
        }

        for capture_name in self.regex.capture_names().flatten() {
            if mapped_destinations.contains(capture_name) {
                continue;
            }
            if mapped_source_groups.contains(capture_name) {
                continue;
            }
            if !property_names.contains(capture_name) {
                continue;
            }
            let Some(captured) = captures.name(capture_name) else {
                continue;
            };
            let decoded = decode_capture(
                captured.as_str(),
                &MappingDecode::Decode(DecodeKind::Auto),
                path,
                type_name,
                capture_name,
            )?;
            out.insert(capture_name.to_string(), decoded);
        }

        for (key, default_value) in &self.defaults {
            out.entry(key.clone())
                .or_insert_with(|| default_value.clone());
        }

        Ok(JsonValue::Object(out))
    }
}

#[derive(Clone, Copy)]
enum DecodeKind {
    Auto,
    String,
    Integer,
    Number,
    Boolean,
    HexU8,
    HexAlpha,
}

struct MappingRule {
    group: String,
    decode: MappingDecode,
}

enum MappingDecode {
    Decode(DecodeKind),
    FromEnum {
        type_name: String,
        allowed: HashSet<String>,
    },
}

fn parse_constructors(
    schema: &JsonValue,
    type_name: &str,
    path: &str,
    types: &BTreeMap<String, JsonValue>,
) -> Result<Option<ConstructorsSpec>, SyamlError> {
    let Some(schema_obj) = schema.as_object() else {
        return Err(SyamlError::SchemaError(format!(
            "schema for type '{}' at {} must be an object",
            type_name, path
        )));
    };
    let Some(raw_constructors) = schema_obj.get("constructors") else {
        return Ok(None);
    };
    let property_names = schema_obj
        .get("properties")
        .and_then(JsonValue::as_object)
        .map(|props| props.keys().cloned().collect::<HashSet<String>>())
        .unwrap_or_default();

    let constructors = raw_constructors.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "constructors for type '{}' at {} must be an object",
            type_name, path
        ))
    })?;
    if constructors.is_empty() {
        return Err(SyamlError::SchemaError(format!(
            "constructors for type '{}' at {} must not be empty",
            type_name, path
        )));
    }

    let mut parsed_constructors = Vec::with_capacity(constructors.len());
    for (constructor_name, raw_constructor) in constructors {
        let constructor_path = format!("{}.constructors.{}", type_name, constructor_name);
        let constructor = raw_constructor.as_object().ok_or_else(|| {
            SyamlError::SchemaError(format!("{constructor_path} must be an object"))
        })?;
        let regex_text = constructor
            .get("regex")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.regex must be a string"))
            })?;
        let regex = Regex::new(regex_text).map_err(|e| {
            SyamlError::SchemaError(format!(
                "invalid constructor regex '{}' at {}: {}",
                regex_text, constructor_path, e
            ))
        })?;
        let order = match constructor.get("order") {
            Some(raw) => {
                let parsed = raw.as_i64().ok_or_else(|| {
                    SyamlError::SchemaError(format!(
                        "{constructor_path}.order must be an integer >= 0"
                    ))
                })?;
                if parsed < 0 {
                    return Err(SyamlError::SchemaError(format!(
                        "{constructor_path}.order must be an integer >= 0"
                    )));
                }
                Some(parsed)
            }
            None => None,
        };

        let mut mappings = BTreeMap::new();
        if let Some(raw_map) = constructor.get("map") {
            let map = raw_map.as_object().ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.map must be an object"))
            })?;
            for (destination, raw_rule) in map {
                let rule_path = format!("{constructor_path}.map.{destination}");
                let rule = raw_rule.as_object().ok_or_else(|| {
                    SyamlError::SchemaError(format!("{rule_path} must be an object"))
                })?;
                let group = rule
                    .get("group")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!("{rule_path}.group must be a string"))
                    })?;
                let decode = match (rule.get("decode"), rule.get("from_enum")) {
                    (Some(_), Some(_)) => {
                        return Err(SyamlError::SchemaError(format!(
                            "{rule_path} cannot set both 'decode' and 'from_enum'"
                        )))
                    }
                    (Some(raw), None) => MappingDecode::Decode(parse_decode_kind(
                        raw.as_str().ok_or_else(|| {
                            SyamlError::SchemaError(format!("{rule_path}.decode must be a string"))
                        })?,
                        &format!("{rule_path}.decode"),
                    )?),
                    (None, Some(raw_from_enum)) => {
                        let enum_type_name = raw_from_enum.as_str().ok_or_else(|| {
                            SyamlError::SchemaError(format!(
                                "{rule_path}.from_enum must be a string"
                            ))
                        })?;
                        let allowed = resolve_enum_type_values(
                            types,
                            enum_type_name,
                            &format!("{rule_path}.from_enum"),
                        )?;
                        MappingDecode::FromEnum {
                            type_name: enum_type_name.to_string(),
                            allowed,
                        }
                    }
                    (None, None) => MappingDecode::Decode(DecodeKind::Auto),
                };
                mappings.insert(
                    destination.clone(),
                    MappingRule {
                        group: group.to_string(),
                        decode,
                    },
                );
            }
        }

        let defaults = match constructor.get("defaults") {
            Some(raw_defaults) => raw_defaults.as_object().cloned().ok_or_else(|| {
                SyamlError::SchemaError(format!("{constructor_path}.defaults must be an object"))
            })?,
            None => JsonMap::new(),
        };

        parsed_constructors.push(NamedConstructorSpec {
            name: constructor_name.clone(),
            order,
            spec: PatternSpec {
                regex,
                mappings,
                defaults,
            },
        });
    }

    Ok(Some(ConstructorsSpec {
        constructors: parsed_constructors,
        property_names,
    }))
}

fn resolve_enum_type_values(
    types: &BTreeMap<String, JsonValue>,
    enum_type_name: &str,
    path: &str,
) -> Result<HashSet<String>, SyamlError> {
    let enum_schema = types.get(enum_type_name).ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "unknown type reference at {}: '{}' not found in schema",
            path, enum_type_name
        ))
    })?;
    let enum_obj = enum_schema.as_object().ok_or_else(|| {
        SyamlError::SchemaError(format!(
            "referenced enum type '{}' at {} must be an object schema",
            enum_type_name, path
        ))
    })?;
    let enum_values = enum_obj
        .get("enum")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            SyamlError::SchemaError(format!(
                "referenced type '{}' at {} must declare an enum array",
                enum_type_name, path
            ))
        })?;
    let mut out = HashSet::new();
    for value in enum_values {
        let Some(as_str) = value.as_str() else {
            return Err(SyamlError::SchemaError(format!(
                "referenced enum type '{}' at {} must contain only strings",
                enum_type_name, path
            )));
        };
        out.insert(as_str.to_string());
    }
    Ok(out)
}

fn parse_decode_kind(value: &str, path: &str) -> Result<DecodeKind, SyamlError> {
    match value {
        "auto" => Ok(DecodeKind::Auto),
        "string" => Ok(DecodeKind::String),
        "integer" => Ok(DecodeKind::Integer),
        "number" => Ok(DecodeKind::Number),
        "boolean" => Ok(DecodeKind::Boolean),
        "hex_u8" => Ok(DecodeKind::HexU8),
        "hex_alpha" => Ok(DecodeKind::HexAlpha),
        _ => Err(SyamlError::SchemaError(format!(
            "unsupported decode '{}' at {}",
            value, path
        ))),
    }
}

fn decode_capture(
    raw: &str,
    decode: &MappingDecode,
    path: &str,
    type_name: &str,
    field: &str,
) -> Result<JsonValue, SyamlError> {
    match decode {
        MappingDecode::Decode(kind) => decode_capture_kind(raw, *kind, path, type_name, field),
        MappingDecode::FromEnum {
            type_name: enum_type,
            allowed,
        } => {
            if !allowed.contains(raw) {
                return Err(SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {}: '{}' is not in enum '{}'",
                    type_name, field, path, raw, enum_type
                )));
            }
            Ok(JsonValue::String(raw.to_string()))
        }
    }
}

fn decode_capture_kind(
    raw: &str,
    decode: DecodeKind,
    path: &str,
    type_name: &str,
    field: &str,
) -> Result<JsonValue, SyamlError> {
    match decode {
        DecodeKind::Auto => mini_yaml::parse_scalar(raw).map_err(|e| {
            SyamlError::SchemaError(format!(
                "failed to decode capture for {}.{} at {} with decode auto: {}",
                type_name, field, path, e
            ))
        }),
        DecodeKind::String => Ok(JsonValue::String(raw.to_string())),
        DecodeKind::Integer => {
            let parsed: i64 = raw.parse().map_err(|_| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as integer",
                    type_name, field, path
                ))
            })?;
            Ok(JsonValue::Number(parsed.into()))
        }
        DecodeKind::Number => {
            let parsed: f64 = raw.parse().map_err(|_| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as number",
                    type_name, field, path
                ))
            })?;
            let number = serde_json::Number::from_f64(parsed).ok_or_else(|| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as finite number",
                    type_name, field, path
                ))
            })?;
            Ok(JsonValue::Number(number))
        }
        DecodeKind::Boolean => match raw {
            "true" => Ok(JsonValue::Bool(true)),
            "false" => Ok(JsonValue::Bool(false)),
            _ => Err(SyamlError::SchemaError(format!(
                "failed to decode capture for {}.{} at {} as boolean",
                type_name, field, path
            ))),
        },
        DecodeKind::HexU8 => {
            let parsed = u8::from_str_radix(raw, 16).map_err(|_| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as hex_u8",
                    type_name, field, path
                ))
            })?;
            Ok(JsonValue::Number(serde_json::Number::from(parsed)))
        }
        DecodeKind::HexAlpha => {
            let parsed = u8::from_str_radix(raw, 16).map_err(|_| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as hex_alpha",
                    type_name, field, path
                ))
            })?;
            let alpha = (parsed as f64) / 255.0;
            let number = serde_json::Number::from_f64(alpha).ok_or_else(|| {
                SyamlError::SchemaError(format!(
                    "failed to decode capture for {}.{} at {} as finite hex_alpha",
                    type_name, field, path
                ))
            })?;
            Ok(JsonValue::Number(number))
        }
    }
}

fn set_json_path(root: &mut JsonValue, path: &str, value: JsonValue) -> Result<(), SyamlError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        *root = value;
        return Ok(());
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        match segment {
            PathSegment::Key(key) => {
                current = current
                    .as_object_mut()
                    .and_then(|map| map.get_mut(key))
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!(
                            "path '{}' not found while setting constructor value",
                            path
                        ))
                    })?;
            }
            PathSegment::Index(i) => {
                current = current
                    .as_array_mut()
                    .and_then(|arr| arr.get_mut(*i))
                    .ok_or_else(|| {
                        SyamlError::SchemaError(format!(
                            "path '{}' not found while setting constructor array index",
                            path
                        ))
                    })?;
            }
        }
    }

    match segments.last().expect("non-empty") {
        PathSegment::Key(key) => {
            let map = current.as_object_mut().ok_or_else(|| {
                SyamlError::SchemaError(format!("path '{}' does not point to object", path))
            })?;
            map.insert(key.clone(), value);
        }
        PathSegment::Index(i) => {
            let arr = current.as_array_mut().ok_or_else(|| {
                SyamlError::SchemaError(format!("path '{}' does not point to array", path))
            })?;
            if *i >= arr.len() {
                return Err(SyamlError::SchemaError(format!(
                    "array index out of bounds in path '{}'",
                    path
                )));
            }
            arr[*i] = value;
        }
    }

    Ok(())
}

#[derive(Debug)]
enum PathSegment {
    Key(String),
    Index(usize),
}

fn parse_path(path: &str) -> Result<Vec<PathSegment>, SyamlError> {
    if path == "$" {
        return Ok(Vec::new());
    }

    if !path.starts_with("$.") {
        return Err(SyamlError::SchemaError(format!(
            "invalid path '{}'; expected to start with '$.'",
            path
        )));
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = path[2..].chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '.' {
            if !current.is_empty() {
                out.push(PathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            continue;
        }

        if ch == '[' {
            if !current.is_empty() {
                out.push(PathSegment::Key(current.clone()));
                current.clear();
            }
            i += 1;
            let mut num = String::new();
            while i < chars.len() && chars[i] != ']' {
                num.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() || chars[i] != ']' {
                return Err(SyamlError::SchemaError(format!(
                    "invalid array path segment in '{}'",
                    path
                )));
            }
            i += 1;
            let idx: usize = num.parse().map_err(|_| {
                SyamlError::SchemaError(format!("invalid array index '{}' in '{}'", num, path))
            })?;
            out.push(PathSegment::Index(idx));
            continue;
        }

        current.push(ch);
        i += 1;
    }

    if !current.is_empty() {
        out.push(PathSegment::Key(current));
    }

    Ok(out)
}
