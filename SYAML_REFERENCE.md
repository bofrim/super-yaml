# Super YAML — LLM Agent Reference

You are working with **super_yaml**, a configuration language and toolchain. This document is your complete reference for reading, writing, and integrating `.syaml` files. Follow these rules precisely when generating `.syaml` content.

## What super_yaml Does

super_yaml compiles `.syaml` files into resolved **JSON** or **YAML** output. It can also generate **Rust structs** and **TypeScript interfaces** from schema definitions.

A `.syaml` file combines:
- A **schema** that defines and validates data shapes
- **Data** with inline type hints, computed expressions, and string interpolation
- **Environment bindings** that inject external values at compile time
- **Templates** for stamping out repeated structures
- **Imports** for composing multiple files
- **String constructors** that expand shorthand strings into structured objects

The CLI compiles `.syaml` into output artifacts:

```bash
# Validate only (prints OK or error)
super-yaml validate config.syaml

# Compile to JSON (default)
super-yaml compile config.syaml --pretty

# Compile to YAML
super-yaml compile config.syaml --format yaml

# Generate Rust types from schemas
super-yaml compile config.syaml --format rust

# Generate TypeScript types from schemas
super-yaml compile config.syaml --format ts

# Allow environment variable access (blocked by default)
super-yaml compile config.syaml --allow-env DB_HOST --allow-env CPU_CORES
```

---

## Document Structure

Every `.syaml` file has this structure:

```yaml
---!syaml/v0
---meta
# Environment bindings, imports, file metadata (optional section)

---schema
# Named type definitions (optional section)

---data
# Configuration values (optional section)
```

### Rules

1. The first non-empty line **must** be exactly `---!syaml/v0`.
2. Sections are opened with `---meta`, `---schema`, or `---data`.
3. All three sections are optional. They can appear in any order. Each section can appear at most once.
4. When omitted, `schema` and `data` default to empty objects.

### Minimal valid document

```yaml
---!syaml/v0
---data
name: hello
```

---

## The `meta` Section

The `meta` section has three optional subsections: `file`, `env`, and `imports`.

### `meta.file` — File-level metadata

Arbitrary key-value pairs describing the file. Not used by the compiler, but available for tooling.

```yaml
---meta
file:
  owner: platform-team
  service: billing
  revision: 7
```

### `meta.env` — Environment variable bindings

Declares environment variables that can be referenced in data as `env.SYMBOL_NAME`.

```yaml
---meta
env:
  # Required variable — compilation fails if not set
  DB_HOST:
    from: env
    key: DB_HOST
    required: true

  # Variable with a default — uses 4 if CPU_CORES is not set
  CPU_CORES:
    from: env
    key: CPU_CORES
    default: 4

  # Optional variable — produces null if not set
  OPTIONAL_FLAG:
    from: env
    key: OPTIONAL_FLAG
    required: false
```

Each binding has these fields:
- `from`: Must be `env` (the only supported source).
- `key`: The name of the process environment variable to read.
- `required`: Whether the variable must be set. Defaults to `true`.
- `default`: Fallback value when the variable is not set.

Resolution order:
1. Read the environment variable named by `key`.
2. If present, parse the string as a YAML scalar (number, boolean, string, etc.).
3. If absent and `default` is set, use the default.
4. If absent and `required: true`, fail.
5. If absent and `required: false`, produce `null`.

### `meta.imports` — Import other `.syaml` files

Loads external `.syaml` files under a namespace alias.

```yaml
---meta
imports:
  # Short form: alias -> path
  shared: ./shared_types.syaml

  # Long form: alias -> object with path
  infra:
    path: ./infra/common.syaml
```

Relative paths resolve from the importing file's directory. Cyclic imports are detected and rejected.

---

## The `schema` Section

The schema section defines named types. Each top-level key is a type name. These types are used to validate data values via type hints.

### Primitive types

```yaml
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535

Percentage:
  type: number
  minimum: 0
  maximum: 1

ServiceName:
  type: string
  minLength: 1
  maxLength: 63
  pattern: '^[a-z][a-z0-9-]*$'
```

Built-in primitive type names: `string`, `integer`, `number`, `boolean`, `object`, `array`, `null`.

### String enums

Define an inline enum as a top-level type:

```yaml
---schema
Environment: [prod, staging, dev]
```

Or within a property:

```yaml
---schema
Config:
  type: object
  properties:
    env: [prod, staging, dev]
```

Both expand to `{ type: string, enum: [...] }`.

### Object types

```yaml
---schema
DatabaseConfig:
  type: object
  properties:
    host:
      type: string
    port:
      type: integer
      minimum: 1
      maximum: 65535
    max_connections:
      type: integer
      constraints: "value >= 1"
    ssl:
      type: boolean
      optional: true
```

Properties are **required by default**. Mark individual properties as optional with `optional: true`.

#### Property shorthand

When a property only needs a type (no extra constraints), use the shorthand form:

```yaml
---schema
DatabaseConfig:
  type: object
  properties:
    host: string          # shorthand for { type: string }
    port: integer         # shorthand for { type: integer }
    ssl: boolean?         # shorthand for { type: boolean, optional: true }
    env: [prod, staging]  # shorthand for { type: string, enum: [prod, staging] }
    port_type: Port       # shorthand for { type: Port } (references named type)
```

### Type composition

Named types can reference other named types via `type`. When combined with additional keywords, both the referenced type's constraints and the local keywords must pass:

```yaml
---schema
PositiveNumber:
  type: number
  exclusiveMinimum: 0

# References PositiveNumber and adds an upper bound
SmallPositive:
  type: PositiveNumber
  maximum: 100

AgentConfig:
  type: object
  properties:
    radius:
      type: PositiveNumber  # reuse in a property
      maximum: 25
    speed:
      type: PositiveNumber
```

### Array types

```yaml
---schema
RuleList:
  type: array
  items:
    type: string
  minItems: 1
  maxItems: 100
```

### Typed dictionaries (dynamic-key objects)

Use `values` for objects where any string key is allowed, but each value must match a type:

```yaml
---schema
WorkerProfile:
  type: object
  properties:
    cores: integer
    memory_gb: integer

# Any key maps to a WorkerProfile
WorkersByName:
  type: object
  values:
    type: WorkerProfile
```

Usage in data:

```yaml
---data
workers <WorkersByName>:
  api:
    cores: 4
    memory_gb: 16
  batch:
    cores: 8
    memory_gb: 32
```

### Constraints

Constraints are boolean expressions attached to schema types. They are evaluated at compile time against every data value that uses that type.

#### Primitive constraints

Use `value` to reference the current field:

```yaml
---schema
ReplicaCount:
  type: integer
  constraints: "value >= 1"

# Multiple constraints (all must pass)
PopulationSize:
  type: integer
  constraints:
    - "value >= 1"
    - "value <= 1000000"
```

#### Object-level cross-field constraints

Reference child properties by name:

```yaml
---schema
DateRange:
  type: object
  properties:
    start_day: integer
    end_day: integer
  constraints:
    - "end_day >= start_day"
```

#### Path-mapped constraints

Organize constraints by target path. Use `$` for constraints on the object itself:

```yaml
---schema
InventoryConfig:
  type: object
  properties:
    reorder_point: integer
    target_stock: integer
    lead_days: integer
  constraints:
    reorder_point:
      - "value >= 1"
    lead_days:
      - "value <= 30"
    $:
      - "target_stock >= reorder_point"
```

### String constructors

String constructors let compact string values be expanded into structured objects at compile time. They are defined on object schemas.

```yaml
---schema
MemoryUnit: [MiB, GiB, TiB]
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: MemoryUnit
  constructors:
    from_text:
      # Regex with named capture groups
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount:
          group: amount     # capture group name
          decode: integer   # parse as integer
        unit:
          group: raw_unit
          from_enum: MemoryUnit  # validate against enum
```

With this constructor, writing `memory <MemorySpec>: 16GiB` in data compiles to `{ amount: 16, unit: "GiB" }`.

#### Multiple constructors with priority

