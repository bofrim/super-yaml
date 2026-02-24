# Super YAML — Language Reference

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
super-yaml validate config.syamlm

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

---contracts
# Function declarations with typed inputs, outputs, permissions, and contracts (optional section)
```

### Rules

1. The first non-empty line **must** be exactly `---!syaml/v0`.
2. Sections are opened with `---meta`, `---schema`, `---data`, or `---contracts`.
3. All four sections are optional. They can appear in any order. Each section can appear at most once.
4. When omitted, `schema` and `data` default to empty objects.
5. `---module` is a special section only valid in files named `module.syaml`. See the [Modules](#modules) section.

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

Two reserved keys have compiler-level meaning when versioned fields are in use:

- `schema_version` — a semver string declaring the active schema version of this document. Used to validate `since` / `deprecated` / `removed` field lifecycle annotations on properties.
- `strict_field_numbers` — when `true`, the compiler enforces that every property `field_number` within a type is unique and non-zero.

```yaml
---meta
file:
  schema_version: "2.1.0"
  strict_field_numbers: true
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
  # Short form: alias -> path (all sections imported)
  shared: ./shared_types.syaml

  # Long form: alias -> object with path (all sections imported)
  infra:
    path: ./infra/common.syaml

  # Module import: @module_name resolves via project registry (syaml.syaml)
  payments: "@payments"

  # Module file import: @module_name/file resolves to a specific file in that module
  inv: "@payments/invoice"
```

Relative paths resolve from the importing file's directory. `@module` paths are resolved via the project registry. Cyclic imports are detected and rejected.

#### Scoping imports with `sections`

By default all sections of the imported file are available under the namespace alias. Use `sections` to restrict which sections are imported.

Valid section names: `schema`, `data`, `contracts`.

```yaml
---meta
imports:
  # Import only schema types — data is not available via this namespace
  types_only:
    path: ./shared.syaml
    sections: [schema]

  # Import only data — schema types are not available via this namespace
  data_only:
    path: ./shared.syaml
    sections: [data]

  # Import both explicitly
  both:
    path: ./shared.syaml
    sections: [schema, data]
```

Block list form is also accepted:

```yaml
---meta
imports:
  shared:
    path: ./shared.syaml
    sections:
      - schema
      - data
```

**Example — types-only import:**

`shared.syaml`:

```yaml
---!syaml/v0
---schema
Port:
  type: integer
  minimum: 1
  maximum: 65535
---data
default_port: 8080
```

`config.syaml`:

```yaml
---!syaml/v0
---meta
imports:
  shared:
    path: ./shared.syaml
    sections: [schema]   # only types; shared.default_port is not accessible
---schema
Server:
  type: object
  properties:
    port:
      type: shared.Port  # OK — schema was imported
---data
server <Server>:
  port: 443
```

If you reference `shared.default_port` (data) or forget to include `schema` and reference `shared.Port`, the compiler emits a targeted error explaining which section to add.

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

### Keyed enums with typed values

You can define enums as a keyed object map where each member is a fully typed value:

```yaml
---schema
TimezoneInfo:
  type: object
  properties:
    locale: string
    offset: string

Timezone:
  type: TimezoneInfo
  enum:
    UTC:
      locale: en-US
      offset: "+00:00"
    EST:
      locale: en-US
      offset: "-05:00"
```

Rules:
- `enum` may be either an array (classic enum) or an object map (keyed enum).
- For keyed enums, keys must match `[A-Za-z_][A-Za-z0-9_]*`.
- Each keyed-enum value must validate against the schema node's declared `type`.

In `---data`, you can reference keyed enum members using exact `Type.member` tokens:

```yaml
---data
tz <TimezoneInfo>: Timezone.UTC
```

`Type.member` references are resolved to the concrete member value during compilation.

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

ServiceConfig:
  type: object
  properties:
    radius:
      type: PositiveNumber  # reuse in a property
      maximum: 25
    speed:
      type: PositiveNumber
```

### Type extension

Object types can inherit fields from a parent type using `ChildType <ParentType>:` syntax. Extension is restricted to object types. All parent fields are implicitly locked — children may only add new fields, never redeclare inherited ones. This guarantees IS-A substitutability: a `ChildType` value is always a valid `ParentType` value.

