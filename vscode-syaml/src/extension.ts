import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { promisify } from "node:util";
import * as vscode from "vscode";

const execFileAsync = promisify(execFile);

const TOKEN_TYPES = [
  "comment",
  "keyword",
  "string",
  "number",
  "variable",
  "property",
  "type",
  "operator"
] as const;
const TOKEN_LEGEND = new vscode.SemanticTokensLegend([...TOKEN_TYPES], []);

type TokenType = (typeof TOKEN_TYPES)[number];

interface ParserCommand {
  cwd: string;
  command: string;
  argPrefix: string[];
}

interface ExecError extends Error {
  code?: number | string;
  stdout?: string | Buffer;
  stderr?: string | Buffer;
}

export function activate(context: vscode.ExtensionContext): void {
  const diagnostics = vscode.languages.createDiagnosticCollection("syaml");
  const validator = new SyamlValidator(diagnostics);
  const semanticProvider = new SyamlSemanticTokensProvider();

  context.subscriptions.push(diagnostics);
  context.subscriptions.push(
    vscode.languages.registerDocumentSemanticTokensProvider(
      { language: "syaml" },
      semanticProvider,
      TOKEN_LEGEND
    )
  );

  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument((doc) => validator.schedule(doc, "open_or_save"))
  );
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((doc) => validator.schedule(doc, "open_or_save"))
  );
  context.subscriptions.push(
    vscode.workspace.onDidCloseTextDocument((doc) => diagnostics.delete(doc.uri))
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeTextDocument((evt) =>
      validator.schedule(evt.document, "change")
    )
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((evt) => {
      if (!evt.affectsConfiguration("syaml")) {
        return;
      }
      for (const doc of vscode.workspace.textDocuments) {
        validator.schedule(doc, "open_or_save");
      }
    })
  );

  for (const doc of vscode.workspace.textDocuments) {
    validator.schedule(doc, "open_or_save");
  }
}

export function deactivate(): void {}

class SyamlValidator {
  private readonly timers = new Map<string, NodeJS.Timeout>();
  private readonly runs = new Map<string, number>();

  constructor(private readonly diagnostics: vscode.DiagnosticCollection) {}

  schedule(document: vscode.TextDocument, reason: "change" | "open_or_save"): void {
    if (!isSyamlDocument(document)) {
      return;
    }

    const config = vscode.workspace.getConfiguration("syaml", document.uri);
    const onType = config.get<boolean>("validate.onType", true);
    if (reason === "change" && !onType) {
      return;
    }

    const debounceMs = Math.max(
      100,
      config.get<number>("validate.debounceMs", 400)
    );
    const delay = reason === "change" ? debounceMs : 10;
    const key = document.uri.toString();
    const nextRun = (this.runs.get(key) ?? 0) + 1;
    this.runs.set(key, nextRun);

    const currentTimer = this.timers.get(key);
    if (currentTimer) {
      clearTimeout(currentTimer);
    }

    const timer = setTimeout(() => {
      void this.validateUri(document.uri, nextRun);
    }, delay);
    this.timers.set(key, timer);
  }

  private async validateUri(uri: vscode.Uri, runId: number): Promise<void> {
    const key = uri.toString();
    if (this.runs.get(key) !== runId) {
      return;
    }

    const document = vscode.workspace.textDocuments.find(
      (doc) => doc.uri.toString() === key
    );
    if (!document || !isSyamlDocument(document)) {
      this.diagnostics.delete(uri);
      return;
    }

    const result = await this.runValidation(document);
    if (this.runs.get(key) !== runId) {
      return;
    }

    if (result.ok) {
      this.diagnostics.delete(uri);
      return;
    }

    const range = diagnosticRange(document, result.message);
    const diagnostic = new vscode.Diagnostic(
      range,
      result.message,
      vscode.DiagnosticSeverity.Error
    );
    diagnostic.source = "super-yaml";
    this.diagnostics.set(uri, [diagnostic]);
  }

  private async runValidation(
    document: vscode.TextDocument
  ): Promise<{ ok: true } | { ok: false; message: string }> {
    const parser = await resolveParserCommand(document);

    return withInputFile(document, async (inputPath) => {
      const args = [...parser.argPrefix, "validate", inputPath];
      try {
        await execFileAsync(parser.command, args, {
          cwd: parser.cwd,
          timeout: 15000,
          maxBuffer: 1024 * 1024
        });
        return { ok: true };
      } catch (error) {
        const execError = error as ExecError;
        if (execError.code === "ENOENT") {
          return {
            ok: false,
            message:
              "Cannot run SYAML parser. Set syaml.parser.path or install Rust/cargo."
          };
        }

        const output = normalizeExecOutput(execError);
        const message = extractDiagnosticMessage(output);
        return { ok: false, message };
      }
    });
  }
}

