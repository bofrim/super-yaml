# AGENTS Guide

This file defines how LLM agents should approach changes in this repository.

## Purpose

- Keep changes correct, test-backed, and documented.
- Keep behavior, examples, and docs in sync.
- Avoid partial feature work that lands without validation artifacts.

## Canonical Documents

- `DEVELOPMENT.md`
  - Contributor map of the codebase.
  - Primary planning reference for where feature changes belong.
  - Read this before proposing or implementing non-trivial changes.

- `LLMs.md`
  - Model-facing language and usage reference for `.syaml`.
  - Defines how agents should interpret and generate SYAML constructs.
  - Update when language semantics, syntax, or feature behavior changes.

- `README.md`
  - User-facing project documentation (CLI, features, usage).
  - Update when user-visible behavior changes.

## Required Workflow for New Work

1. Identify feature type and affected subsystems using `DEVELOPMENT.md`.
2. Inspect existing tests in `tests/` for nearby behavior.
3. Implement code changes in the smallest coherent set of files.
4. Add or update tests for behavior changes.
5. Add or update examples for user-visible features.
6. Update docs (`README.md`, `LLMs.md`, and `DEVELOPMENT.md` if architecture/process changed).
7. Run validation commands and report results.

## Non-Negotiable Requirements

- Tests are required:
  - Every behavior change must include test coverage in `tests/*.rs`.
  - Bug fixes must include a regression test.
  - Do not merge feature logic without at least one failing-before / passing-after test case.

- Examples are required for new user-visible features:
  - Add a representative `.syaml` example in `examples/`.
  - Add matching expected output (`.expected.json` or other expected artifact when appropriate).
  - Ensure `tests/examples_integration.rs` includes or validates the new example.

- Documentation must stay current:
  - Update `README.md` for externally visible behavior (CLI flags, sections, syntax, outputs, limitations).
  - Update `LLMs.md` for SYAML authoring/interpretation changes that affect model behavior.
  - Update `DEVELOPMENT.md` if pipeline stages, module responsibilities, or change touchpoints shift.

## Change-Type Reminders

- Parser/section/schema changes usually require updates across:
  - `src/section_scanner.rs`, `src/mini_yaml.rs`, `src/schema.rs`, `src/lib.rs`, `src/validate.rs`
  - plus tests and docs.

- Codegen changes usually require updates across:
  - `src/rust_codegen.rs`, `src/typescript_codegen.rs`, `src/proto_codegen.rs`
  - plus codegen integration tests and README/LLMs docs if output contracts changed.

- Import/module/security changes usually require updates across:
  - `src/lib.rs`, `src/fetch.rs`, `src/module.rs`, `src/verify.rs`
  - plus imports/module integration tests and docs.

## Validation Commands

Run relevant checks after changes:

- `cargo test --all-targets`
- `cargo fmt`
- `cargo clippy --all-targets --all-features`
- `scripts/check-examples.sh`

If extension behavior is affected:

- `cd vscode-syaml && npm ci && npm run compile`

## Documentation Sync Policy

When behavior changes, treat docs as part of the same change:

- Code and docs should land together.
- If behavior changed but docs were not updated, that is an incomplete change.
- Specifically verify `LLMs.md` and `README.md` for any needed updates before finishing.