```yaml
---schema
Animal:
  type: object
  properties:
    name: string
    age: integer

Dog <Animal>:          # Dog IS-A Animal
  type: object
  properties:
    breed: string      # only new fields allowed; cannot redeclare name or age
```

After expansion, `Dog` is a flat object with `name`, `age`, and `breed`. Anywhere an `Animal` type is expected, a `Dog` value is also accepted.

**Multi-level chains** work as expected: each level only adds new fields.

```yaml
---schema
A:
  type: object
  properties:
    a_field: string

B <A>:
  type: object
  properties:
    b_field: integer

C <B>:                 # C inherits a_field (from A) and b_field (from B)
  type: object
  properties:
    c_field: boolean
```

**Constraint scope**: inherited fields are available in child constraints automatically because expansion happens before constraint collection.

```yaml
---schema
Base:
  type: object
  properties:
    min_val: integer

Range <Base>:
  type: object
  properties:
    max_val: integer
  constraints:
    - "min_val <= max_val"    # min_val comes from Base
```

**Error cases**:


| Condition                     | Error                                                                       |
| ----------------------------- | --------------------------------------------------------------------------- |
| `<UnknownParent>`             | `"type 'Child' extends unknown type 'UnknownParent'"`                       |
| Parent is not `type: object`  | `"type 'Child' extends 'Parent' which is not an object type"`               |
| Child is not `type: object`   | `"only object types can use extends, but 'Child' is not an object type"`    |
| Child redeclares parent field | `"type 'Child' cannot redeclare field 'field' already defined in 'Parent'"` |
| Circular chain                | `"circular type extension involving: A, B"`                                 |


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

### Union types