class SyamlSemanticTokensProvider
  implements vscode.DocumentSemanticTokensProvider
{
  provideDocumentSemanticTokens(
    document: vscode.TextDocument
  ): vscode.ProviderResult<vscode.SemanticTokens> {
    const collector = new TokenCollector();

    for (let line = 0; line < document.lineCount; line += 1) {
      const text = document.lineAt(line).text;
      if (text.length === 0) {
        continue;
      }

      const commentStart = findCommentStart(text);
      const code = commentStart >= 0 ? text.slice(0, commentStart) : text;

      if (commentStart >= 0) {
        collector.add(line, commentStart, text.length - commentStart, "comment");
      }

      const markerMatch = /^(\s*)(---!syaml\/v0)\s*$/.exec(code);
      if (markerMatch) {
        collector.add(line, markerMatch[1].length, markerMatch[2].length, "keyword");
      }

      const sectionMatch = /^(\s*)(---(?:front_matter|schema|data))\s*$/.exec(code);
      if (sectionMatch) {
        collector.add(
          line,
          sectionMatch[1].length,
          sectionMatch[2].length,
          "keyword"
        );
      }

      const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
      if (keyMatch) {
        const keyOffset = keyMatch[1].length;
        const keyRaw = keyMatch[2];
        const typeMatch = /^(.*?)(\s*<\s*([A-Za-z_][\w.]*)\s*>)\s*$/.exec(keyRaw);

        if (typeMatch) {
          const keyName = typeMatch[1].trim();
          if (keyName.length > 0) {
            const keyStart = keyOffset + keyRaw.indexOf(keyName);
            collector.add(line, keyStart, keyName.length, "property");
          }

          const typeName = typeMatch[3];
          const typeStart = keyOffset + keyRaw.indexOf(typeName);
          collector.add(line, typeStart, typeName.length, "type");
        } else {
          const keyName = keyRaw.trim();
          if (keyName.length > 0) {
            const keyStart = keyOffset + keyRaw.indexOf(keyName);
            collector.add(line, keyStart, keyName.length, "property");
          }
        }
      }

      collectRegexTokens(collector, line, code, /"([^"\\]|\\.)*"|'([^'\\]|\\.)*'/g, "string");
      collectRegexTokens(collector, line, code, /\$\{[^}]+\}/g, "variable");
      collectRegexTokens(collector, line, code, /\benv\.[A-Za-z_][\w.]*/g, "variable");
      collectRegexTokens(collector, line, code, /\bvalue\b/g, "variable");
      collectRegexTokens(collector, line, code, /\b(true|false|null)\b/g, "keyword");
      collectRegexTokens(collector, line, code, /\b-?\d+(?:\.\d+)?\b/g, "number");
      collectRegexTokens(
        collector,
        line,
        code,
        /(==|!=|<=|>=|&&|\|\||[+\-*/%<>=!])/g,
        "operator"
      );
    }

    const builder = new vscode.SemanticTokensBuilder(TOKEN_LEGEND);
    collector.emit(builder);
    return builder.build();
  }
}

class TokenCollector {
  private readonly tokens: Array<{
    line: number;
    start: number;
    end: number;
    type: TokenType;
  }> = [];

  add(line: number, start: number, length: number, type: TokenType): void {
    if (length <= 0 || start < 0) {
      return;
    }

    const end = start + length;
    for (const token of this.tokens) {
      if (token.line !== line) {
        continue;
      }
      if (end <= token.start || start >= token.end) {
        continue;
      }
      return;
    }

    this.tokens.push({ line, start, end, type });
  }

  emit(builder: vscode.SemanticTokensBuilder): void {
    this.tokens.sort((a, b) => {
      if (a.line !== b.line) {
        return a.line - b.line;
      }
      return a.start - b.start;
    });

    for (const token of this.tokens) {
      builder.push(
        new vscode.Range(token.line, token.start, token.line, token.end),
        token.type
      );
    }
  }
}

function collectRegexTokens(
  collector: TokenCollector,
  line: number,
  text: string,
  regex: RegExp,
  tokenType: TokenType
): void {
  regex.lastIndex = 0;
  for (let match = regex.exec(text); match; match = regex.exec(text)) {
    const start = match.index;
    const value = match[0];
    if (value.length > 0) {
      collector.add(line, start, value.length, tokenType);
    }
    if (!regex.global) {
      break;
    }
  }
}

function isSyamlDocument(document: vscode.TextDocument): boolean {
  if (document.languageId === "syaml") {
    return true;
  }
  if (document.uri.scheme !== "file") {
    return false;
  }
  return document.uri.fsPath.endsWith(".syaml");
}

