# super_yaml

`super_yaml` is a Rust implementation of a YAML-inspired declarative format with section fences, typed keys, derived expressions, constraints, and environment bindings.

## v0 Syntax

The first non-empty line must be:

```yaml
---!syaml/v0
```

Section fences are strict and must be in this order:

1. `---front_matter` (optional)
2. `---schema` (required)
3. `---data` (required)

Example:

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

## CLI

Validate a document:

```bash
cargo run --bin super-yaml -- validate examples/basic.syaml
```

Compile to resolved JSON:

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --pretty
```

Compile to resolved YAML:

```bash
cargo run --bin super-yaml -- compile examples/basic.syaml --format yaml
```

## Library API

```rust
use super_yaml::{
    compile_document, compile_document_to_json, compile_document_to_yaml, validate_document,
    ProcessEnvProvider,
};

let env = ProcessEnvProvider;
let compiled = compile_document(input, &env)?;
let as_json = compile_document_to_json(input, &env, true)?;
let as_yaml = compile_document_to_yaml(input, &env)?;
validate_document(input, &env)?;
```

## Supported Schema Subset (v0)

- `type`, `enum`
- numeric: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`
- string: `minLength`, `maxLength`, `pattern`
- object: `properties`, `required`
- array: `items`, `minItems`, `maxItems`

## More Sample Use Cases

- `examples/basic.syaml`: baseline env + expressions + constraints
- `examples/service_scaling.syaml`: service sizing, ports, URL interpolation
- `examples/pricing_engine.syaml`: money math, percentages, rounding, summary strings
- `examples/inventory_policy.syaml`: nested objects, dotted references, nested constraints
- `examples/alert_rules.syaml`: array validation and `len(...)`-driven derived values

Each sample has an expected compiled output JSON in `examples/*.expected.json`.

Try one:

```bash
cargo run --bin super-yaml -- compile examples/service_scaling.syaml --pretty
```
