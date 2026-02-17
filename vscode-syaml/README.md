# SYAML VS Code Extension (Scaffold)

This extension adds:

- `.syaml` language registration
- syntax highlighting (TextMate fallback + semantic tokens)
- parser-backed diagnostics via `super-yaml validate`

## How parser invocation works

In order, the extension tries:

1. `syaml.parser.path` (if set)
2. bundled binary in the extension: `bin/<platform>-<arch>/super-yaml`
3. `target/debug/super-yaml` in the current workspace
4. `rust/target/debug/super-yaml` in the current workspace
5. `super-yaml` from `PATH`
6. `cargo run --quiet --bin super-yaml -- validate <file>`

## Bundling parser binaries into VSIX

Put parser binaries under:

- `bin/darwin-arm64/super-yaml`
- `bin/darwin-x64/super-yaml`
- `bin/linux-x64/super-yaml`
- `bin/linux-arm64/super-yaml`
- `bin/win32-x64/super-yaml.exe`
- `bin/win32-arm64/super-yaml.exe`

For local development on your current machine:

```bash
cd /Users/bradofrim/git/super_yaml/vscode-syaml
npm run bundle:parser:local
```

## CI and release automation

This repository includes GitHub Actions workflows for extension packaging:

- `.github/workflows/ci.yml` runs Rust tests plus extension TypeScript compile on pushes/PRs.
- `.github/workflows/vscode-extension-release.yml` builds parser binaries for multiple platforms, packages the VSIX, and publishes a GitHub release when a tag like `v0.1.0` is pushed.

## Local dev

```bash
cd /Users/bradofrim/git/super_yaml/vscode-syaml
npm install
npm run compile
```

Then open this folder in VS Code and press `F5` to run an Extension Development Host.

## Recommended settings

If you already built the parser binary, point directly at it:

```json
{
  "syaml.parser.path": "/Users/bradofrim/git/super_yaml/target/debug/super-yaml"
}
```