A union type (analogous to JSON Schema's `oneOf`) accepts a value that matches exactly one of several variants. There are three forms.

#### Tagged dispatch (map form)

Use `tag` to name the discriminator field. At runtime the value of that field is looked up in `options` to select the variant. Set `tag_required: true` to require the tag field to be present.

```yaml
---schema
ErrorDetail:
  type: object
  properties:
    code: integer
    message: string

SuccessPayload:
  type: object
  properties:
    id: string
    created: boolean

ApiResponse:
  type: union
  tag: status
  tag_required: true
  options:
    ok:
      type: object
      properties:
        status: string
        data: SuccessPayload
    error:
      type: object
      properties:
        status: string
        error: ErrorDetail
```

#### Ordered matching (list form)

Variants are tried in order; the first match wins. No discriminator field is required.

```yaml
---schema
FlexibleInput:
  type: union
  options:
    - string
    - type: object
      properties:
        query: string
        page: "integer?"
```

#### Pipe shorthand

For a quick inline union of named types, use `|` between type names:

```yaml
---schema
EndpointResult: SuccessPayload | ErrorDetail
```

This is equivalent to a list-form union with those two variants.

#### Using union types in data

```yaml
---data
response <ApiResponse>:
  status: ok
  data:
    id: abc-123
    created: true
quick_input <FlexibleInput>: just a string
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
PoolSize:
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

### Mutability

Schema types can declare a `mutability` policy that constrains how data values of that type may be updated over time. This is enforced by the contracts section's permission model.


| Value               | Meaning                                                                   |
| ------------------- | ------------------------------------------------------------------------- |
| `frozen`            | The value may never be changed after initial write.                       |
| `monotone_increase` | The value may only increase (e.g. an ever-growing counter).               |
| `replace`           | The value may be freely replaced. (Default when `mutability` is omitted.) |


```yaml
---schema
Score:
  type: integer
  minimum: 0
  mutability: monotone_increase

PlayerName:
  type: string
  mutability: frozen

LevelLabel:
  type: string
  mutability: replace
```

#### Instance-level freeze (`^`)

Append `^` to a data key to freeze that specific instance, regardless of the type-level `mutability` policy. Even if the type allows changes, the key with `^` cannot be written by any function.

```yaml
---data
score <Score>: 0
high_score^: 9999   # frozen at the instance level; no function may write this key
level <LevelLabel>: "Beginner"
```

The `^` suffix is stripped from the output key name — `high_score^` compiles to `"high_score"` in JSON.

### Versioned fields

Properties in an object type can carry lifecycle annotations that describe when they were introduced, deprecated, or removed. The compiler checks these annotations against `meta.file.schema_version` and emits warnings or errors accordingly.


| Annotation     | Value                   | Meaning                                                                                                                                       |
| -------------- | ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `field_number` | positive integer        | Stable protobuf-style field identity (must be unique when `strict_field_numbers: true`)                                                       |
| `since`        | semver string           | Version when the field was introduced. Error if `schema_version` is older than `since`.                                                       |
| `deprecated`   | semver string or object | Version when the field was deprecated. A warning is emitted when a data value uses this field and `schema_version` >= the deprecated version. |
| `removed`      | semver string           | Version when the field was removed. Error if `schema_version` >= `removed` and the data still contains the field.                             |


The `deprecated` annotation accepts two forms:

```yaml
# Simple form — just a version string
username:
  type: string
  deprecated: "1.5.0"
  optional: true

# Rich form — version, severity, and human-readable message
phone:
  type: string
  deprecated:
    version: "2.0.0"
    severity: warning
    message: "Contact by email only; phone field will be removed in v3.0"
  optional: true
```

Full example:

```yaml
---!syaml/v0
---meta
file:
  schema_version: "2.1.0"
  strict_field_numbers: true

---schema
UserProfile:
  type: object
  properties:
    id:
      type: integer
      field_number: 1
      since: "1.0.0"
    name:
      type: string
      field_number: 2
      since: "1.0.0"
    email:
      type: string
      field_number: 3
      since: "1.0.0"
    phone:
      type: string
      field_number: 4
      since: "1.0.0"
      deprecated:
        version: "2.0.0"
        severity: warning
        message: "Contact by email only; phone field will be removed in v3.0"
      optional: true
    old_id:
      type: string
      field_number: 5
      since: "1.0.0"
      removed: "2.0.0"
      optional: true
```

Deprecation warnings are printed to stderr during `compile` and `validate`. They do not prevent successful compilation.

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

### `as_string` — Object-to-string serialization

Object types can declare an `as_string` template that describes how to render the object as a string. The template uses `{{property_name}}` placeholders that are replaced with the corresponding property values at code-generation time.

```yaml
---schema
Semver:
  type: object
  as_string: "{{major}}.{{minor}}.{{patch}}"
  properties:
    major: integer
    minor: integer
    patch: integer
```

Once defined:

- **Type compatibility**: a `Semver` object can be used anywhere a `string` is expected. For example, if a parent schema declares `version: string`, you can hint the data value as `version <Semver>:` and the type checker will accept it.
- **Rust codegen**: generates `impl std::fmt::Display for Semver`, giving you a free `.to_string()` call.
- **TypeScript codegen**: generates a standalone `semverToString(semver: Semver): string` function using a template literal.

Rules:
- Only allowed on `type: object` schemas.
- The value must be a non-empty string.
- Every `{{placeholder}}` must reference a property declared in `properties`.
- Only include required properties in the template — optional (`?`) properties would render as their Rust/TypeScript `Option` representation rather than their inner value.

`as_string` and `constructors` are natural inverses: constructors parse strings into objects, `as_string` serializes objects back to strings.

### Complete schema keyword reference


| Category   | Keywords                                                                  |
| ---------- | ------------------------------------------------------------------------- |
| Common     | `type`, `enum`                                                            |
| Numeric    | `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`              |
| String     | `minLength`, `maxLength`, `pattern`                                       |
| Object     | `properties`, `values`, `required`, `optional`, `constructors`, `as_string` |
| Array      | `items`, `minItems`, `maxItems`                                           |
| Validation | `constraints`                                                             |
| Union      | `tag`, `tag_required`, `options` (on `type: union`)                       |
| Versioning | `field_number`, `since`, `deprecated`, `removed` (on properties)         |
| Mutability | `mutability` (`frozen`, `monotone_increase`, `replace`)                   |


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

### Direct data references

A scalar value that starts with `$.` or `.` is a **direct data reference** — it copies another value without requiring an expression (`=`). References are resolved after the document is fully loaded, so any resolved data value can be referenced.

Two forms are supported:

- `$.path.to.value` — **file-scope**: resolves from the document root.
- `.sibling_key` — **current-scope**: resolves relative to the immediate parent object.

Both forms can copy scalars **or** entire sub-objects (the copied subtree is deep-cloned into the target).

```yaml
---data
_defaults:
  host: 0.0.0.0
  timeout: 30
  tls:
    enabled: false

api <EndpointConfig>:
  host: $._defaults.host        # scalar copied from _defaults (file-scope)
  port: 8080
  admin_port: .port             # sibling ref: copies api.port → 8080
  timeout: $._defaults.timeout  # scalar copied from _defaults (file-scope)
  tls: $._defaults.tls          # entire sub-object copied from _defaults

worker <EndpointConfig>:
  host: $._defaults.host
  port: 9000
  admin_port: .port             # sibling ref: copies worker.port → 9000
  timeout: $._defaults.timeout
  tls: $.api.tls                # reuses api's tls sub-object (file-scope)
```

Compiles to:

```json
{
  "api": {
    "admin_port": 8080,
    "host": "0.0.0.0",
    "port": 8080,
    "timeout": 30,
    "tls": { "enabled": false }
  },
  "worker": {
    "admin_port": 9000,
    "host": "0.0.0.0",
    "port": 9000,
    "timeout": 30,
    "tls": { "enabled": false }
  }
}
```

**Rules:**

- Sibling references (`.key`) only look in the direct parent object — they do not walk up the tree.
- References to private keys (`_`-prefixed) are valid within the same file; private data is stripped from output but can seed other values via references.
- Circular references are detected and rejected.
- Type hints and constraints are validated on the resolved value, not the reference itself.

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
    level: standard
```

#### Invoking templates

Use the template path as a key in an object, and pass variable bindings as the value:

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
    "level": "standard"
  }
}
```

#### Mixing templates with sibling keys

Template invocations can appear alongside other keys in the same object. The template output is merged with the sibling keys, and siblings override any conflicting template key:

```yaml
---data
vip_service <Service>:
  {{_templates.service}}:
    NAME: vip-service
    HOST: vip.internal
    ENV: prod
  tls: true
  port: 9443
  level: critical
