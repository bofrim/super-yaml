# Development Guide

This file is a contributor-oriented map of the codebase and a guide for where to make changes for different feature types.

## Repository Overview

- `src/`: Rust library and CLI implementation.
- `src/bin/super-yaml.rs`: CLI entrypoint (`validate`, `compile`, `docs`, `from-json-schema`).
- `tests/`: integration and behavior tests grouped by feature area.
- `examples/`: user-facing `.syaml` fixtures plus expected outputs.
- `library/`: reusable SYAML modules (`module.syaml` + `examples.syaml`) used by docs/examples.
- `vscode-syaml/`: VS Code extension (syntax, diagnostics, preview/export commands).
- `.github/workflows/`: CI and VSIX release workflows.

## Core Architecture

`src/lib.rs` is the orchestration layer. Most features involve a module in `src/` plus a pipeline hook in `lib.rs`.

Compilation flow (high-level):

1. Scan marker/sections: `src/section_scanner.rs`
2. Parse YAML subset per section: `src/mini_yaml.rs`
3. Parse schema + normalize data type hints: `src/schema.rs`, `src/type_hints.rs`
4. Merge/import schemas and data: `src/lib.rs` + `src/fetch.rs` + `src/module.rs` + `src/verify.rs`
5. Expand templates and references: `src/template.rs`, `src/resolve.rs`
6. Resolve env and expressions: `src/resolve.rs`, `src/expr/*`
7. Coerce constructors: `src/coerce.rs`
8. Validate type hints/constraints/versioning: `src/validate.rs`, `src/schema.rs`
9. Functional section checks: `src/functional.rs`

## Module Responsibilities (`src/`)

- `ast.rs`: shared data model (`ParsedDocument`, `CompiledDocument`, `Meta`, module/functional structs).
- `error.rs`: top-level error enum (`SyamlError`) used throughout.
- `section_scanner.rs`: marker and section fence validation.
- `mini_yaml.rs`: constrained YAML parser.
- `schema.rs`: schema parsing/normalization, type resolution, keyword validation, version/mutability helpers.
- `type_hints.rs`: `<Type>` extraction from data keys and freeze-marker handling.
- `resolve.rs`: env resolution, expression/interpolation evaluation, path/data reference resolution.
- `expr/lexer.rs`, `expr/parser.rs`, `expr/eval.rs`: expression language implementation.
- `template.rs`: template invocation + placeholder substitution.
- `coerce.rs`: regex constructor coercion for hinted object types.
- `validate.rs`: type-hint validation, constraint evaluation, versioned-field checks.
- `fetch.rs`: URL import fetch/cache/lockfile behavior.
- `verify.rs`: hash/signature/version checks for imports.
- `module.rs`: `module.syaml` parsing, module registry lookup, import policy enforcement.
- `functional.rs`: `---functional` parsing, validation, JSON/stub generation.
- `rust_codegen.rs`, `typescript_codegen.rs`, `proto_codegen.rs`: schema-to-code generation.
- `json_schema_export.rs`, `json_schema_import.rs`: JSON Schema conversion bridge.
- `html_docs_gen.rs`: HTML docs generation and import-graph site generation.
- `yaml_writer.rs`: compiled JSON -> YAML rendering.

## Feature-to-File Change Map

Use this as a first-pass “where do I edit?” index.

