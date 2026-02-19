# super_yaml

`super_yaml` is a Rust crate and CLI for compiling a strict, sectioned YAML dialect (`.syaml`) into resolved JSON or YAML, and for generating first-pass Rust and TypeScript types from schema type definitions.

It combines:

- a predictable file structure (`marker + sections`)
- schema-backed validation
- inline type hints on keys
- derived values via expressions
- external configuration through environment bindings

## Why this exists

`super_yaml` is designed for configuration that should stay declarative but still support computed fields and validation. The format gives you enough expressiveness for real-world config while keeping parsing and behavior constrained.

## Maintenance Status

This is currently a personal project and is not actively maintained for external users.
Issues and pull requests are welcome, but may not receive a response.

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

### 4. Generate Rust types

```bash
cargo run --bin super-yaml -- compile examples/type_composition.syaml --format rust
```

### 5. Generate TypeScript types

```bash
cargo run --bin super-yaml -- compile examples/type_composition.syaml --format ts
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

- `meta` (optional)
- `schema` (optional)
- `data` (optional)

Unknown sections and duplicates are rejected. Section order is flexible.

When omitted, `schema` and `data` default to empty objects.

### Full example

```yaml
---!syaml/v0
---meta
file:
  owner: platform
  service: inventory
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
Port:
  type: integer
  minimum: 1
  maximum: 65535
ReplicaCount:
  type: integer
  constraints: "value >= 1"
MaxConnections:
  type: integer
  constraints: "value % replicas == 0"

---data
host <string>: "${env.DB_HOST}"
port <Port>: 5432
replicas <ReplicaCount>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <MaxConnections>: "=replicas * worker_threads * 25"
```

## `meta.file` Semantics

`meta.file` is an optional mapping for file-level details/metadata.

```yaml
file:
  owner: platform
  service: inventory
  revision: 3
```

Rules:

- `file` must be a mapping/object when present.
- values can be any YAML scalar/array/object supported by the parser.

## `meta.env` Semantics

Each binding under `meta.env` maps a symbol (e.g. `CPU_CORES`) to a source.

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

## `meta.imports` Semantics

`meta.imports` loads external `.syaml` documents under a namespace alias.

```yaml
imports:
  shared: ./shared.syaml
```

Rules:

- `imports` must be a mapping.
- each key is a namespace alias.
- each value is either a path string or an object with a string `path`.
- relative paths resolve from the importing file's directory (CLI and `*_from_path` APIs).

Behavior:

- imported data is mounted under `data.<alias>`.
- imported schema types are mounted under `<alias.TypeName>`.
- imported files run their own env/expression/type/constraint pipeline.
- cyclic imports are rejected.

## `schema` Section

Top-level keys in `schema` are named schemas:

```yaml
Port:
  type: integer
  minimum: 1
  maximum: 65535
```

A data key can reference this type using `<TypeName>`.

Schema nodes can also reference named types via `type`:

```yaml
PositiveNumber:
  type: number
  exclusiveMinimum: 0
EyeConfig:
  type: object
  properties:
    agent_physical_radius:
      type: PositiveNumber
```

For simple schema nodes where only `type` is needed, you can use shorthand:

```yaml
BoundsConfig:
  type: object
  properties:
    x_min: number
    radius: number?
    agent_port: Port
```

This is equivalent to:

```yaml
BoundsConfig:
  type: object
  properties:
    x_min:
      type: number
    radius:
      type: number
      optional: true
    agent_port:
      type: Port
```

String enum properties also support shorthand:

```yaml
DerivedMetricSpec:
  type: object
  properties:
    operator: [ema, derivative, rolling_mean, rolling_var, rolling_min, rolling_max]
```

This is equivalent to:

```yaml
DerivedMetricSpec:
  type: object
  properties:
    operator:
      type: string
      enum: [ema, derivative, rolling_mean, rolling_var, rolling_min, rolling_max]
```

When a schema node combines `type: <NamedType>` with additional keywords,
both must pass (logical conjunction / `allOf`-like behavior).

### Constraints (type-local)

Constraints are defined on schema nodes. Top-level path maps
(`schema.constraints`) are not supported.

Each `constraints` value can be:

- a string expression
- a list of string expressions
- a path map (`<relative_path>: expression | [expressions]`)

Type-local constraints are expanded onto each data path that uses that type hint:

```yaml
EpisodeConfig:
  type: object
  properties:
    initial_population_size:
      type: integer
      constraints:
        - "value >= 1"
        - "value <= max_agents"
    max_agents:
      type: integer
      constraints: "value >= 1"
  constraints:
    - "initial_population_size <= max_agents"
```

Inside type-local constraints:

- `value` still targets the current path.
- bare names first resolve from root data, then local scope (`max_agents` above).

## Supported Schema Keywords (v0)

The validator supports this subset:

- common: `type`, `enum`
- numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
- string: `minLength`, `maxLength`, `pattern`
- object: `properties`, `values`, `required`, `optional` (on property schemas)
- array: `items`, `minItems`, `maxItems`

Built-in primitive type names are also accepted directly:

- `string`, `integer`, `number`, `boolean`, `object`, `array`, `null`

Named type references are accepted anywhere `type` appears.

For object schemas, properties are required by default. Mark an individual
property as optional with `optional: true` (or shorthand `property: TypeName?` /
`type: TypeName?`). Legacy `required: [...]` lists are
still accepted.

Typed dictionaries are supported via `values` on object schemas:

```yaml
WorkerProfile:
  type: object
  properties:
    cores: integer
    memory_gb: integer
WorkersByName:
  type: object
  values:
    type: WorkerProfile
```

`WorkersByName` means "any string key is allowed, and each value must match
`WorkerProfile`".

## `data` Section and Type Hints

A data key may carry a type hint suffix:

```yaml
port <Port>: 5432
replicas <integer>: 3
port_from_shared <shared.Port>: 8080
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
super-yaml compile <file> [--pretty] [--format json|yaml|rust|ts|typescript] [--allow-env KEY]...
super-yaml compile <file> [--yaml|--json|--rust|--ts] [--allow-env KEY]...
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
- `--format json|yaml|rust|ts|typescript`: explicit output format
- `--yaml`, `--json`, `--rust`, `--ts`: format shortcuts
- `--allow-env KEY`: allow access to one process environment variable key (repeatable)

## Rust API

```rust,no_run
use super_yaml::{
    compile_document, compile_document_to_json, compile_document_to_yaml, generate_rust_types,
    generate_typescript_types, validate_document, ProcessEnvProvider,
};

fn run(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let env = ProcessEnvProvider;

    let compiled = compile_document(input, &env)?;
    let json = compile_document_to_json(input, &env, true)?;
    let yaml = compile_document_to_yaml(input, &env)?;
    let rust = generate_rust_types(input)?;
    let ts = generate_typescript_types(input)?;
    validate_document(input, &env)?;

    println!("{}", compiled.to_json_string(false)?);
    println!("{}", json);
    println!("{}", yaml);
    println!("{}", rust);
    println!("{}", ts);
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
- `examples/type_composition.syaml`

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
- Rust and TypeScript codegen are first-pass and currently target named top-level schema definitions only (anonymous inline object schemas map to fallback types)
- compilation enforces depth/size guardrails for expressions, constraints, and YAML structures

## License

MIT. See `LICENSE`.