```

This resolves to:

```json
{
  "vip_service": {
    "name": "vip-service",
    "host": "vip.internal",
    "port": 9443,
    "tls": true,
    "env": "prod",
    "level": "critical"
  }
}
```

#### Locked template fields

Append `!` to a key name in the template definition to prevent sibling keys from overriding it:

```yaml
---data
_templates:
  service:
    name!: "{{NAME}}"
    host!: "{{HOST}}"
    port: "{{PORT:8080}}"
    tls: "{{TLS:false}}"
```

With this template, `port` and `tls` can be overridden by siblings, but attempting to override `name` or `host` produces an error. The `!` suffix is stripped from the output key name.

Rules:

- All required variables must be provided. Unknown variables are rejected.
- When sibling keys are present, the template must expand to an object (not a scalar or array).
- Sibling keys override template output when names conflict, unless the field is locked (`!`).
- Only one template invocation is allowed per object.
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

### URL imports

Import paths can be HTTP or HTTPS URLs. Downloaded content is cached to disk and tracked
in a `syaml.lock` lockfile next to the root file.

```yaml
---meta
imports:
  base: https://example.com/schemas/base.syaml
```

Sub-imports from URL-sourced files resolve relative paths as relative URLs.

CLI flags:

- `--update-imports` - force re-fetch of all URL imports (bypass lockfile cache).
- `--cache-dir <path>` - override the default cache directory (`$SYAML_CACHE_DIR` or `~/.cache/super_yaml/`).

### Hash verification

Pin an import to an exact content hash so any modification is caught before compilation.
Only `sha256` is supported. The format is `sha256:<hex_digest>`.

```yaml
---meta
imports:
  shared:
    path: ./shared.syaml
    hash: sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

### Signature verification

Verify that an import's content was signed by a trusted Ed25519 key.

```yaml
---meta
imports:
  trusted:
    path: https://corp.dev/schemas/base.syaml
    signature:
      public_key: ./keys/corp.pub
      value: base64-encoded-detached-signature==
```

- `public_key` - path to a raw 32-byte or PEM-encoded Ed25519 public key.
- `value` - base64-encoded detached Ed25519 signature over the raw file bytes.

### Version pinning

Require that an imported file declares a semver version that satisfies a requirement.
The imported file advertises its version in `meta.file.version`.

Imported file:

```yaml
---!syaml/v0
---meta
file:
  version: "1.2.3"
---schema
...
```

Importing file:

```yaml
---meta
imports:
  shared:
    path: ./shared.syaml
    version: "^1.0.0"
```

