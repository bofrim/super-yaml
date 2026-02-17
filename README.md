# super_yaml

`super_yaml` is a Rust crate and CLI for compiling a strict, sectioned YAML dialect (`.syaml`) into resolved JSON or YAML.

It combines:

- a predictable file structure (`marker + sections`)
- schema-backed validation
- inline type hints on keys
- derived values via expressions
- external configuration through environment bindings

## Why this exists

`super_yaml` is designed for configuration that should stay declarative but still support computed fields and validation. The format gives you enough expressiveness for real-world config while keeping parsing and behavior constrained.

## Quick Start

### 1. Validate a document

```bash
cargo run --bin super-yaml -- validate examples/basic.syaml
```

Expected output:

```text
OK
```

### 2. Compile to JSON

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --pretty
```

### 3. Compile to YAML

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --format yaml
```

## Document Format (v0)

### Marker

The first non-empty line must be:

```yaml
---!syaml/v0
```

If it is missing or different, parsing fails with a marker error.

### Section fences

Only these sections are allowed:

- `front_matter` (optional)
- `schema` (required)
- `data` (required)

Only these orders are valid:

1. `schema`, `data`
2. `front_matter`, `schema`, `data`

Unknown sections, duplicates, or other orderings are rejected.

### Full example

```yaml
---!syaml/v0
---front_matter
env:
  DB_HOST:
    from: env
    key: DB_HOST
    required: true
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 4

---schema
types:
  Port:
    type: integer
    minimum: 1
    maximum: 65535
constraints:
  replicas:
    - "value >= 1"
  max_connections:
    - "value % replicas == 0"

---data
host <string>: "${env.DB_HOST}"
port <Port>: 5432
replicas <integer>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <integer>: "=replicas * worker_threads * 25"
```

## `front_matter.env` Semantics

Each binding under `front_matter.env` maps a symbol (e.g. `CPU_CORES`) to a source.

Supported fields:

- `from`: currently only `env` is supported
- `key`: environment variable name to read
- `required`: defaults to `true`
- `default`: used when the env var is missing

Resolution behavior:

1. Read `key` from the configured environment provider.
2. If present, parse the string as a scalar using the mini YAML parser.
3. If absent and `default` exists, use `default`.
4. If absent and required, fail.
5. If absent and not required, produce `null`.

## `schema` Section

### `types`

`types` is a map of named schemas:

```yaml
types:
  Port:
    type: integer
    minimum: 1
    maximum: 65535
```

A data key can reference this type using `<TypeName>`.

### `constraints`

`constraints` maps paths to one expression or a list of expressions:

```yaml
constraints:
  replicas: "value >= 1"
  max_connections:
    - "value >= replicas"
    - "value % replicas == 0"
```

Path format:

- absolute: `$.a.b`
- shorthand: `a.b` (normalized internally to `$.a.b`)

Within constraints, `value` refers to the targeted node for that path.

## Supported Schema Keywords (v0)

The validator supports this subset:

- common: `type`, `enum`
- numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
- string: `minLength`, `maxLength`, `pattern`
- object: `properties`, `required`
- array: `items`, `minItems`, `maxItems`

Built-in primitive type names are also accepted directly:

- `string`, `integer`, `number`, `boolean`, `object`, `array`, `null`

## `data` Section and Type Hints

A data key may carry a type hint suffix:

```yaml
port <Port>: 5432
replicas <integer>: 3
```

Normalization behavior:

- key is rewritten to canonical name (`port`, `replicas`)
- hint is stored at JSON path (`$.port`, `$.replicas`)
- duplicate canonical keys are rejected

## Expressions

Expressions are used in two places:

- derived values in `data` (strings starting with `=`)
- interpolation segments in strings (`${...}`)
- schema constraints

### Variable sources

- data references: `replicas`, `service.port`, etc.
- environment symbols: `env.CPU_CORES`
- constraint-local target: `value` (only inside constraints)

### Operators

- arithmetic: `+ - * / %`
- comparison: `== != < <= > >=`
- boolean: `&& || !`
- grouping: `( ... )`

### Functions