Use `order` to set priority (lower wins). If omitted, constructors are unordered (exactly one must match):

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
    rgba:
      order: 1
      regex: '^rgba\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+),\s*(?<alpha>0|1|0?\.\d+)\)$'
    hex:
      order: 2
      regex: '^#(?<red_hex>[0-9A-Fa-f]{2})(?<green_hex>[0-9A-Fa-f]{2})(?<blue_hex>[0-9A-Fa-f]{2})$'
      map:
        red: { group: red_hex, decode: hex_u8 }
        green: { group: green_hex, decode: hex_u8 }
        blue: { group: blue_hex, decode: hex_u8 }
      defaults:
        alpha: 1
```

Available decoders: `auto`, `string`, `integer`, `number`, `boolean`, `hex_u8`, `hex_alpha`.

Use `from_enum: TypeName` to validate a captured string against a named enum type (mutually exclusive with `decode`).

`defaults` fills properties not matched by capture groups.

### Complete schema keyword reference

| Category | Keywords |
|----------|----------|
| Common | `type`, `enum` |
| Numeric | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum` |
| String | `minLength`, `maxLength`, `pattern` |
| Object | `properties`, `values`, `required`, `optional`, `constructors` |
| Array | `items`, `minItems`, `maxItems` |
| Validation | `constraints` |

---

## The `data` Section

The data section contains the actual configuration values. After compilation, this section becomes the output JSON/YAML.

### Type hints

Attach a type hint to any data key using `<TypeName>` syntax. The value is validated against that schema type at compile time.

```yaml
---data
port <Port>: 5432
name <string>: my-service
replicas <ReplicaCount>: 3
# Imported types use the alias prefix
proxy_port <shared.Port>: 8080
```

The `<TypeName>` is stripped from the key in output — `port <Port>: 5432` produces `{ "port": 5432 }` in JSON.

Type hints can appear at any depth:

```yaml
---data
resource <VmResource>:
  vm_name: api-prod-01
  cpu <CpuCount>: 4
  memory <MemorySpec>: 16GiB
```

### Expressions (derived values)

String values starting with `=` are evaluated as expressions:

```yaml
---data
replicas <integer>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <integer>: "=replicas * worker_threads * 25"
```

Expressions support:
- **Arithmetic**: `+ - * / %`
- **Comparison**: `== != < <= > >=`
- **Boolean**: `&& || !`
- **Grouping**: `( ... )`
- **Functions**: `min()`, `max()`, `abs()`, `floor()`, `ceil()`, `round()`, `len()`, `coalesce()`

Variable sources:
- Data paths: `replicas`, `service.port`, `inventory.daily_demand`
- Environment: `env.CPU_CORES`, `env.DB_HOST`
- Imported data: `shared.defaults.port`
- Constraint target: `value` (only in constraint expressions)

Expressions are resolved in dependency order across multiple passes. Circular dependencies are detected and rejected.

### String interpolation

Strings containing `${...}` segments are interpolated:

```yaml
---data
region <string>: us-east-1
http_port <integer>: 8080
public_url <string>: "https://${region}.example.internal:${http_port}"
dsn <string>: "postgres://${env.DB_HOST}:${port}/app"
```

If the entire string is exactly one interpolation (`"${expr}"`), the result preserves the native type. Otherwise, all segments are concatenated into a string.

### Private keys

Top-level data keys starting with `_` are private. They are available for internal references (templates, expressions) but are removed from compiled output and hidden from importing files.

```yaml
---data
_base_port <integer>: 7000

# These are in the output
grpc_port <integer>: "=_base_port"
http_port <integer>: "=_base_port + 1"
```

Compiled output: `{ "grpc_port": 7000, "http_port": 7001 }` — no `_base_port`.

### Templates

Templates define reusable configuration shapes with placeholder variables.

#### Defining templates

Use `{{VAR}}` for required placeholders and `{{VAR:default}}` for optional ones with defaults:

```yaml
---data
# Private key keeps templates out of output
_templates:
  service:
    name: "{{NAME}}"
    host: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
    env: "{{ENV}}"
```

#### Invoking templates

Use the template path as the only key in an object, and pass variable bindings as the value:

```yaml
---data
api_service <Service>:
  {{_templates.service}}:
    NAME: api-service
    HOST: api.internal
    ENV: prod
```

This resolves to:

```json
{
  "api_service": {
    "name": "api-service",
    "host": "api.internal",
    "port": 8080,
    "tls": false,
    "env": "prod"
  }
}
```

