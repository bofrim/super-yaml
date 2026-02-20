# super_yaml

A Rust crate and CLI that compiles a strict, sectioned YAML dialect (`.syaml`) into resolved JSON or YAML — and generates Rust and TypeScript types from schema definitions.

super_yaml is designed for configuration that should stay declarative but still support computed fields, schema validation, and cross-field constraints. The format gives you enough expressiveness for real-world config (environment bindings, derived values, reusable templates, string constructors) while keeping parsing and behavior predictable.

## Table of Contents

- [super_yaml](#super_yaml)
  - [Table of Contents](#table-of-contents)
  - [Quick Start](#quick-start)
    - [Validate a document](#validate-a-document)
    - [Compile to JSON](#compile-to-json)
    - [Compile to YAML](#compile-to-yaml)
    - [Generate Rust types from schemas](#generate-rust-types-from-schemas)
    - [Generate TypeScript types from schemas](#generate-typescript-types-from-schemas)
  - [Document Structure](#document-structure)
  - [Features](#features)
    - [Schema Definitions and Type Hints](#schema-definitions-and-type-hints)
      - [Type composition](#type-composition)
      - [Property shorthand](#property-shorthand)
      - [Supported schema keywords](#supported-schema-keywords)
    - [Environment Bindings](#environment-bindings)
    - [Expressions and Derived Values](#expressions-and-derived-values)
      - [Built-in functions](#built-in-functions)
      - [Variable sources](#variable-sources)
      - [Dependency resolution](#dependency-resolution)
    - [String Interpolation](#string-interpolation)
    - [Constraints](#constraints)
      - [Single-value constraints](#single-value-constraints)
      - [Cross-field constraints on objects](#cross-field-constraints-on-objects)
      - [Path-mapped constraints](#path-mapped-constraints)
    - [Templates](#templates)
      - [Defining a template](#defining-a-template)
      - [Invoking a template](#invoking-a-template)
    - [Imports](#imports)
      - [Importing a file](#importing-a-file)
      - [Using imported types in schemas](#using-imported-types-in-schemas)
      - [Using imported data](#using-imported-data)
      - [Import behavior](#import-behavior)
    - [String Constructors](#string-constructors)
      - [Defining constructors](#defining-constructors)
      - [Using constructors in data](#using-constructors-in-data)
      - [Advanced constructors: multiple patterns](#advanced-constructors-multiple-patterns)
    - [Typed Dictionaries](#typed-dictionaries)
    - [Private Data Keys](#private-data-keys)
    - [Code Generation](#code-generation)
      - [Rust](#rust)
      - [TypeScript](#typescript)
  - [Use Cases and Patterns](#use-cases-and-patterns)
    - [Service Configuration](#service-configuration)
    - [Pricing and Business Logic](#pricing-and-business-logic)
    - [Infrastructure as Config](#infrastructure-as-config)
    - [Reusable Config Templates](#reusable-config-templates)
  - [CLI Reference](#cli-reference)
    - [`validate`](#validate)
    - [`compile`](#compile)
  - [Rust API](#rust-api)
    - [Custom environment provider](#custom-environment-provider)
    - [File-based compilation](#file-based-compilation)
  - [Compilation Pipeline](#compilation-pipeline)
    - [Error categories](#error-categories)
  - [Mini YAML Subset](#mini-yaml-subset)
  - [VS Code Extension](#vs-code-extension)
  - [Current Limitations](#current-limitations)
  - [Examples Directory](#examples-directory)
  - [Development](#development)
  - [Maintenance Status](#maintenance-status)
  - [License](#license)

## Quick Start

### Validate a document

```bash
cargo run --bin super-yaml -- validate examples/basic.syaml
```

Prints `OK` if the document is valid, or a descriptive error otherwise.

### Compile to JSON

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --pretty
```

### Compile to YAML

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --format yaml
```

### Generate Rust types from schemas

```bash
cargo run --bin super-yaml -- compile examples/type_composition.syaml --format rust
```

### Generate TypeScript types from schemas

```bash
cargo run --bin super-yaml -- compile examples/type_composition.syaml --format ts
```

## Document Structure

Every `.syaml` file starts with a version marker and is organized into three optional sections: `meta`, `schema`, and `data`. The sections can appear in any order, and each section appears at most once.

```yaml
---!syaml/v0
---meta
file:
  owner: platform
  service: billing
env:
  DB_HOST:
    from: env
    key: DB_HOST
    default: localhost

---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535

---data
host <string>: "${env.DB_HOST}"
port <Port>: 5432
```

**Marker** — The first non-empty line must be `---!syaml/v0`. If it's missing or different, parsing fails immediately.

**`meta`** — File-level metadata (`meta.file`), environment variable bindings (`meta.env`), and imports from other `.syaml` files (`meta.imports`).

**`schema`** — Named type definitions used for validation. Each top-level key defines a type that can be referenced from `data` via type hints.

**`data`** — The configuration values. Keys can carry inline type hints (`key <TypeName>`), values can be expressions (`=expr`) or interpolated strings (`${expr}`), and entire subtrees can be stamped out from templates.

## Features

### Schema Definitions and Type Hints

Schemas define named types that are used to validate data values. Data keys reference schemas with the `<TypeName>` suffix syntax.

```yaml
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
ReplicaCount:
  type: integer
  constraints: "value >= 1"

---data
port <Port>: 5432
replicas <ReplicaCount>: 3
```

Built-in primitives (`string`, `integer`, `number`, `boolean`, `object`, `array`, `null`) can be used directly as type hints without defining them in the schema:

```yaml
---data
name <string>: my-service
enabled <boolean>: true
```

#### Type composition

Named types can reference other named types, and additional constraints are applied as a logical conjunction (both must pass):

```yaml
---schema
PositiveNumber:
  type: number
  exclusiveMinimum: 0
DisplayProfile:
  type: object
  properties:
    scale_factor:
      type: PositiveNumber
      maximum: 25
    refresh_hz:
      type: PositiveNumber
```

#### Property shorthand

For object properties where only the type matters, a shorthand syntax avoids the verbose `type:` nesting. Append `?` to mark optional properties, and use inline arrays for string enums:

```yaml
---schema
Service:
  type: object
  properties:
    name: string
    host: string
    port: integer
    tls: boolean
    env: [prod, staging, dev]
    tags: string?
```

This is equivalent to the fully expanded form:

```yaml
---schema
Service:
  type: object
  properties:
    name:
      type: string
    host:
      type: string
    port:
      type: integer
    tls:
      type: boolean
    env:
      type: string
      enum: [prod, staging, dev]
    tags:
      type: string
      optional: true
```

#### Supported schema keywords

| Category | Keywords                                                       |
| -------- | -------------------------------------------------------------- |
| Common   | `type`, `enum`                                                 |
| Numeric  | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`   |
| String   | `minLength`, `maxLength`, `pattern`                            |
| Object   | `properties`, `values`, `required`, `optional`, `constructors` |
| Array    | `items`, `minItems`, `maxItems`                                |

Properties are required by default. Mark individual properties as optional with `optional: true` or the `?` shorthand. Legacy `required: [...]` lists are also accepted.

### Environment Bindings

Environment variables are declared in `meta.env` and referenced in data via `env.KEY`. Each binding maps a symbol to a process environment variable with optional defaults.

```yaml
---meta
env:
  DB_HOST:
    from: env
    key: DB_HOST
    required: true
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 4

---data
host <string>: "${env.DB_HOST}"
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
```

Resolution order:

1. Read the environment variable named by `key`.
2. If present, parse the string value as a scalar.
3. If absent and `default` exists, use the default.
4. If absent and `required: true` (the default), fail with an error.
5. If absent and `required: false`, produce `null`.

By default, the CLI blocks all environment access. Use `--allow-env KEY` to explicitly permit each variable:

```bash
super-yaml compile config.syaml --allow-env DB_HOST --allow-env CPU_CORES
```

### Expressions and Derived Values

Values prefixed with `=` are evaluated as expressions, enabling computed configuration fields. Expressions can reference other data keys, environment symbols, and call built-in functions.

```yaml
---data
replicas <integer>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <integer>: "=replicas * worker_threads * 25"
```

Expressions support standard arithmetic (`+ - * / %`), comparison (`== != < <= > >=`), boolean logic (`&& || !`), and grouping with parentheses.

#### Built-in functions

| Function              | Description                        |
| --------------------- | ---------------------------------- |
| `min(x, ...)`         | Minimum of arguments               |
| `max(x, ...)`         | Maximum of arguments               |
| `abs(x)`              | Absolute value                     |
| `floor(x)`            | Floor to integer                   |
| `ceil(x)`             | Ceiling to integer                 |
| `round(x)`            | Round to nearest integer           |
| `len(x)`              | Length of string, array, or object |
| `coalesce(a, b, ...)` | First non-null argument            |

#### Variable sources

- **Data references**: `replicas`, `service.port`, `inventory.daily_demand` — dot-separated paths into the data tree
- **Environment symbols**: `env.CPU_CORES`, `env.DB_HOST`
- **Constraint target**: `value` (only inside constraint expressions)

#### Dependency resolution

Derived values are resolved in multiple passes. If value `A` depends on value `B`, `B` is resolved first. If no forward progress can be made (circular dependency), compilation fails with a cycle error listing the unresolved paths.

### String Interpolation

Strings containing `${...}` segments are interpolated. Each segment is evaluated as an expression and substituted into the string.

```yaml
---data
region <string>: us-east-1
http_port <integer>: 8080
public_url <string>: "https://${region}.example.internal:${http_port}"
dsn <string>: "postgres://${env.DB_HOST}:${port}/app"
```

If the entire string is a single interpolation (`"${expr}"`), the result preserves the native type of the expression. Otherwise, all parts are concatenated into a string.

### Constraints

Constraints are boolean expressions defined on schema types that are evaluated against every data value using that type. They provide cross-field validation that goes beyond what static schema keywords can express.

#### Single-value constraints

Constraints on primitive types use `value` to reference the current field:

```yaml
---schema
ReplicaCount:
  type: integer
  constraints: "value >= 1"
```

Multiple constraints can be specified as a list — all must pass:

```yaml
---schema
PoolSize:
  type: integer
  constraints:
    - "value >= 1"
    - "value <= 1000000"
```

#### Cross-field constraints on objects

Object-level constraints can reference child properties by name, enabling cross-field validation:

```yaml
---schema
Booking:
  type: object
  properties:
    name: string
    seat_limit: integer
    requested_seats: integer
  constraints:
    - "requested_seats <= seat_limit"
```

#### Path-mapped constraints

For finer control, constraints can be organized as a path map. Use `$` to target the object itself:

```yaml
---schema
InventoryConfig:
  type: object
  properties:
    reorder_point:
      type: integer
    target_stock:
      type: integer
    lead_days:
      type: integer
  constraints:
    reorder_point:
      - "value >= 1"
    lead_days:
      - "value <= 30"
    $:
      - "target_stock >= reorder_point"
```

### Templates

Templates let you define reusable configuration shapes with placeholder variables, then stamp out concrete instances by providing values for those variables.

#### Defining a template

Templates are regular data objects with `{{VAR}}` placeholders. Use `{{VAR:default}}` for optional placeholders with defaults:

```yaml
---data
_templates:
  service:
    name: "{{NAME}}"
    host: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
    env: "{{ENV}}"
```

#### Invoking a template

Use the template path as a key, and pass a mapping of variable bindings as the value:

```yaml
---data
api_service <Service>:
  {{_templates.service}}:
    NAME: api-service
    HOST: api.internal
    ENV: prod
```

This resolves to `{name: "api-service", host: "api.internal", port: 8080, tls: false, env: "prod"}`.

Template rules:

- The template invocation key must be the only key in the object.
- All required placeholders (`{{VAR}}`) must be provided.
- Unknown variables are rejected.
- Resolved values are validated against schema and type hints.

Templates combine naturally with [private data keys](#private-data-keys) (prefix `_templates` to keep them out of compiled output) and [imports](#imports) (reference templates from external files).

### Imports

The import system lets you compose multiple `.syaml` files. An imported file's schema types and data become available under a namespace alias.

#### Importing a file

```yaml
---meta
imports:
  shared: ./shared.syaml
```

#### Using imported types in schemas

Imported types are referenced as `alias.TypeName`:

```yaml
---schema
Service:
  type: object
  properties:
    host: string
    port:
      type: shared.Port
```

#### Using imported data

Imported data is available for expressions and interpolation:

```yaml
---data
service_host <string>: "${shared.defaults.host}"
proxy_port <shared.Port>: "=shared.defaults.port + 100"
```

You can also extract entire subtrees from imports by referencing them as plain values:

```yaml
---data
shared_defaults: shared.defaults
```

#### Import behavior

- Imported files run their own complete compilation pipeline (env, expressions, validation).
- Schema types are mounted under `<alias.TypeName>`.
- Private data keys (prefixed with `_`) from imported files are not exposed.
- Cyclic imports are detected and rejected.
- Relative paths resolve from the importing file's directory.

### String Constructors

String constructors let you write compact string representations of structured objects. A regex pattern matches the string and maps capture groups to object properties, expanding the string into a full object at compile time.

#### Defining constructors

Constructors are defined on object schemas:

```yaml
---schema
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: [MiB, GiB, TiB]
  constructors:
    from_text:
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount:
          group: amount
          decode: integer
        unit:
          group: raw_unit
          from_enum: MemoryUnit
```

#### Using constructors in data

When a string value matches a constructor pattern for its type-hinted type, it's automatically expanded:

```yaml
---data
memory <MemorySpec>: 16GiB
disk_size <DiskSizeSpec>: 512GB
```

The string `16GiB` is compiled into `{amount: 16, unit: "GiB"}`.

#### Advanced constructors: multiple patterns

A type can have multiple constructors with priority ordering. Lower `order` values are tried first:

```yaml
---schema
Color:
  type: object
  properties:
    red: integer
    green: integer
    blue: integer
    alpha: number?
  constructors:
    rgb:
      order: 1
      regex: '^rgb\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+)\)$'
      defaults:
        alpha: 1
    hex:
      order: 2
      regex: '^#(?<red_hex>[0-9A-Fa-f]{2})(?<green_hex>[0-9A-Fa-f]{2})(?<blue_hex>[0-9A-Fa-f]{2})$'
      map:
        red: { group: red_hex, decode: hex_u8 }
        green: { group: green_hex, decode: hex_u8 }
        blue: { group: blue_hex, decode: hex_u8 }
      defaults:
        alpha: 1

---data
accent_rgb <Color>: "rgb(10, 20, 30)"
accent_hex <Color>: "#0A141E"
```

Both resolve to structured `{red, green, blue, alpha}` objects.

**Constructor rules:**

- Constructors only apply to object types.
- Each constructor must declare `regex`; `map`, `defaults`, and `order` are optional.
- By default, capture group names map directly to property names. Use `map` to override the source group or apply a decoder.
- Available decoders: `auto`, `string`, `integer`, `number`, `boolean`, `hex_u8`, `hex_alpha`.
- Use `from_enum` on a map rule to validate against a named string enum type (mutually exclusive with `decode`).
- When multiple constructors match, the lowest `order` wins. Ambiguous matches are errors.
- `defaults` fills properties not captured by the regex.

### Typed Dictionaries

Object schemas with `values` instead of (or in addition to) `properties` define typed dictionaries — objects where any string key is allowed, and each value must match the specified type:

```yaml
---schema
WorkerProfile:
  type: object
  properties:
    cores: integer
    memory_gb: integer
WorkersByName:
  type: object
  values:
    type: WorkerProfile

---data
workers <WorkersByName>:
  api:
    cores: 4
    memory_gb: 16
  batch:
    cores: 8
    memory_gb: 32
```

This is useful for maps where the keys are dynamic (worker names, region codes, etc.) but the value shape is fixed.

### Private Data Keys

Top-level data keys prefixed with `_` are private. They are available for template resolution, expressions, and other internal references within the same file, but are stripped from the final compiled output and are not visible to importing files.

```yaml
---data
_base_port <integer>: 7000
grpc_port <integer>: "=_base_port"
http_port <integer>: "=_base_port + 1"
```

Compiled output contains only `grpc_port` and `http_port`. This is particularly useful for templates — define templates under a `_templates` key to keep them out of the compiled config while still using them for stamping out data.

### Code Generation

super_yaml can generate Rust structs and TypeScript interfaces from schema definitions, giving you type-safe access to your configuration in application code.

#### Rust

```bash
super-yaml compile config.syaml --format rust
```

Generates `struct` definitions with `serde::Serialize` and `serde::Deserialize` derives, enum types for string enums, and type aliases for constrained primitives.

#### TypeScript

```bash
super-yaml compile config.syaml --format ts
```

Generates TypeScript `interface` definitions, string union types for enums, and type aliases.

Code generation targets named top-level schema definitions. Anonymous inline object schemas fall back to generic types.

## Use Cases and Patterns

### Service Configuration

Define ports, replicas, and derived scaling parameters with environment-driven overrides:

```yaml
---!syaml/v0
---meta
env:
  REGION:
    from: env
    key: REGION
    default: us-east-1
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 6
  BASE_PORT:
    from: env
    key: BASE_PORT
    default: 7000

---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
Replicas:
  type: integer
  constraints: "value >= 1"

---data
service_name <string>: billing
region <string>: "${env.REGION}"
replicas <Replicas>: 3
worker_threads <integer>: "=max(replicas, env.CPU_CORES * 2)"
grpc_port <Port>: "${env.BASE_PORT}"
http_port <Port>: "=grpc_port + 1"
public_url <string>: "https://${region}.example.internal:${http_port}"
```

Expressions ensure worker threads scale with CPU cores, port offsets are always consistent, and URLs are assembled from resolved components.

### Pricing and Business Logic

Encode pricing rules with constraints that prevent invalid states:

```yaml
---!syaml/v0
---meta
env:
  TAX_RATE:
    from: env
    key: TAX_RATE
    default: 0.0825
  DISCOUNT_RATE:
    from: env
    key: DISCOUNT_RATE
    default: 0.15

---schema
Money:
  type: number
  minimum: 0
Percent:
  type: number
  minimum: 0
  maximum: 1

---data
subtotal <Money>: 240
discount_rate <Percent>: "${env.DISCOUNT_RATE}"
tax_rate <Percent>: "${env.TAX_RATE}"
discount_amount <Money>: "=round(subtotal * discount_rate)"
taxable_subtotal <Money>: "=subtotal - discount_amount"
tax_amount <Money>: "=round(taxable_subtotal * tax_rate)"
final_total <Money>: "=taxable_subtotal + tax_amount"
summary <string>: "USD ${final_total}"
```

Every intermediate value is computed, typed, and constrained. Change the environment variables and the entire chain recalculates.

### Infrastructure as Config

Combine string constructors, typed dictionaries, and pattern validation for infrastructure definitions:

```yaml
---!syaml/v0
---schema
CpuCount:
  type: integer
  minimum: 1
  maximum: 128
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: [MiB, GiB, TiB]
  constructors:
    from_text:
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount: { group: amount, decode: integer }
        unit: { group: raw_unit, from_enum: MemoryUnit }
DiskSizeSpec:
  type: object
  properties:
    amount: integer
    unit: [GB, TB, PB]
  constructors:
    from_text:
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount: { group: amount, decode: integer }
        unit: { group: raw_unit, from_enum: DiskSizeUnit }
VmNetwork:
  type: object
  properties:
    subnet:
      type: string
      pattern: '^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}/\d{1,2}$'
    assign_public_ip: boolean

---data
resource:
  vm_name: api-prod-01
  cpu <CpuCount>: 4
  memory <MemorySpec>: 16GiB
  disks:
    root:
      size <DiskSizeSpec>: 120GB
      disk_kind: ssd
    data:
      size <DiskSizeSpec>: 512GB
      disk_kind: ssd
  network:
    subnet: 10.40.2.0/24
    assign_public_ip: false
```

String constructors turn `16GiB` and `512GB` into structured objects, pattern validation ensures subnet format, and numeric ranges bound CPU counts — all at compile time.

### Reusable Config Templates

Define a template shape once (locally or in an imported file) and stamp out multiple instances with different variable bindings:

**shared.syaml:**

```yaml
---!syaml/v0
---schema
Service:
  type: object
  properties:
    name: string
    host: string
    port: integer
    tls: boolean
    env: [prod, staging, dev]

---data
templates:
  service:
    name: "{{NAME}}"
    host: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
    env: "{{ENV}}"
```

**main.syaml:**

```yaml
---!syaml/v0
---meta
imports:
  tpl: ./shared.syaml

---data
service <tpl.Service>:
  {{tpl.templates.service}}:
    NAME: api-service
    HOST: api.internal
    ENV: prod
```

Templates with constraints also work — define a `Booking` type with cross-field constraints, write a template that fixes some fields, and each instantiation is validated:

```yaml
---schema
Booking:
  type: object
  properties:
    name: string
    seat_limit: integer
    requested_seats: integer
  constraints:
    - "requested_seats <= seat_limit"

---data
_templates:
  booking:
    name: "{{NAME}}"
    seat_limit: 100
    requested_seats: "{{REQUESTED_SEATS}}"

strict_booking <Booking>:
  {{_templates.booking}}:
    NAME: Aurora Book Club
    REQUESTED_SEATS: 3

flex_booking <Booking>:
  {{_templates.booking}}:
    NAME: Harbor Tech Meetup
    REQUESTED_SEATS: 20
```

## CLI Reference

```text
super-yaml validate <file> [--allow-env KEY]...
super-yaml compile <file> [--pretty] [--format json|yaml|rust|ts|typescript] [--allow-env KEY]...
super-yaml compile <file> [--yaml|--json|--rust|--ts] [--allow-env KEY]...
```

### `validate`

Runs the full compilation pipeline and prints `OK` on success. Environment access is disabled unless keys are explicitly allowed with `--allow-env`.

### `compile`

Compiles the document and emits resolved output. Defaults to compact JSON.

| Option                                      | Description                                                 |
| ------------------------------------------- | ----------------------------------------------------------- |
| `--pretty`                                  | Pretty-print JSON output                                    |
| `--format json\|yaml\|rust\|ts\|typescript` | Output format                                               |
| `--yaml`, `--json`, `--rust`, `--ts`        | Format shortcuts                                            |
| `--allow-env KEY`                           | Allow access to a process environment variable (repeatable) |

## Rust API

```rust
use super_yaml::{
    compile_document, compile_document_to_json, compile_document_to_yaml,
    generate_rust_types, generate_typescript_types,
    validate_document, ProcessEnvProvider,
};

fn run(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let env = ProcessEnvProvider;

    // Full compilation
    let compiled = compile_document(input, &env)?;
    println!("{}", compiled.to_json_string(false)?);

    // Direct serialization
    let json = compile_document_to_json(input, &env, true)?;
    let yaml = compile_document_to_yaml(input, &env)?;

    // Code generation (schema only, no env needed)
    let rust = generate_rust_types(input)?;
    let ts = generate_typescript_types(input)?;

    // Validation-only (no output)
    validate_document(input, &env)?;

    Ok(())
}
```

### Custom environment provider

For testing or embedded use, supply a map-based provider instead of reading from the process environment:

```rust
use std::collections::HashMap;
use super_yaml::MapEnvProvider;

let mut vars = HashMap::new();
vars.insert("CPU_CORES".to_string(), "8".to_string());
let env = MapEnvProvider::new(vars);
```

### File-based compilation

For documents with imports, use the `_from_path` variants so that relative import paths resolve correctly:

```rust
use super_yaml::{compile_document_from_path, ProcessEnvProvider};

let compiled = compile_document_from_path("config/main.syaml", &ProcessEnvProvider)?;
```

## Compilation Pipeline

`compile_document` runs these steps in order:

1. **Scan marker and sections** — validate `---!syaml/v0` and parse section fences.
2. **Parse section bodies** — run the mini YAML parser on each section.
3. **Parse schema and normalize type hints** — extract `<TypeName>` annotations from data keys.
4. **Extract explicit import references** — resolve bare import path references in data values.
5. **Expand templates** — substitute `{{VAR}}` placeholders from template invocations.
6. **Resolve environment bindings** — read and parse `env.*` values.
7. **Resolve expressions and interpolations** — evaluate `=expr` and `${expr}` with multi-pass dependency resolution.
8. **Coerce string constructors** — match type-hinted string values against constructor regexes and expand to objects.
9. **Validate type hints** — check resolved values against their schema types.
10. **Validate constraints** — evaluate constraint expressions against resolved data.

If any step fails, compilation stops with a `SyamlError`.

### Error categories

| Error                | Cause                                        |
| -------------------- | -------------------------------------------- |
| `MarkerError`        | Missing or invalid `---!syaml/v0`            |
| `SectionError`       | Unknown section, duplicate section           |
| `YamlParseError`     | Syntax error in a section body               |
| `SchemaError`        | Invalid schema definition                    |
| `TypeHintError`      | Invalid or mismatched type hint              |
| `ExpressionError`    | Failed expression evaluation                 |
| `ConstraintError`    | Constraint expression returned false         |
| `EnvError`           | Missing required environment variable        |
| `CycleError`         | Circular dependency between derived values   |
| `ImportError`        | Failed import (file not found, cyclic, etc.) |
| `TemplateError`      | Missing template variable, unknown variable  |
| `SerializationError` | JSON/YAML serialization failure              |
| `Io`                 | File system error                            |

## Mini YAML Subset

The internal parser intentionally supports a constrained subset of YAML — not full YAML 1.2:

- Mappings (`key: value`)
- Sequences (`- item`)
- Nested indentation (spaces only)
- Inline objects (`{a: 1, b: 2}`)
- Inline arrays (`[1, 2, 3]`)
- Scalars: numbers, booleans, null, strings
- Quoted strings with escapes
- Comments (`# ...`) in supported positions

## VS Code Extension

The repository includes a VS Code extension (`vscode-syaml/`) providing syntax highlighting and parser-backed diagnostics for `.syaml` files.

## Current Limitations

- Only `v0` marker is accepted.
- Only `from: env` bindings are supported.
- Expression variable paths are dot-based object traversal.
- Parser is a YAML subset, not full YAML.
- Rust and TypeScript codegen are first-pass and currently target named top-level schema definitions only (anonymous inline object schemas map to fallback types).
- Compilation enforces depth/size guardrails for expressions, constraints, and YAML structures.

## Examples Directory

The repository ships with sample `.syaml` inputs and expected JSON outputs covering each feature:

| Example                     | Features demonstrated                                          |
| --------------------------- | -------------------------------------------------------------- |
| `basic.syaml`               | Environment bindings, expressions, constraints, type hints     |
| `service_scaling.syaml`     | Service config with computed scaling and URLs                  |
| `pricing_engine.syaml`      | Multi-step arithmetic, percentage types                        |
| `inventory_policy.syaml`    | Path-mapped constraints, `coalesce()`, nested expressions      |
| `alert_rules.syaml`         | Array types, `len()`, `minItems`                               |
| `type_composition.syaml`    | Named type references, nested objects                          |
| `color_constructors.syaml`  | Multiple string constructors with hex decoding                 |
| `vm_resource.syaml`         | Constructors with `from_enum`, pattern validation, typed dicts |
| `typed_dict.syaml`          | `values`-based object schemas                                  |
| `template_service.syaml`    | Cross-file template invocation with imports                    |
| `template_constraint.syaml` | Templates with constraint validation, private data keys        |
| `imported_types.syaml`      | Importing types and data from another file                     |

Each has a matching `examples/*.expected.json` for reference.

## Development

Run tests:

```bash
cargo test
```

Format and lint:

```bash
cargo fmt
cargo clippy --all-targets --all-features
```

## Maintenance Status

This is currently a personal project and is not actively maintained for external users. Issues and pull requests are welcome, but may not receive a response.

## License

MIT. See `LICENSE`.