- `min(x, ...)`
- `max(x, ...)`
- `abs(x)`
- `floor(x)`
- `ceil(x)`
- `round(x)`
- `len(x)` for string/array/object
- `coalesce(a, b, ...)`

### Derived value forms

1. Whole expression:

```yaml
worker_threads: "=max(2, env.CPU_CORES * 2)"
```

2. Interpolation:

```yaml
dsn: "postgres://${env.DB_HOST}:${port}/app"
```

Interpolation behavior:

- if the entire string is exactly one interpolation, the result keeps native type
- otherwise interpolated parts are concatenated into a string

### Dependency resolution and cycles

Derived values are resolved in passes.

- unresolved references are retried in later passes
- if no progress is possible, compilation fails with a cycle error listing unresolved paths

## Mini YAML Subset

The internal parser intentionally supports a constrained subset:

- mappings (`key: value`)
- sequences (`- item`)
- nested indentation with spaces
- inline objects (`{a: 1, b: 2}`)
- inline arrays (`[1, 2, 3]`)
- scalars: numbers, booleans, null, strings
- quoted strings with escapes
- comments (`# ...`) in supported positions

This is not a full YAML 1.2 parser.

## CLI Reference

```text
super-yaml validate <file> [--allow-env KEY]...
super-yaml compile <file> [--pretty] [--format json|yaml] [--allow-env KEY]...
super-yaml compile <file> [--yaml|--json] [--allow-env KEY]...
```

### `validate`

- reads file
- runs full compilation pipeline
- prints `OK` on success
- environment access is disabled unless keys are explicitly allowed

### `compile`

- reads file
- compiles and emits resolved output
- defaults to compact JSON
- environment access is disabled unless keys are explicitly allowed

Options:

- `--pretty`: pretty JSON output
- `--format json|yaml`: explicit output format
- `--yaml`, `--json`: format shortcuts
- `--allow-env KEY`: allow access to one process environment variable key (repeatable)

## Rust API

```rust,no_run
use super_yaml::{
    compile_document, compile_document_to_json, compile_document_to_yaml, validate_document,
    ProcessEnvProvider,
};

fn run(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let env = ProcessEnvProvider;

    let compiled = compile_document(input, &env)?;
    let json = compile_document_to_json(input, &env, true)?;
    let yaml = compile_document_to_yaml(input, &env)?;
    validate_document(input, &env)?;

    println!("{}", compiled.to_json_string(false)?);
    println!("{}", json);
    println!("{}", yaml);
    Ok(())
}
```

### Custom environment provider

```rust
use std::collections::HashMap;
use super_yaml::MapEnvProvider;

let mut vars = HashMap::new();
vars.insert("CPU_CORES".to_string(), "8".to_string());
let env = MapEnvProvider::new(vars);
```

## Compilation Pipeline

`compile_document` runs these steps:

1. scan marker + sections
2. parse section bodies
3. parse schema and normalize data/type hints
4. resolve env bindings
5. resolve expressions/interpolations
6. validate type hints against schema
7. validate constraints

If any step fails, compilation stops with a `SyamlError`.

## Error Categories

The library uses `SyamlError` variants to indicate where failures occurred:

- `MarkerError`
- `SectionError`
- `YamlParseError`
- `SchemaError`
- `TypeHintError`
- `ExpressionError`
- `ConstraintError`
- `EnvError`
- `CycleError`
- `SerializationError`
- `Io`

## Examples Directory

The repository ships with sample `.syaml` inputs and expected JSON outputs:

- `examples/basic.syaml`
- `examples/service_scaling.syaml`
- `examples/pricing_engine.syaml`
- `examples/inventory_policy.syaml`
- `examples/alert_rules.syaml`

Each has a matching `examples/*.expected.json`.

## Development

Run tests:

```bash
cargo test
```

Format and lint (if you add tools locally):

```bash
cargo fmt
cargo clippy --all-targets --all-features
```

## Current Limitations

- only `v0` marker is accepted
- only `from: env` bindings are supported
- expression variable paths are dot-based object traversal
- parser is a YAML subset, not full YAML