Rules:
- The template invocation key (`{{path}}`) must be the **only** key in that object.
- All required variables must be provided. Unknown variables are rejected.
- After template expansion, type hints and constraints are validated.

#### Templates from imported files

```yaml
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

---

## Imports

Imports compose multiple `.syaml` files into a single compilation.

### Declaring imports

```yaml
---meta
imports:
  shared: ./shared_types.syaml
```

### What gets imported

**Schema types** are mounted under the alias namespace:

```yaml
---schema
# Use imported type in a property definition
MyService:
  type: object
  properties:
    port:
      type: shared.Port  # references Port from shared_types.syaml
```

**Data** is available for expressions, interpolation, and template references:

```yaml
---data
host <string>: "${shared.defaults.host}"
proxy_port <shared.Port>: "=shared.defaults.port + 100"
```

**Entire subtrees** can be extracted by referencing an import path as a plain value:

```yaml
---data
all_defaults: shared.defaults
```

### Import rules

- Each imported file runs its own full compilation pipeline.
- Private keys (`_`-prefixed) in imported files are not accessible.
- Cyclic imports are detected and produce an error.
- Paths are relative to the importing file's directory.
- Schema types must be referenced with the alias prefix (`shared.Port`, not `Port`).

---

## Compilation Output

### JSON output (default)

Given this input:

```yaml
---!syaml/v0
---meta
env:
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
  constraints: "value >= 1"

---data
host <string>: "${env.DB_HOST}"
port <Port>: 5432
replicas <ReplicaCount>: 3
worker_threads <integer>: "=max(2, env.CPU_CORES * 2)"
max_connections <MaxConnections>: "=replicas * worker_threads * 25"
```

`super-yaml compile --pretty` produces:

```json
{
  "host": "localhost",
  "max_connections": 600,
  "port": 5432,
  "replicas": 3,
  "worker_threads": 8
}
```

Type hints are stripped, expressions are resolved, constraints are validated — the output is pure data.

### YAML output

`super-yaml compile --format yaml` produces:

```yaml
host: localhost
max_connections: 600
port: 5432
replicas: 3
worker_threads: 8
```

### Rust code generation

`super-yaml compile --format rust` reads the **schema** section and generates Rust types:

Given this schema:

```yaml
---schema
PositiveNumber:
  type: number
  exclusiveMinimum: 0
StereoVisionEye:
  type: object
  properties:
    agent_physical_radius:
      type: PositiveNumber
      maximum: 25
    baseline:
      type: PositiveNumber
```

It generates:

```rust
use serde::{Deserialize, Serialize};

pub type PositiveNumber = f64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StereoVisionEye {
    pub agent_physical_radius: PositiveNumber,
    pub baseline: PositiveNumber,
}
```

Richer schemas produce richer output. Enums become Rust enums with serde rename attributes. Optional properties become `Option<T>` with `skip_serializing_if`. Typed dictionaries become `BTreeMap<String, T>`. Constraint check functions are generated alongside the types.

### TypeScript code generation

`super-yaml compile --format ts` generates TypeScript types from the same schemas:

```typescript
export type PositiveNumber = number;

export interface StereoVisionEye {
  agent_physical_radius: PositiveNumber;
  baseline: PositiveNumber;
}
```

Enums become string union types. Optional properties use `?` syntax.

---

## Complete Examples

### Example 1: Service scaling with environment overrides

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
WorkerThreads:
  type: integer
  constraints: "value >= 1"

---data
service_name <string>: billing
region <string>: "${env.REGION}"
replicas <Replicas>: 3
worker_threads <WorkerThreads>: "=max(replicas, env.CPU_CORES * 2)"
grpc_port <Port>: "${env.BASE_PORT}"
http_port <Port>: "=grpc_port + 1"
public_url <string>: "https://${region}.example.internal:${http_port}"
```

Compiles to:

```json
{
  "grpc_port": 7000,
  "http_port": 7001,
  "public_url": "https://us-east-1.example.internal:7001",
  "region": "us-east-1",
  "replicas": 3,
  "service_name": "billing",
  "worker_threads": 12
}
```

### Example 2: String constructors expanding shorthand values

