# SYAML VS Code Extension (Scaffold)

This extension adds:

- `.syaml` language registration
- syntax highlighting (TextMate fallback + semantic tokens)
- parser-backed diagnostics via `super-yaml validate`

## How parser invocation works

In order, the extension tries:

1. `syaml.parser.path` (if set)
2. `target/debug/super-yaml` in the current workspace
3. `cargo run --quiet --bin super-yaml -- validate <file>`

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