Standard semver requirement syntax is supported (`^`, `~`, `>=`, `<`, etc.).

### Combining verification options

All three options are independent and composable:

```yaml
---meta
imports:
  trusted:
    path: https://corp.dev/schemas/base.syaml
    hash: sha256:deadbeef...
    signature:
      public_key: ./keys/corp.pub
      value: base64-sig==
    version: ">=2.0.0, <3.0.0"
```

### Import rules

- Each imported file runs its own full compilation pipeline.
- Private keys (`_`-prefixed) in imported files are not accessible.
- Cyclic imports are detected and produce an error.
- Local paths are relative to the importing file's directory.
- URL paths resolve relative sub-imports as relative URLs.
- Schema types must be referenced with the alias prefix (`shared.Port`, not `Port`).
- Hash and signature are verified on the raw file bytes before any parsing.
- Version is checked after compilation against `meta.file.version`.

---

## Modules

Modules are the unit of code organization in super_yaml. A directory becomes a module by placing a `module.syaml` manifest file inside it. The manifest declares the module's identity, enforces import rules, and provides shared metadata and imports that automatically apply to every `.syaml` file in the module.

### Project registry (`syaml.syaml`)

The project registry lives at the project root (the nearest ancestor directory that contains `syaml.syaml` or `.git`). It maps module names to directory paths using standard syaml syntax:

```yaml
---!syaml/v0
---data
modules:
  payments: "services/payments/"
  core: "shared/core/"
  infra: "infra/"
```

When resolving `@payments`, the compiler finds the registry, reads `modules.payments`, and loads `services/payments/module.syaml`.

### Module manifest (`module.syaml`)

Any directory containing `module.syaml` is a module. The manifest uses a `---module` section to declare identity and policy:

```yaml
---!syaml/v0

---module
name: payments
version: "1.0.0"
description: "Payment processing schemas"

# Merged into meta.file for every file in this module (file-level values win)
metadata:
  owner: platform-team
  schema_version: "1.0.0"

# Restricts what files in this module can import
import_policy:
  allow_network_imports: false   # Block URL-based imports
  require_version: false         # Every import must specify version
  require_hash: false            # Every import must specify a content hash
  require_signature: false       # Every import must carry a signature
  allowed_domains: []            # Non-empty = allowlist for network import hosts
  blocked_modules: []            # Module names that cannot be imported

---meta
# These imports are injected into every file in the module
# Files can shadow them with their own declarations
imports:
  core: "@core"

---schema
# Optional: shared types defined at the module level
Currency: [USD, EUR, GBP]
```

**Rules for manifests:**

- `---module` is only valid in a file named `module.syaml`. Using it in any other file is an error.
- `---data` and `---contracts` are not allowed in `module.syaml`.
- `---schema` is allowed for module-level shared types.

### `@module` import syntax

Use `@module_name` or `@module_name/file` as an import path to reference modules by name rather than filesystem path:

```yaml
---meta
imports:
  # Import a whole module namespace (schema types and data from module.syaml)
  payments: "@payments"

  # Import a specific file within a module
  inv: "@payments/invoice"
  ref: "@payments/refund"
```

This lets you reorganize directories without updating every import that references the module — only the registry entry changes.

### Module metadata inheritance

When any `.syaml` file is compiled, the compiler walks up the directory tree to find a `module.syaml`. If found:

1. The module `metadata` block is merged into the file's `meta.file`. **File-level values win** — the module only fills in keys the file doesn't declare.
2. Module-level `---meta` imports are injected into the file's import namespace. **File-level imports shadow module imports** under the same alias.

This means every file in a module automatically inherits the team ownership, schema version, and shared imports declared in the manifest without having to repeat them.

### Import policy enforcement

The `import_policy` block in a manifest is enforced for every import declared in any file that belongs to the module:

| Policy key              | Default | Effect when enabled                                          |
| ----------------------- | ------- | ------------------------------------------------------------ |
| `allow_network_imports` | `true`  | When `false`, any `http://` or `https://` import is rejected |
| `require_version`       | `false` | Every import must include a `version` constraint             |
| `require_hash`          | `false` | Every import must include a `hash` field                     |
| `require_signature`     | `false` | Every import must include a `signature` block                |
| `allowed_domains`       | `[]`    | Non-empty: only listed domains are permitted for URLs        |
| `blocked_modules`       | `[]`    | `@module` imports matching these names are rejected          |