| Feature type | Primary files to update | Usually also update |
|---|---|---|
| New top-level section or section rules | `src/section_scanner.rs`, `src/lib.rs` (`parse_document*`) | `src/ast.rs`, `tests/section_and_parse.rs`, README/this doc |
| YAML syntax support (parser behavior) | `src/mini_yaml.rs` | `tests/section_and_parse.rs`, feature tests that rely on parsing |
| New schema keyword / schema semantics | `src/schema.rs` | `src/validate.rs`, `src/rust_codegen.rs`, `src/typescript_codegen.rs`, `src/proto_codegen.rs`, `tests/schema_validation.rs` |
| Type-hint syntax changes | `src/type_hints.rs` | `src/validate.rs`, `tests/schema_validation.rs`, `tests/section_and_parse.rs` |
| Expression operator/function changes | `src/expr/lexer.rs`, `src/expr/parser.rs`, `src/expr/eval.rs` | `src/resolve.rs`, `src/validate.rs`, `tests/expr_engine.rs` |
| Env binding behavior | `src/resolve.rs` (`resolve_env_bindings`) | `src/bin/super-yaml.rs` (`--allow-env` handling), `tests/resolve_behavior.rs` |
| Template syntax or merge behavior | `src/template.rs` | `tests/examples_integration.rs`, template-specific examples |
| Data/path reference behavior (`$.`, `.sibling`) | `src/resolve.rs` (`resolve_data_references`) | `tests/data_references.rs` |
| Import resolution/fetch/caching | `src/lib.rs` (import merge), `src/fetch.rs` | `tests/imports_integration.rs`, `scripts/check-examples.sh` fixtures |
| Import integrity/policy/security | `src/verify.rs`, `src/module.rs`, `src/lib.rs` | `tests/imports_integration.rs`, `tests/module_integration.rs` |
| Module manifest / registry behavior | `src/module.rs`, `src/lib.rs` (module context application) | `tests/module_integration.rs`, `library/*/module.syaml` if examples depend on it |
| Functional section semantics | `src/functional.rs`, `src/lib.rs`, `src/ast.rs` | CLI output modes (`--functional-json`) and tests |
| Rust code generation | `src/rust_codegen.rs` | `tests/rust_codegen_integration.rs`, CLI flags/docs |
| TypeScript code generation | `src/typescript_codegen.rs` | `tests/typescript_codegen_integration.rs`, CLI flags/docs |
| Proto code generation | `src/proto_codegen.rs`, `src/schema.rs` (`field_number` semantics) | `tests/proto_codegen_integration.rs`, CLI flags/docs |
| JSON Schema import/export | `src/json_schema_import.rs`, `src/json_schema_export.rs` | `tests/examples_integration.rs` (`examples/generate-from/...`), CLI `from-json-schema` |
| HTML docs output | `src/html_docs_gen.rs`, `src/bin/super-yaml.rs` (`docs`) | docs-related extension commands |
| Output YAML formatting | `src/yaml_writer.rs` | snapshot/fixture expectations if formatting is tested |
| Error taxonomy/messages | `src/error.rs` + callsites | tests that assert error text |

## Test Map

- `tests/section_and_parse.rs`: marker/section parsing and meta shape checks.
- `tests/schema_validation.rs`: schema/type-hint/constraint/version validation behavior.
- `tests/expr_engine.rs`: expression language lexer/parser/eval semantics.
- `tests/resolve_behavior.rs`: env + expression resolution details.
- `tests/data_references.rs`: `$` and sibling path resolution.
- `tests/coercion_constructors.rs`: constructor coercion logic.
- `tests/imports_integration.rs`: file/remote imports and verification behavior.
- `tests/module_integration.rs`: module manifests, registry, import policy.
- `tests/rust_codegen_integration.rs`: Rust generation behavior.
- `tests/typescript_codegen_integration.rs`: TS generation behavior.
- `tests/proto_codegen_integration.rs`: proto generation behavior.
- `tests/examples_integration.rs`: end-to-end fixture parity across `examples/`.

## VS Code Extension Touchpoints

If a feature affects editor UX (diagnostics, preview, commands, conversion):

- `vscode-syaml/src/extension.ts`: command wiring, parser invocation, previews.
- `vscode-syaml/package.json`: command contributions, activation events, settings.
- `vscode-syaml/syntaxes/syaml.tmLanguage.json`: TextMate grammar updates.
- `vscode-syaml/language-configuration.json`: language-level editor behavior.

## Typical Change Checklist

1. Update parser/model surface (`section_scanner`, `mini_yaml`, `ast`) if syntax or structure changed.
2. Update compile semantics (`lib`, `resolve`, `template`, `schema`, `validate`, etc.).
3. Update generators (`rust/ts/proto/json-schema/html`) if the feature changes emitted artifacts.
4. Add/adjust focused tests in the relevant `tests/*.rs` file.
5. Update `examples/*.syaml` + `examples/*.expected.json` when behavior is user-visible.
6. Run local checks:
   - `cargo test --all-targets`
   - `cargo fmt`
   - `cargo clippy --all-targets --all-features`
   - `scripts/check-examples.sh`
7. If extension behavior changed:
   - `cd vscode-syaml && npm ci && npm run compile`

## Notes on Cross-Cutting Changes

- Most syntax or semantic changes require updates in multiple phases (parse + resolve + validate + tests).
- `src/lib.rs` is the best place to verify pipeline order and where a new phase should be inserted.
- Keep error classes in `SyamlError` specific; many tests assert on message content.