```yaml
---!syaml/v0
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
    rgba:
      order: 1
      regex: '^rgba\((?<red>\d+),\s*(?<green>\d+),\s*(?<blue>\d+),\s*(?<alpha>0|1|0?\.\d+)\)$'
    hex:
      order: 2
      regex: '^#(?<red_hex>[0-9A-Fa-f]{2})(?<green_hex>[0-9A-Fa-f]{2})(?<blue_hex>[0-9A-Fa-f]{2})(?<alpha_hex>[0-9A-Fa-f]{2})?$'
      map:
        red: { group: red_hex, decode: hex_u8 }
        green: { group: green_hex, decode: hex_u8 }
        blue: { group: blue_hex, decode: hex_u8 }
        alpha: { group: alpha_hex, decode: hex_alpha }
      defaults:
        alpha: 1

---data
accent_rgb <Color>: "rgb(10, 20, 30)"
accent_rgba <Color>: "rgba(10, 20, 30, 0.5)"
accent_hex <Color>: "#0A141E80"
```

Compiles to:

```json
{
  "accent_hex": { "alpha": 0.502, "blue": 30, "green": 20, "red": 10 },
  "accent_rgb": { "alpha": 1, "blue": 30, "green": 20, "red": 10 },
  "accent_rgba": { "alpha": 0.5, "blue": 30, "green": 20, "red": 10 }
}
```

### Example 3: Multi-file template invocation

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

Compiles to:

```json
{
  "service": {
    "env": "prod",
    "host": "api.internal",
    "name": "api-service",
    "port": 8080,
    "tls": false
  }
}
```

### Example 4: Infrastructure config with constructors and typed dicts

```yaml
---!syaml/v0
---schema
CpuCount:
  type: integer
  minimum: 1
  maximum: 128

MemoryUnit: [MiB, GiB, TiB]
MemorySpec:
  type: object
  properties:
    amount: integer
    unit: MemoryUnit
  constructors:
    from_text:
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount: { group: amount, decode: integer }
        unit: { group: raw_unit, from_enum: MemoryUnit }

DiskSizeUnit: [GB, TB, PB]
DiskSizeSpec:
  type: object
  properties:
    amount: integer
    unit: DiskSizeUnit
  constraints:
    - "amount >= 1"
  constructors:
    from_text:
      regex: '^(?<amount>\d+)(?<raw_unit>[A-Za-z]+)$'
      map:
        amount: { group: amount, decode: integer }
        unit: { group: raw_unit, from_enum: DiskSizeUnit }

Disk:
  type: object
  properties:
    size: DiskSizeSpec
    disk_kind: [ssd, hdd]
    mount_path: string?

VmNetwork:
  type: object
  properties:
    subnet:
      type: string
      pattern: '^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}/\d{1,2}$'
    assign_public_ip: boolean

VmResource:
  type: object
  properties:
    vm_name:
      type: string
      pattern: '^[a-z0-9-]+$'
    image: string
    region: [us-west-2, us-east-1, eu-west-1]
    cpu: CpuCount
    memory: MemorySpec
    disks:
      type: object
      values: Disk
    network: VmNetwork
    tags:
      type: object
      values: string
      optional: true

---data
resource <VmResource>:
  vm_name: api-prod-01
  image: ubuntu-22.04
  region: us-west-2
  cpu <CpuCount>: 4
  memory <MemorySpec>: 16MiB
  disks:
    root:
      size <DiskSizeSpec>: 120GB
      disk_kind: ssd
      mount_path: /
    data:
      size <DiskSizeSpec>: 512GB
      disk_kind: ssd
      mount_path: /data
  network:
    subnet: 10.40.2.0/24
    assign_public_ip: false
  tags:
    env: prod
    owner: platform
```

### Example 5: Pricing logic with computed chain

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

### Example 6: Templates with constraint validation