Policy violations are compile errors.

### Using modules to organize code

A typical module-based project layout:

```text
project/
  syaml.syaml           ← project registry
  services/
    billing/
      module.syaml      ← billing module manifest
      service.syaml     ← billing service config
      events.syaml      ← event types for billing
    payments/
      module.syaml      ← payments module manifest
      invoice.syaml     ← invoice schemas and defaults
      refund.syaml      ← refund schemas
  shared/
    core/
      module.syaml      ← core module manifest
      types.syaml       ← project-wide base types
```

Files in `services/billing/` automatically inherit:

- `meta.file` metadata from `billing/module.syaml` (e.g. `owner: billing-team`)
- Any module-level imports declared in `billing/module.syaml`

Files in `services/payments/` inherit from their own module manifest independently.

A file in `services/billing/service.syaml` can import the payments module with no path knowledge:

```yaml
---!syaml/v0
---meta
imports:
  inv: "@payments/invoice"

---data
default_currency <string>: "=inv.default_currency"
```

---

## The `contracts` Section

The `contracts` section declares named functions whose inputs, outputs, permissions, and behavioral contracts are all specified in the `.syaml` file. The compiler validates the contracts at compile time and generates typed stub code (Rust or TypeScript) from them.

```yaml
---contracts
FunctionName:
  inputs:
    param_name:
      type: TypeName   # any schema type or primitive
      mutable: false   # whether the caller may mutate this parameter (default false)
  output:
    type: TypeName
  errors:
    - ErrorTypeName    # optional list of error types the function may raise
  permissions:
    data:
      read:
        - "$.path.to.field"   # JSONPath-style selectors for readable data fields
      write:
        - "$.path.to.field"   # JSONPath-style selectors for writable data fields
  specification:
    description: "Human-readable description of the function."
    preconditions:
      strict:
        - "input.param_name > 0"   # evaluatable expression; variables: input.*, data.*, output.*
      semantic:
        - "param_name must be positive"  # human-readable prose
    postconditions:
      strict:
        - "output >= input.param_name"
      semantic:
        - "result is at least as large as the input"
```

### Function fields


| Field                                   | Required | Description                                                                                                                  |
| --------------------------------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `inputs`                                | no       | Named input parameters with `type` and optional `mutable` flag. Inline object/array schemas generate named types in codegen. |
| `output`                                | no       | Return type declaration.                                                                                                     |
| `errors`                                | no       | List of named error types the function may raise.                                                                            |
| `permissions.data.read`                 | no       | JSONPath selectors for data fields the function is allowed to read.                                                          |
| `permissions.data.write`                | no       | JSONPath selectors for data fields the function is allowed to write.                                                         |
| `specification.description`             | no       | Free-form description.                                                                                                       |
| `specification.preconditions.strict`    | no       | Evaluatable boolean expressions checked before execution. Scope: `input.*`, `data.*`.                                        |
| `specification.preconditions.semantic`  | no       | Human-readable intent (not evaluated).                                                                                       |
| `specification.postconditions.strict`   | no       | Evaluatable boolean expressions checked after execution. Scope: `input.*`, `data.*`, `output`.                               |
| `specification.postconditions.semantic` | no       | Human-readable intent (not evaluated).                                                                                       |


### Strict condition expressions

Strict pre/postconditions use the same expression language as schema `constraints`, with three available variable roots:

- `input.<param>` — references an input parameter by name
- `data.<path>` — references a compiled data field (must be in the function's `read` permissions)
- `output` — references the return value (postconditions only)

The compiler validates that references use only these three roots and that `data.*` paths are listed in `permissions.data.read`. Invalid paths are a compile error.

### Generated code

When compiled with `--format rust` or `--format ts`, each function with strict conditions produces four generated functions:

1. `check_preconditions(...)` — validates all `preconditions.strict` expressions
2. `<function_name>_impl(...)` — the stub to fill in with implementation
3. `check_postconditions(...)` — validates all `postconditions.strict` expressions
4. `<function_name>(...)` — public entry point that chains 1 → 2 → 3

### Example

```yaml
---!syaml/v0
---schema
Score:
  type: integer
  minimum: 0

---data
score <Score>: 0
high_score: 9999

---contracts
AddPoints:
  inputs:
    points:
      type: integer
      mutable: false
  output:
    type: Score
  permissions:
    data:
      read:
        - "$.score"
        - "$.high_score"
      write:
        - "$.score"
  specification:
    preconditions:
      strict:
        - "input.points > 0"
        - "input.points < data.high_score"
      semantic:
        - "points must be positive and less than the high score ceiling"
    postconditions:
      strict:
        - "output >= input.points"
      semantic:
        - "score increases by the given points"
```

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
DisplayProfile:
  type: object
  properties:
    scale_factor:
      type: PositiveNumber
      maximum: 25
    refresh_hz:
      type: PositiveNumber
```

It generates:

```rust
use serde::{Deserialize, Serialize};

pub type PositiveNumber = f64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayProfile {
    pub scale_factor: PositiveNumber,
    pub refresh_hz: PositiveNumber,
}
```

Richer schemas produce richer output. Enums become Rust enums with serde rename attributes. Optional properties become `Option<T>` with `skip_serializing_if`. Typed dictionaries become `BTreeMap<String, T>`. Constraint check functions are generated alongside the types.

### TypeScript code generation

`super-yaml compile --format ts` generates TypeScript types from the same schemas:

```typescript
export type PositiveNumber = number;

export interface DisplayProfile {
  scale_factor: PositiveNumber;
  refresh_hz: PositiveNumber;
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

### Example 7: Module-based project organization

This example shows the `payments` module from the `examples/` directory. Three files work together — the registry, the module manifest, and a consuming file.

**`examples/syaml.syaml`** (project registry):

```yaml
---!syaml/v0
---data
modules:
  payments: "payments/"
```

**`examples/payments/module.syaml`** (module manifest):

```yaml
---!syaml/v0

---module
name: payments
version: "1.0.0"
description: "Payment processing schemas"
metadata:
  owner: platform-team
  schema_version: "1.0.0"
import_policy:
  allow_network_imports: false

---schema
Currency: [USD, EUR, GBP]

Money:
  type: object
  properties:
    amount: number
    currency: Currency
  constraints:
    - "amount >= 0"
```

**`examples/payments/invoice.syaml`** (module member — inherits manifest metadata automatically):

```yaml
---!syaml/v0

---schema
InvoiceStatus: [pending, paid, cancelled]

Invoice:
  type: object
  properties:
    id: string
    status: InvoiceStatus
    amount: number
    currency: string

---data
default_currency <string>: "USD"
tax_rate <number>: 0.08
```

**`examples/checkout.syaml`** (outside the module — uses `@payments/invoice`):

```yaml
---!syaml/v0

---meta
imports:
  inv: "@payments/invoice"

---schema
CheckoutConfig:
  type: object
  properties:
    currency: string
    tax_rate: number
    max_retries: integer

---data
checkout <CheckoutConfig>:
  currency <string>: "=inv.default_currency"
  tax_rate <number>: "=inv.tax_rate"
  max_retries <integer>: 3
```

`checkout.syaml` compiles to:

```json
{
  "checkout": {
    "currency": "USD",
    "max_retries": 3,
    "tax_rate": 0.08
  }
}
```

Meanwhile, `payments/invoice.syaml` automatically carries `owner: platform-team` and `schema_version: "1.0.0"` in its `meta.file` without declaring them — inherited from `module.syaml`.

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
4. **Multiple template keys in one object**: Only one `{{template.path}}` invocation is allowed per object. Sibling (non-template) keys are fine and will be merged with the template output.
5. **Missing type hint for constructors**: String constructors only fire when the value has a type hint pointing to the constructor's object type. `memory: 16GiB` does nothing — `memory <MemorySpec>: 16GiB` triggers the constructor.
6. **Forgetting `from: env`**: Each env binding must include `from: env` (the only supported source).
7. **Circular dependencies**: `a: "=b"` and `b: "=a"` will fail with a cycle error.
8. **Accessing env without `--allow-env`**: The CLI blocks all environment access by default. Pass `--allow-env KEY` for each variable.
9. **Import alias conflicts**: Each import alias must be unique. Imported type names are prefixed with the alias (`shared.Port`), not used bare.
10. **Tabs in indentation**: The parser only supports spaces for indentation, not tabs.
11. **Confusing references with expressions**: `$.path` and `.sibling` are direct data references (no `=` prefix). Wrapping them in `"=..."` makes them expression variables, not reference syntax — use the bare form for copies and `"=..."` only when you need arithmetic or function calls.

---

## Quick Syntax Reference

```yaml
---!syaml/v0                              # Required version marker

---meta
file:                                      # Optional file metadata
  owner: team-name
  schema_version: "2.0.0"                 # Active schema version (for versioned fields)
  strict_field_numbers: true              # Enforce unique field_number per type
env:                                       # Environment bindings
  VAR_NAME:
    from: env
    key: ENV_VAR_NAME
    default: fallback_value                # or required: true
imports:                                   # Import other .syaml files
  alias: ./path/to/file.syaml
  mod_alias: "@module_name"               # Module import (resolved via syaml.syaml registry)
  file_alias: "@module_name/file"         # Specific file within a module

---schema
TypeName:                                  # Named type definition
  type: integer                            # Primitive type
  minimum: 0                               # Numeric constraint keyword
  constraints: "value >= 1"                # Expression constraint
  mutability: frozen                       # frozen | monotone_increase | replace

EnumName: [a, b, c]                        # String enum shorthand

PipeUnion: TypeA | TypeB                   # Pipe shorthand union

TaggedUnion:                               # Tagged dispatch union
  type: union
  tag: kind
  tag_required: true
  options:
    variant_a:
      type: object
      properties: { kind: string }
    variant_b:
      type: object
      properties: { kind: string }

OrderedUnion:                              # Ordered matching union
  type: union
  options:
    - string
    - type: object
      properties:
        query: string

ObjectType:                                # Object type
  type: object
  properties:
    required_prop: string                  # Property shorthand
    optional_prop: integer?                # Optional shorthand
    enum_prop: [x, y, z]                   # Enum shorthand
    typed_prop:                            # Full property definition
      type: TypeName
      constraints: "value <= 100"
      field_number: 1                      # Versioned field: stable identity
      since: "1.0.0"                       # Versioned field: introduced in
      deprecated: "2.0.0"                  # Versioned field: deprecated in (or object with version/severity/message)
      removed: "3.0.0"                     # Versioned field: removed in
  values:                                  # Typed dictionary (dynamic keys)
    type: ValueType
  constraints:                             # Cross-field constraints
    - "prop_a <= prop_b"
  constructors:                            # String constructors (string → object)
    name:
      regex: '^pattern$'
      map:
        field: { group: capture, decode: integer }
      defaults:
        field: value
  as_string: "{{prop_a}}-{{prop_b}}"      # Object-to-string template (object → string)

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
ref_root: $.path.to.value                 # File-scope direct reference (from document root)
ref_sibling: .sibling_key                 # Current-scope direct reference (from parent object)
_private_key: internal_only               # Private (stripped from output)
frozen_key^: 9999                          # Instance-level freeze (^ stripped from output key)

template_result <TypeName>:                # Template invocation
  {{path.to.template}}:
    VAR: value

template_with_overrides <TypeName>:        # Template with sibling overrides
  {{path.to.template}}:
    VAR: value
  extra_key: extra_value                   # Merged; overrides template on conflict

# In template definitions:
_templates:
  example:
    locked_field!: value                   # Cannot be overridden by siblings
    open_field: value                      # Can be overridden by siblings

---contracts
FunctionName:                              # Function declaration
  inputs:
    param:
      type: TypeName
      mutable: false
  output:
    type: TypeName
  permissions:
    data:
      read: ["$.field"]
      write: ["$.field"]
  specification:
    preconditions:
      strict: ["input.param > 0"]          # Evaluatable (input.*, data.*, output)
      semantic: ["param must be positive"] # Human-readable prose
    postconditions:
      strict: ["output >= input.param"]
      semantic: ["result is at least input"]

# module.syaml only — declare module identity, policy, and shared imports
---module
name: module_name                          # Required
version: "1.0.0"                           # Optional semver
description: "Human-readable description"  # Optional
metadata:                                  # Merged into meta.file for all member files
  owner: team-name
  schema_version: "1.0.0"
import_policy:                             # Applied to all imports in member files
  allow_network_imports: false             # default: true
  require_version: false                   # default: false
  require_hash: false                      # default: false
  require_signature: false                 # default: false
  allowed_domains: []                      # default: [] (unrestricted)
  blocked_modules: []                      # default: [] (none blocked)
```