async function resolveParserCommand(document: vscode.TextDocument): Promise<ParserCommand> {
  const config = vscode.workspace.getConfiguration("syaml", document.uri);
  const configuredPath = config.get<string>("parser.path", "").trim();
  const workspace = vscode.workspace.getWorkspaceFolder(document.uri);
  const cwd =
    workspace?.uri.fsPath ??
    (document.uri.scheme === "file"
      ? path.dirname(document.uri.fsPath)
      : process.cwd());

  if (configuredPath.length > 0) {
    return { cwd, command: configuredPath, argPrefix: [] };
  }

  if (workspace) {
    const localBinary = path.join(workspace.uri.fsPath, "target", "debug", "super-yaml");
    if (await fileExists(localBinary)) {
      return { cwd: workspace.uri.fsPath, command: localBinary, argPrefix: [] };
    }
  }

  return {
    cwd,
    command: "cargo",
    argPrefix: ["run", "--quiet", "--bin", "super-yaml", "--"]
  };
}

async function withInputFile<T>(
  document: vscode.TextDocument,
  run: (inputPath: string) => Promise<T>
): Promise<T> {
  const canUseSourcePath = document.uri.scheme === "file" && !document.isDirty;
  if (canUseSourcePath) {
    return run(document.uri.fsPath);
  }

  const tempPath = path.join(
    os.tmpdir(),
    `syaml-vscode-${Date.now()}-${Math.random().toString(16).slice(2)}.syaml`
  );
  await fs.writeFile(tempPath, document.getText(), "utf8");
  try {
    return await run(tempPath);
  } finally {
    void fs.unlink(tempPath).catch(() => undefined);
  }
}

async function fileExists(filePath: string): Promise<boolean> {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

function normalizeExecOutput(error: ExecError): string {
  const parts: string[] = [];
  if (typeof error.stderr === "string") {
    parts.push(error.stderr);
  } else if (error.stderr) {
    parts.push(error.stderr.toString("utf8"));
  }
  if (typeof error.stdout === "string") {
    parts.push(error.stdout);
  } else if (error.stdout) {
    parts.push(error.stdout.toString("utf8"));
  }
  parts.push(error.message);
  return parts.join("\n");
}

function extractDiagnosticMessage(rawOutput: string): string {
  const lines = rawOutput
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);

  const filtered = lines.filter((line) => {
    const lower = line.toLowerCase();
    if (lower.startsWith("usage:")) {
      return false;
    }
    if (lower.startsWith("super-yaml ")) {
      return false;
    }
    return true;
  });

  const message = (filtered[0] ?? lines[0] ?? "SYAML validation failed").replace(
    /^error:\s*/i,
    ""
  );
  return message;
}

function diagnosticRange(document: vscode.TextDocument, message: string): vscode.Range {
  const absoluteLineNumber = inferAbsoluteLineNumber(document, message);
  if (!absoluteLineNumber) {
    return new vscode.Range(0, 0, 0, Math.max(1, document.lineAt(0).text.length));
  }

  const lineIndex = Math.max(0, Math.min(document.lineCount - 1, absoluteLineNumber - 1));
  const length = Math.max(1, document.lineAt(lineIndex).text.length);
  return new vscode.Range(lineIndex, 0, lineIndex, length);
}

function inferAbsoluteLineNumber(
  document: vscode.TextDocument,
  message: string
): number | undefined {
  const lineMatch = /\bline\s+(\d+)\b/i.exec(message);
  if (!lineMatch) {
    return undefined;
  }

  const parsedLine = Number.parseInt(lineMatch[1], 10);
  if (!Number.isFinite(parsedLine) || parsedLine < 1) {
    return undefined;
  }

  const sectionMatch = /section\s+'([^']+)'/i.exec(message);
  if (!sectionMatch) {
    return parsedLine;
  }

  const sectionName = sectionMatch[1];
  for (let i = 0; i < document.lineCount; i += 1) {
    const lineText = document.lineAt(i).text.trim();
    if (lineText === `---${sectionName}`) {
      // Reported parser lines are section-relative, so convert to absolute.
      return i + 1 + parsedLine;
    }
  }

  return parsedLine;
}

function findCommentStart(text: string): number {
  let inSingle = false;
  let inDouble = false;
  let escaped = false;

  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];

    if (escaped) {
      escaped = false;
      continue;
    }

    if (inDouble && ch === "\\") {
      escaped = true;
      continue;
    }

    if (!inDouble && ch === "'") {
      inSingle = !inSingle;
      continue;
    }
    if (!inSingle && ch === "\"") {
      inDouble = !inDouble;
      continue;
    }

    if (!inSingle && !inDouble && ch === "#") {
      return i;
    }
  }

  return -1;
}