```yaml
---!syaml/v0
---schema
SeatLimit:
  type: integer
  minimum: 1
RequestedSeats:
  type: integer
  minimum: 1
Booking:
  type: object
  properties:
    name: string
    seat_limit: SeatLimit
    requested_seats: RequestedSeats
  constraints:
    - "requested_seats <= seat_limit"

---data
# Private key — kept out of compiled output
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

---

## Integrating super_yaml into a Codebase

### Workflow: config file + generated types

1. **Define your config** in `.syaml` with schemas for all data shapes.
2. **Generate types** with `super-yaml compile config.syaml --format rust` or `--format ts`.
3. **Compile config** with `super-yaml compile config.syaml --pretty` to produce JSON.
4. **Load the JSON** in your application code using the generated types for type-safe deserialization.

### Workflow: multi-file composition

1. **Create shared schemas** in a base file (e.g., `shared.syaml`) with common types and template shapes.
2. **Import in service configs** — each service file imports shared and stamps out its own instances.
3. **Compile each service file** independently — imports are resolved automatically.

### Rust integration example

```rust
use super_yaml::{compile_document_from_path, ProcessEnvProvider};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compiled = compile_document_from_path("config/app.syaml", &ProcessEnvProvider)?;
    let json = compiled.to_json_string(true)?;
    println!("{}", json);
    Ok(())
}
```

For testing, use `MapEnvProvider` to supply controlled environment values:

```rust
use std::collections::HashMap;
use super_yaml::{compile_document, MapEnvProvider};

let mut vars = HashMap::new();
vars.insert("DB_HOST".to_string(), "test-db".to_string());
vars.insert("CPU_CORES".to_string(), "2".to_string());
let env = MapEnvProvider::new(vars);

let compiled = compile_document(input, &env)?;
```

---

## Common Mistakes to Avoid

1. **Missing marker**: Every file must start with `---!syaml/v0` as the first non-empty line.
2. **Unquoted expressions**: Expression values must be quoted strings: `"=a + b"`, not `=a + b`.
3. **Unquoted interpolation**: Interpolated strings must be quoted: `"${env.HOST}"`, not `${env.HOST}`.
4. **Template key not alone**: The `{{template.path}}` key must be the only key in its object.
5. **Missing type hint for constructors**: String constructors only fire when the value has a type hint pointing to the constructor's object type. `memory: 16GiB` does nothing — `memory <MemorySpec>: 16GiB` triggers the constructor.
6. **Forgetting `from: env`**: Each env binding must include `from: env` (the only supported source).
7. **Circular dependencies**: `a: "=b"` and `b: "=a"` will fail with a cycle error.
8. **Accessing env without `--allow-env`**: The CLI blocks all environment access by default. Pass `--allow-env KEY` for each variable.
9. **Import alias conflicts**: Each import alias must be unique. Imported type names are prefixed with the alias (`shared.Port`), not used bare.
10. **Tabs in indentation**: The parser only supports spaces for indentation, not tabs.

---

## Quick Syntax Reference

```yaml
---!syaml/v0                              # Required version marker

---meta
file:                                      # Optional file metadata
  owner: team-name
env:                                       # Environment bindings
  VAR_NAME:
    from: env
    key: ENV_VAR_NAME
    default: fallback_value                # or required: true
imports:                                   # Import other .syaml files
  alias: ./path/to/file.syaml

---schema
TypeName:                                  # Named type definition
  type: integer                            # Primitive type
  minimum: 0                               # Numeric constraint keyword
  constraints: "value >= 1"                # Expression constraint

EnumName: [a, b, c]                        # String enum shorthand

ObjectType:                                # Object type
  type: object
  properties:
    required_prop: string                  # Property shorthand
    optional_prop: integer?                # Optional shorthand
    enum_prop: [x, y, z]                   # Enum shorthand
    typed_prop:                            # Full property definition
      type: TypeName
      constraints: "value <= 100"
  values:                                  # Typed dictionary (dynamic keys)
    type: ValueType
  constraints:                             # Cross-field constraints
    - "prop_a <= prop_b"
  constructors:                            # String constructors
    name:
      regex: '^pattern$'
      map:
        field: { group: capture, decode: integer }
      defaults:
        field: value

ArrayType:                                 # Array type
  type: array
  items:
    type: string
  minItems: 1

---data
key <TypeName>: value                      # Type-hinted value
key <string>: literal                      # Built-in type hint
derived <integer>: "=a + b"               # Expression
interpolated <string>: "prefix ${a} suffix"  # Interpolation
env_value <string>: "${env.VAR_NAME}"      # Environment reference
_private_key: internal_only               # Private (stripped from output)

template_result <TypeName>:                # Template invocation
  {{path.to.template}}:
    VAR: value
```
