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
  "class",
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

type PreviewFormat = "yaml" | "json" | "rust" | "typescript" | "proto";

const PREVIEW_FORMAT_META: Record<
  PreviewFormat,
  { cliFlag: string; ext: string }
> = {
  yaml:       { cliFlag: "yaml",  ext: ".yaml" },
  json:       { cliFlag: "json",  ext: ".json" },
  rust:       { cliFlag: "rust",  ext: ".rs"   },
  typescript: { cliFlag: "ts",    ext: ".ts"   },
  proto:      { cliFlag: "proto", ext: ".proto" }
};

// Import source formats: schema/definition languages that can be converted to .syaml.
// Extend this union and IMPORT_SOURCE_META to support additional source formats.
type ImportSourceFormat = "json-schema";

const IMPORT_SOURCE_META: Record<
  ImportSourceFormat,
  { cliCommand: string; label: string; extensions: string[] }
> = {
  "json-schema": {
    cliCommand: "from-json-schema",
    label: "JSON Schema",
    extensions: [".json"]
  }
};

function detectImportSourceFormat(
  document: vscode.TextDocument
): ImportSourceFormat | undefined {
  const ext = path.extname(document.uri.fsPath).toLowerCase();
  for (const [format, meta] of Object.entries(IMPORT_SOURCE_META) as [
    ImportSourceFormat,
    (typeof IMPORT_SOURCE_META)[ImportSourceFormat]
  ][]) {
    if (meta.extensions.includes(ext)) {
      return format;
    }
  }
  return undefined;
}

export function activate(context: vscode.ExtensionContext): void {
  const diagnostics = vscode.languages.createDiagnosticCollection("syaml");
  const validator = new SyamlValidator(diagnostics, context.extensionPath);
  const semanticProvider = new SyamlSemanticTokensProvider();
  const typeDefinitionProvider = new SyamlTypeDefinitionProvider();
  const typeReferenceProvider = new SyamlTypeReferenceProvider();
  const inlayHintsProvider = new SyamlInlayHintsProvider();
  const previewProvider = new SyamlPreviewContentProvider();

  context.subscriptions.push(diagnostics);
  context.subscriptions.push(previewProvider);
  context.subscriptions.push(
    vscode.languages.registerDocumentSemanticTokensProvider(
      { language: "syaml" },
      semanticProvider,
      TOKEN_LEGEND
    )
  );
  context.subscriptions.push(
    vscode.languages.registerDefinitionProvider(
      { language: "syaml" },
      typeDefinitionProvider
    )
  );
  context.subscriptions.push(
    vscode.languages.registerReferenceProvider(
      { language: "syaml" },
      typeReferenceProvider
    )
  );
  context.subscriptions.push(
    vscode.languages.registerInlayHintsProvider(
      { language: "syaml" },
      inlayHintsProvider
    )
  );
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(
      "syaml-preview",
      previewProvider
    )
  );
  const registerPreviewCommand = (
    commandId: string,
    format: PreviewFormat
  ): vscode.Disposable =>
    vscode.commands.registerCommand(commandId, async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (!document || !isSyamlDocument(document)) {
        void vscode.window.showInformationMessage(
          "Open a .syaml document to preview expanded output."
        );
        return;
      }
      await previewExpandedOutput(document, context.extensionPath, previewProvider, format);
    });

  context.subscriptions.push(registerPreviewCommand("syaml.previewExpandedOutput", "yaml"));
  context.subscriptions.push(registerPreviewCommand("syaml.previewJson", "json"));
  context.subscriptions.push(registerPreviewCommand("syaml.previewRust", "rust"));
  context.subscriptions.push(registerPreviewCommand("syaml.previewTypeScript", "typescript"));
  context.subscriptions.push(registerPreviewCommand("syaml.previewProto", "proto"));

  const registerSaveCommand = (
    commandId: string,
    format: PreviewFormat
  ): vscode.Disposable =>
    vscode.commands.registerCommand(commandId, async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (!document || !isSyamlDocument(document)) {
        void vscode.window.showInformationMessage(
          "Open a .syaml document to save generated output."
        );
        return;
      }
      await saveExpandedOutput(document, context.extensionPath, format);
    });

  context.subscriptions.push(registerSaveCommand("syaml.saveExpandedOutput", "yaml"));
  context.subscriptions.push(registerSaveCommand("syaml.saveJson", "json"));
  context.subscriptions.push(registerSaveCommand("syaml.saveRust", "rust"));
  context.subscriptions.push(registerSaveCommand("syaml.saveTypeScript", "typescript"));
  context.subscriptions.push(registerSaveCommand("syaml.saveProto", "proto"));

  context.subscriptions.push(
    vscode.commands.registerCommand("syaml.import.preview", async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (!document) {
        void vscode.window.showInformationMessage(
          "Open a schema file to preview its SYAML conversion."
        );
        return;
      }
      await previewImportAsSyaml(document, context.extensionPath, previewProvider);
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("syaml.import.save", async () => {
      const document = vscode.window.activeTextEditor?.document;
      if (!document) {
        void vscode.window.showInformationMessage(
          "Open a schema file to convert and save as SYAML."
        );
        return;
      }
      await saveImportAsSyaml(document, context.extensionPath);
    })
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

class SyamlPreviewContentProvider implements vscode.TextDocumentContentProvider {
  private readonly emitter = new vscode.EventEmitter<vscode.Uri>();
  private readonly contentByUri = new Map<string, string>();
  readonly onDidChange = this.emitter.event;

  provideTextDocumentContent(uri: vscode.Uri): string {
    return (
      this.contentByUri.get(uri.toString()) ??
      "# No preview content available.\n"
    );
  }

  setContent(uri: vscode.Uri, content: string): void {
    this.contentByUri.set(uri.toString(), content);
    this.emitter.fire(uri);
  }

  dispose(): void {
    this.contentByUri.clear();
    this.emitter.dispose();
  }
}

class SyamlValidator {
  private readonly timers = new Map<string, NodeJS.Timeout>();
  private readonly runs = new Map<string, number>();

  constructor(
    private readonly diagnostics: vscode.DiagnosticCollection,
    private readonly extensionPath: string
  ) {}

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
    let parser: ParserCommand;
    try {
      parser = await resolveParserCommand(document, this.extensionPath);
    } catch {
      return {
        ok: false,
        message: "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
      };
    }

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
              "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
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
    const typeDefinitionKeysByLine = collectTypeDefinitionKeyRangesByLine(document);
    let currentSection: "meta" | "schema" | "data" | undefined;

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

      const sectionMatch = /^(\s*)---(meta|schema|data)\s*$/.exec(code);
      if (sectionMatch) {
        currentSection = sectionMatch[2] as "meta" | "schema" | "data";
        const marker = `---${sectionMatch[2]}`;
        collector.add(
          line,
          sectionMatch[1].length,
          marker.length,
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
            collector.add(
              line,
              keyStart,
              keyName.length,
              isTypeDefinitionKeyRange(
                typeDefinitionKeysByLine,
                line,
                keyStart,
                keyName.length
              )
                ? "class"
                : "property"
            );
          }

          const typeName = typeMatch[3];
          const typeStart = keyOffset + keyRaw.indexOf(typeName);
          collector.add(
            line,
            typeStart,
            typeName.length,
            isBuiltinTypeName(typeName) ? "keyword" : "type"
          );
        } else {
          const keyName = keyRaw.trim();
          if (keyName.length > 0) {
            const keyStart = keyOffset + keyRaw.indexOf(keyName);
            collector.add(
              line,
              keyStart,
              keyName.length,
              isTypeDefinitionKeyRange(
                typeDefinitionKeysByLine,
                line,
                keyStart,
                keyName.length
              )
                ? "class"
                : "property"
            );
          }
        }
      }

      if (currentSection === "schema") {
        for (const reference of parseSchemaTypeValueReferences(code)) {
          collector.add(
            line,
            reference.start,
            reference.name.length,
            reference.isBuiltin ? "keyword" : "type"
          );
          if (reference.optionalMarkerStart !== undefined) {
            collector.add(line, reference.optionalMarkerStart, 1, "operator");
          }
        }
        for (const reference of parseSchemaFromEnumTypeReferences(code)) {
          collector.add(
            line,
            reference.start,
            reference.name.length,
            reference.isBuiltin ? "keyword" : "type"
          );
        }

        for (const shorthand of parseSchemaInlineTypeShorthandReferences(code)) {
          collector.add(
            line,
            shorthand.start,
            shorthand.name.length,
            shorthand.isBuiltin ? "keyword" : "type"
          );
          if (shorthand.optionalMarkerStart !== undefined) {
            collector.add(line, shorthand.optionalMarkerStart, 1, "operator");
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

interface TypeOccurrence {
  name: string;
  range: vscode.Range;
  kind: "definition" | "reference";
}

interface ImportNamespace {
  alias: string;
  aliasRange: vscode.Range;
  importPathRange?: vscode.Range;
  importUri?: vscode.Uri;
}

class SyamlTypeDefinitionProvider implements vscode.DefinitionProvider {
  async provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position
  ): Promise<vscode.Definition | undefined> {
    if (!isSyamlDocument(document)) {
      return undefined;
    }

    const importNamespaces = collectImportNamespaces(document);
    const importPathDefinition = findImportPathReferenceAtPosition(
      position,
      importNamespaces
    );
    if (importPathDefinition) {
      return importPathDefinition;
    }
    const namespaceDefinition = findImportNamespaceReferenceAtPosition(
      document,
      position,
      importNamespaces
    );
    if (namespaceDefinition) {
      return namespaceDefinition;
    }

    const occurrences = collectTypeOccurrences(document);
    const target = findTypeOccurrenceAtPosition(occurrences, position);
    if (!target) {
      return findTypedDataKeySchemaDefinition(document, position);
    }

    const importedLocation = await findImportedDefinitionForTypeOccurrence(
      document,
      target,
      position,
      importNamespaces
    );
    if (importedLocation) {
      return importedLocation;
    }

    const definitions = occurrences.filter(
      (occurrence) =>
        occurrence.kind === "definition" && occurrence.name === target.name
    );
    if (definitions.length === 0) {
      return undefined;
    }

    return definitions.map(
      (definition) => new vscode.Location(document.uri, definition.range)
    );
  }
}

class SyamlTypeReferenceProvider implements vscode.ReferenceProvider {
  provideReferences(
    document: vscode.TextDocument,
    position: vscode.Position,
    context: vscode.ReferenceContext
  ): vscode.ProviderResult<vscode.Location[]> {
    if (!isSyamlDocument(document)) {
      return undefined;
    }

    const occurrences = collectTypeOccurrences(document);
    const target = findTypeOccurrenceAtPosition(occurrences, position);
    if (!target) {
      return undefined;
    }

    const references = occurrences.filter((occurrence) => {
      if (occurrence.name !== target.name) {
        return false;
      }
      if (context.includeDeclaration) {
        return true;
      }
      return occurrence.kind !== "definition";
    });

    return references.map(
      (reference) => new vscode.Location(document.uri, reference.range)
    );
  }
}

class SyamlInlayHintsProvider implements vscode.InlayHintsProvider {
  async provideInlayHints(
    document: vscode.TextDocument,
    range: vscode.Range
  ): Promise<vscode.InlayHint[]> {
    if (!isSyamlDocument(document)) {
      return [];
    }

    const schemaIndex = await buildSchemaTypeIndexWithImports(document);
    if (schemaIndex.roots.size === 0) {
      return [];
    }

    const locations = collectDataKeyLocations(document);
    if (locations.length === 0) {
      return [];
    }

    const explicitHints = new Map<string, string>();
    for (const location of locations) {
      if (!location.explicitTypeHint) {
        continue;
      }
      explicitHints.set(location.path, location.explicitTypeHint);
    }

    const hints: vscode.InlayHint[] = [];
    for (const location of locations) {
      if (location.explicitTypeHint) {
        continue;
      }
      if (location.line < range.start.line || location.line > range.end.line) {
        continue;
      }

      const ancestor = nearestAncestorHint(location.path, explicitHints);
      if (!ancestor) {
        continue;
      }

      const relativeSegments = relativeJsonPathSegments(ancestor.path, location.path);
      if (!relativeSegments || relativeSegments.length === 0) {
        continue;
      }

      const inferredType = inferSchemaTypeFromAncestor(
        schemaIndex,
        ancestor.typeName,
        relativeSegments
      );
      if (!inferredType) {
        continue;
      }

      const hint = new vscode.InlayHint(
        new vscode.Position(location.line, location.end),
        shortTypeName(inferredType),
        vscode.InlayHintKind.Type
      );
      hint.paddingLeft = true;
      hint.tooltip = `Inferred from ${shortTypeName(ancestor.typeName)}`;
      hints.push(hint);
    }

    return hints;
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

function collectTypeDefinitionKeyRangesByLine(
  document: vscode.TextDocument
): Map<number, Array<{ start: number; end: number }>> {
  const byLine = new Map<number, Array<{ start: number; end: number }>>();
  for (const definition of collectTypeDefinitions(document)) {
    const line = definition.range.start.line;
    const ranges = byLine.get(line) ?? [];
    ranges.push({
      start: definition.range.start.character,
      end: definition.range.end.character
    });
    byLine.set(line, ranges);
  }
  return byLine;
}

function isTypeDefinitionKeyRange(
  byLine: Map<number, Array<{ start: number; end: number }>>,
  line: number,
  start: number,
  length: number
): boolean {
  const ranges = byLine.get(line);
  if (!ranges) {
    return false;
  }
  const end = start + length;
  return ranges.some((range) => range.start === start && range.end === end);
}

function collectTypeOccurrences(document: vscode.TextDocument): TypeOccurrence[] {
  return [
    ...collectTypeDefinitions(document),
    ...collectTypeHintReferences(document),
    ...collectSchemaTypeReferences(document)
  ];
}

function collectTypeDefinitions(document: vscode.TextDocument): TypeOccurrence[] {
  const bounds = findSectionBounds(document, "schema");
  if (!bounds) {
    return [];
  }

  const occurrences: TypeOccurrence[] = [];
  const stack: Array<{ indent: number; key: string }> = [];

  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    const rawKey = keyMatch[2];
    const keyName = rawKey.trim();
    if (keyName.length === 0) {
      continue;
    }

    const type = parseTypeDefinitionName(rawKey);
    if (!type) {
      continue;
    }

    const keyStart = indent + type.offset;
    while (stack.length > 0 && stack[stack.length - 1].indent >= indent) {
      stack.pop();
    }
    stack.push({ indent, key: keyName });

    const fullPath = stack.map((entry) => entry.key);
    const isTopLevelType = fullPath.length === 1 && keyName !== "types";
    const isLegacyWrappedType = fullPath.length === 2 && fullPath[0] === "types";
    if (!isTopLevelType && !isLegacyWrappedType) {
      continue;
    }

    occurrences.push({
      name: type.name,
      range: new vscode.Range(line, keyStart, line, keyStart + type.name.length),
      kind: "definition"
    });
  }

  return occurrences;
}

function collectTypeHintReferences(document: vscode.TextDocument): TypeOccurrence[] {
  const bounds = findSectionBounds(document, "data");
  if (!bounds) {
    return [];
  }

  const occurrences: TypeOccurrence[] = [];
  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const dashMatch = /^(\s*)-(\s*)(.*)$/.exec(code);
    if (dashMatch) {
      const inlineKeyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(dashMatch[3]);
      if (inlineKeyMatch) {
        const keyOffset =
          dashMatch[1].length +
          1 +
          dashMatch[2].length +
          inlineKeyMatch[1].length;
        pushTypeHintReference(occurrences, line, inlineKeyMatch[2], keyOffset);
      }
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
    if (!keyMatch) {
      continue;
    }
    const keyOffset = keyMatch[1].length;
    pushTypeHintReference(occurrences, line, keyMatch[2], keyOffset);
  }

  return occurrences;
}

function collectSchemaTypeReferences(document: vscode.TextDocument): TypeOccurrence[] {
  const bounds = findSectionBounds(document, "schema");
  if (!bounds) {
    return [];
  }

  const occurrences: TypeOccurrence[] = [];
  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    for (const reference of parseSchemaTypeValueReferences(code)) {
      if (reference.isBuiltin) {
        continue;
      }
      occurrences.push({
        name: reference.name,
        range: new vscode.Range(
          line,
          reference.start,
          line,
          reference.start + reference.name.length
        ),
        kind: "reference"
      });
    }
    for (const reference of parseSchemaFromEnumTypeReferences(code)) {
      if (reference.isBuiltin) {
        continue;
      }
      occurrences.push({
        name: reference.name,
        range: new vscode.Range(
          line,
          reference.start,
          line,
          reference.start + reference.name.length
        ),
        kind: "reference"
      });
    }

    for (const shorthand of parseSchemaInlineTypeShorthandReferences(code)) {
      if (shorthand.isBuiltin) {
        continue;
      }
      occurrences.push({
        name: shorthand.name,
        range: new vscode.Range(
          line,
          shorthand.start,
          line,
          shorthand.start + shorthand.name.length
        ),
        kind: "reference"
      });
    }
  }

  return occurrences;
}

function buildSchemaTypeIndex(document: vscode.TextDocument): SchemaTypeIndex {
  const bounds = findSectionBounds(document, "schema");
  const roots = new Map<string, SchemaNodeLite>();
  if (!bounds) {
    return { roots };
  }

  const stack: Array<{ indent: number; key: string }> = [];
  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):(.*)$/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    const rawKey = keyMatch[2];
    const keyName = rawKey.trim();
    if (keyName.length === 0) {
      continue;
    }
    const valueText = keyMatch[4].trim();
    const keyStart = indent + rawKey.indexOf(keyName);

    while (stack.length > 0 && stack[stack.length - 1].indent >= indent) {
      stack.pop();
    }
    stack.push({ indent, key: keyName });

    const fullPath = stack.map((entry) => entry.key);
    let typeName: string | undefined;
    let relPath: string[] = [];

    if (fullPath[0] === "types") {
      if (fullPath.length < 2) {
        continue;
      }
      typeName = fullPath[1];
      relPath = fullPath.slice(2);
    } else {
      typeName = fullPath[0];
      relPath = fullPath.slice(1);
    }

    if (!typeName || !/^[A-Za-z_][\w.]*$/.test(typeName)) {
      continue;
    }

    const typeRoot = ensureTypeRoot(roots, typeName);
    if (relPath.length === 0) {
      typeRoot.definitionRange = new vscode.Range(
        line,
        keyStart,
        line,
        keyStart + keyName.length
      );
      typeRoot.definitionUri = document.uri;
    }

    if (relPath.length >= 2 && relPath[relPath.length - 2] === "properties") {
      const propertyNode = ensureSchemaNodeByRelPath(typeRoot, relPath);
      propertyNode.definitionRange = new vscode.Range(
        line,
        keyStart,
        line,
        keyStart + keyName.length
      );
      propertyNode.definitionUri = document.uri;
      const inlinePropertyType = extractTypeNameFromValue(valueText);
      if (inlinePropertyType) {
        propertyNode.typeName = inlinePropertyType;
      }
    }

    if (keyName === "type") {
      const parentNode = ensureSchemaNodeByRelPath(typeRoot, relPath.slice(0, -1));
      const declaredType = extractTypeNameFromValue(valueText);
      if (declaredType) {
        parentNode.typeName = declaredType;
      }
      continue;
    }

    if (keyName === "properties") {
      const parentNode = ensureSchemaNodeByRelPath(typeRoot, relPath.slice(0, -1));
      parentNode.hasPropertiesKeyword = true;
      continue;
    }

    if (keyName === "items") {
      const parentNode = ensureSchemaNodeByRelPath(typeRoot, relPath.slice(0, -1));
      parentNode.hasItemsKeyword = true;
      const itemsNode = ensureSchemaNodeByRelPath(typeRoot, relPath);
      const inlineItemType = extractTypeNameFromValue(valueText);
      if (inlineItemType) {
        itemsNode.typeName = inlineItemType;
      }
    }
  }

  return { roots };
}

async function buildSchemaTypeIndexWithImports(
  document: vscode.TextDocument,
  stackUris = new Set<string>(),
  cache = new Map<string, SchemaTypeIndex>()
): Promise<SchemaTypeIndex> {
  if (document.uri.scheme !== "file") {
    return buildSchemaTypeIndex(document);
  }

  const uriKey = document.uri.toString();
  const cached = cache.get(uriKey);
  if (cached) {
    return cached;
  }
  if (stackUris.has(uriKey)) {
    return buildSchemaTypeIndex(document);
  }

  const schemaIndex = buildSchemaTypeIndex(document);
  stackUris.add(uriKey);

  const imports = collectImportNamespaces(document);
  for (const importNamespace of imports.values()) {
    if (!importNamespace.importUri) {
      continue;
    }

    let importedDocument: vscode.TextDocument;
    try {
      importedDocument = await vscode.workspace.openTextDocument(importNamespace.importUri);
    } catch {
      continue;
    }
    if (!isSyamlDocument(importedDocument)) {
      continue;
    }

    const importedIndex = await buildSchemaTypeIndexWithImports(
      importedDocument,
      stackUris,
      cache
    );
    mergeImportedSchemaTypeIndex(schemaIndex, importNamespace.alias, importedIndex);
  }

  stackUris.delete(uriKey);
  cache.set(uriKey, schemaIndex);
  return schemaIndex;
}

function mergeImportedSchemaTypeIndex(
  target: SchemaTypeIndex,
  alias: string,
  imported: SchemaTypeIndex
): void {
  if (imported.roots.size === 0) {
    return;
  }

  const renameMap = new Map<string, string>();
  for (const name of imported.roots.keys()) {
    renameMap.set(name, `${alias}.${name}`);
  }

  for (const [name, node] of imported.roots) {
    const namespacedName = renameMap.get(name);
    if (!namespacedName || target.roots.has(namespacedName)) {
      continue;
    }
    const rewritten = cloneSchemaNodeWithRenamedTypeRefs(node, renameMap);
    target.roots.set(namespacedName, rewritten);
  }
}

function cloneSchemaNodeWithRenamedTypeRefs(
  source: SchemaNodeLite,
  renameMap: Map<string, string>
): SchemaNodeLite {
  const cloned = createSchemaNode();
  cloned.definitionRange = source.definitionRange;
  cloned.definitionUri = source.definitionUri;
  cloned.hasPropertiesKeyword = source.hasPropertiesKeyword;
  cloned.hasItemsKeyword = source.hasItemsKeyword;

  const sourceTypeName = source.typeName;
  if (sourceTypeName && renameMap.has(sourceTypeName) && !isBuiltinTypeName(sourceTypeName)) {
    cloned.typeName = renameMap.get(sourceTypeName);
  } else {
    cloned.typeName = sourceTypeName;
  }

  for (const [propertyName, propertyNode] of source.properties) {
    cloned.properties.set(
      propertyName,
      cloneSchemaNodeWithRenamedTypeRefs(propertyNode, renameMap)
    );
  }

  if (source.items) {
    cloned.items = cloneSchemaNodeWithRenamedTypeRefs(source.items, renameMap);
  }

  return cloned;
}

function shortTypeName(typeName: string): string {
  const lastDot = typeName.lastIndexOf(".");
  if (lastDot < 0 || lastDot === typeName.length - 1) {
    return typeName;
  }
  return typeName.slice(lastDot + 1);
}

function ensureTypeRoot(
  roots: Map<string, SchemaNodeLite>,
  typeName: string
): SchemaNodeLite {
  const existing = roots.get(typeName);
  if (existing) {
    return existing;
  }
  const created = createSchemaNode();
  roots.set(typeName, created);
  return created;
}

function createSchemaNode(): SchemaNodeLite {
  return {
    properties: new Map<string, SchemaNodeLite>(),
    hasPropertiesKeyword: false,
    hasItemsKeyword: false
  };
}

function ensureSchemaNodeByRelPath(
  root: SchemaNodeLite,
  relPath: string[]
): SchemaNodeLite {
  let current = root;
  for (let i = 0; i < relPath.length; i += 1) {
    const segment = relPath[i];
    if (segment === "properties") {
      const propName = relPath[i + 1];
      if (!propName) {
        break;
      }
      let child = current.properties.get(propName);
      if (!child) {
        child = createSchemaNode();
        current.properties.set(propName, child);
      }
      current = child;
      i += 1;
      continue;
    }

    if (segment === "items") {
      if (!current.items) {
        current.items = createSchemaNode();
      }
      current = current.items;
    }
  }
  return current;
}

function extractTypeNameFromValue(valueText: string): string | undefined {
  if (valueText.length === 0) {
    return undefined;
  }

  const quoted = /^(["'])([A-Za-z_][\w.]*)(\?)?\1$/.exec(valueText);
  if (quoted) {
    return quoted[2];
  }

  const plain = /^([A-Za-z_][\w.]*)(\?)?$/.exec(valueText);
  if (plain) {
    return plain[1];
  }

  const inlineType =
    /(?:^|[{,]\s*)type\s*:\s*(?:"([A-Za-z_][\w.]*)(\?)?"|'([A-Za-z_][\w.]*)(\?)?'|([A-Za-z_][\w.]*)(\?)?)(?:\s*[,}].*|\s*$)/.exec(
      valueText
    );
  if (!inlineType) {
    if (isInlineStringEnumShorthand(valueText)) {
      return "string";
    }
    return undefined;
  }
  return inlineType[1] ?? inlineType[3] ?? inlineType[5];
}

function isInlineStringEnumShorthand(valueText: string): boolean {
  const trimmed = valueText.trim();
  if (!trimmed.startsWith("[") || !trimmed.endsWith("]")) {
    return false;
  }

  const inner = trimmed.slice(1, -1).trim();
  if (inner.length === 0) {
    return true;
  }

  const items: string[] = [];
  let current = "";
  let quote: '"' | "'" | undefined;
  for (let i = 0; i < inner.length; i += 1) {
    const ch = inner[i];
    if (quote) {
      current += ch;
      if (ch === quote && inner[i - 1] !== "\\") {
        quote = undefined;
      }
      continue;
    }

    if (ch === '"' || ch === "'") {
      quote = ch;
      current += ch;
      continue;
    }

    if (ch === ",") {
      items.push(current.trim());
      current = "";
      continue;
    }

    current += ch;
  }
  if (quote) {
    return false;
  }
  items.push(current.trim());

  if (items.some((item) => item.length === 0)) {
    return false;
  }

  return items.every((item) => isStringLikeEnumItem(item));
}

function isStringLikeEnumItem(item: string): boolean {
  if ((item.startsWith('"') && item.endsWith('"')) || (item.startsWith("'") && item.endsWith("'"))) {
    return item.length >= 2;
  }

  if (/^(true|false|null)$/i.test(item)) {
    return false;
  }
  if (/^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$/.test(item)) {
    return false;
  }
  if (item.startsWith("{") || item.startsWith("[") || item.includes(":")) {
    return false;
  }
  return true;
}

function inferSchemaTypeFromAncestor(
  index: SchemaTypeIndex,
  ancestorType: string,
  segments: JsonPathSegment[]
): string | undefined {
  const root = createSchemaNode();
  root.typeName = ancestorType;

  let current = root;
  for (const segment of segments) {
    const next = descendSchemaForInference(index, current, segment, new Set<string>());
    if (!next) {
      return undefined;
    }
    current = next;
  }

  if (!current.typeName || !/^[A-Za-z_][\w.]*$/.test(current.typeName)) {
    return undefined;
  }
  return current.typeName;
}

function descendSchemaForInference(
  index: SchemaTypeIndex,
  current: SchemaNodeLite,
  segment: JsonPathSegment,
  visitedTypes: Set<string>
): SchemaNodeLite | undefined {
  if (segment.kind === "key") {
    const direct = current.properties.get(String(segment.value));
    if (direct) {
      return direct;
    }
    if (current.hasPropertiesKeyword) {
      return undefined;
    }
  } else {
    if (current.items) {
      return current.items;
    }
    if (current.hasItemsKeyword) {
      return undefined;
    }
  }

  const declaredType = current.typeName;
  if (!declaredType) {
    return undefined;
  }
  if (isBuiltinTypeName(declaredType)) {
    if (segment.kind === "key" && declaredType !== "object") {
      return undefined;
    }
    if (segment.kind === "index" && declaredType !== "array") {
      return undefined;
    }
    return undefined;
  }

  if (visitedTypes.has(declaredType)) {
    return undefined;
  }
  const referenced = index.roots.get(declaredType);
  if (!referenced) {
    return undefined;
  }

  const nestedVisited = new Set(visitedTypes);
  nestedVisited.add(declaredType);
  return descendSchemaForInference(index, referenced, segment, nestedVisited);
}

function nearestAncestorHint(
  path: string,
  hintsByPath: Map<string, string>
): { path: string; typeName: string } | undefined {
  let current = parentJsonPath(path);
  while (current) {
    const typeName = hintsByPath.get(current);
    if (typeName) {
      return { path: current, typeName };
    }
    current = parentJsonPath(current);
  }
  return undefined;
}

function relativeJsonPathSegments(
  ancestorPath: string,
  path: string
): JsonPathSegment[] | undefined {
  const ancestorSegments = parseJsonPathSegments(ancestorPath);
  const pathSegments = parseJsonPathSegments(path);
  if (!ancestorSegments || !pathSegments) {
    return undefined;
  }
  if (ancestorSegments.length > pathSegments.length) {
    return undefined;
  }

  for (let i = 0; i < ancestorSegments.length; i += 1) {
    if (
      ancestorSegments[i].kind !== pathSegments[i].kind ||
      ancestorSegments[i].value !== pathSegments[i].value
    ) {
      return undefined;
    }
  }
  return pathSegments.slice(ancestorSegments.length);
}

function parseJsonPathSegments(path: string): JsonPathSegment[] | undefined {
  if (path === "$") {
    return [];
  }
  if (!path.startsWith("$.")) {
    return undefined;
  }

  const segments: JsonPathSegment[] = [];
  const chars = path.slice(2);
  let current = "";
  let i = 0;

  while (i < chars.length) {
    const ch = chars[i];
    if (ch === ".") {
      if (current.length > 0) {
        segments.push({ kind: "key", value: current });
        current = "";
      }
      i += 1;
      continue;
    }

    if (ch === "[") {
      if (current.length > 0) {
        segments.push({ kind: "key", value: current });
        current = "";
      }
      i += 1;
      let digits = "";
      while (i < chars.length && chars[i] !== "]") {
        digits += chars[i];
        i += 1;
      }
      if (i >= chars.length || chars[i] !== "]") {
        return undefined;
      }
      i += 1;
      if (!/^\d+$/.test(digits)) {
        return undefined;
      }
      segments.push({ kind: "index", value: Number.parseInt(digits, 10) });
      continue;
    }

    current += ch;
    i += 1;
  }

  if (current.length > 0) {
    segments.push({ kind: "key", value: current });
  }
  return segments;
}

function parentJsonPath(path: string): string | undefined {
  if (path === "$") {
    return undefined;
  }

  let lastSeparator = -1;
  for (let i = 0; i < path.length; i += 1) {
    const ch = path[i];
    if ((ch === "." && i > 1) || ch === "[") {
      lastSeparator = i;
    }
  }

  if (lastSeparator === 1) {
    return "$";
  }
  if (lastSeparator <= 0) {
    return undefined;
  }
  return path.slice(0, lastSeparator);
}

function pushTypeHintReference(
  occurrences: TypeOccurrence[],
  line: number,
  rawKey: string,
  keyOffset: number
): void {
  const typeHint = parseTypeHint(rawKey);
  if (!typeHint) {
    return;
  }
  if (isBuiltinTypeName(typeHint.name)) {
    return;
  }

  const start = keyOffset + typeHint.offset;
  occurrences.push({
    name: typeHint.name,
    range: new vscode.Range(line, start, line, start + typeHint.name.length),
    kind: "reference"
  });
}

function parseTypeDefinitionName(
  rawKey: string
): { name: string; offset: number } | undefined {
  const candidate = rawKey.trim();
  if (!/^[A-Za-z_][\w.]*$/.test(candidate)) {
    return undefined;
  }

  const offset = rawKey.indexOf(candidate);
  if (offset < 0) {
    return undefined;
  }

  return { name: candidate, offset };
}

function parseTypeHint(rawKey: string): { name: string; offset: number } | undefined {
  const match = /^(.*?)(\s*<\s*([A-Za-z_][\w.]*)\s*>)\s*$/.exec(rawKey);
  if (!match) {
    return undefined;
  }

  const typeName = match[3];
  const typeOffsetInHint = match[2].indexOf(typeName);
  if (typeOffsetInHint < 0) {
    return undefined;
  }

  return {
    name: typeName,
    offset: match[1].length + typeOffsetInHint
  };
}

function parseSchemaTypeValueReferences(
  code: string
): Array<{
  name: string;
  start: number;
  isBuiltin: boolean;
  optionalMarkerStart?: number;
}> {
  const references: Array<{
    name: string;
    start: number;
    isBuiltin: boolean;
    optionalMarkerStart?: number;
  }> = [];
  const typeRegex =
    /(?:^\s*|[{,]\s*)type\s*:\s*(?:"([A-Za-z_][\w.]*)(\?)?"|'([A-Za-z_][\w.]*)(\?)?'|([A-Za-z_][\w.]*)(\?)?)/g;

  for (let match = typeRegex.exec(code); match; match = typeRegex.exec(code)) {
    const typeName = match[1] ?? match[3] ?? match[5];
    if (!typeName) {
      continue;
    }

    const optionalSuffix = match[2] ?? match[4] ?? match[6] ?? "";
    const fullToken = `${typeName}${optionalSuffix}`;
    const offsetInMatch = match[0].lastIndexOf(fullToken);
    if (offsetInMatch < 0) {
      continue;
    }
    const start = match.index + offsetInMatch;

    references.push({
      name: typeName,
      start,
      isBuiltin: isBuiltinTypeName(typeName),
      optionalMarkerStart: optionalSuffix.length > 0 ? start + typeName.length : undefined
    });
  }

  return references;
}

function parseSchemaFromEnumTypeReferences(
  code: string
): Array<{
  name: string;
  start: number;
  isBuiltin: boolean;
}> {
  const references: Array<{
    name: string;
    start: number;
    isBuiltin: boolean;
  }> = [];
  const fromEnumRegex =
    /(?:^\s*|[{,]\s*)from_enum\s*:\s*(?:"([A-Za-z_][\w.]*)"|'([A-Za-z_][\w.]*)'|([A-Za-z_][\w.]*))/g;

  for (
    let match = fromEnumRegex.exec(code);
    match;
    match = fromEnumRegex.exec(code)
  ) {
    const typeName = match[1] ?? match[2] ?? match[3];
    if (!typeName) {
      continue;
    }
    const start = match.index + match[0].lastIndexOf(typeName);
    if (start < 0) {
      continue;
    }
    references.push({
      name: typeName,
      start,
      isBuiltin: isBuiltinTypeName(typeName)
    });
  }

  return references;
}

function parseSchemaInlineTypeShorthandReferences(
  code: string
): Array<{
  name: string;
  start: number;
  isBuiltin: boolean;
  optionalMarkerStart?: number;
}> {
  const lineMatch =
    /^(\s*)([^:#\n][^:#\n]*?)(\s*):\s*(?:"([A-Za-z_][\w.]*)(\?)?"|'([A-Za-z_][\w.]*)(\?)?'|([A-Za-z_][\w.]*)(\?)?)\s*$/.exec(
      code
    );
  if (!lineMatch) {
    return [];
  }

  const keyName = lineMatch[2].trim();
  if (keyName === "type") {
    return [];
  }

  const typeName = lineMatch[4] ?? lineMatch[6] ?? lineMatch[8];
  if (!typeName) {
    return [];
  }

  const optionalSuffix = lineMatch[5] ?? lineMatch[7] ?? lineMatch[9] ?? "";
  const fullToken = `${typeName}${optionalSuffix}`;
  const start = code.lastIndexOf(fullToken);
  if (start < 0) {
    return [];
  }

  return [
    {
      name: typeName,
      start,
      isBuiltin: isBuiltinTypeName(typeName),
      optionalMarkerStart: optionalSuffix.length > 0 ? start + typeName.length : undefined
    }
  ];
}

function findTypeOccurrenceAtPosition(
  occurrences: TypeOccurrence[],
  position: vscode.Position
): TypeOccurrence | undefined {
  return occurrences.find((occurrence) => occurrence.range.contains(position));
}

function findImportNamespaceReferenceAtPosition(
  document: vscode.TextDocument,
  position: vscode.Position,
  importNamespaces: Map<string, ImportNamespace>
): vscode.Location | undefined {
  if (importNamespaces.size === 0) {
    return undefined;
  }

  const code = lineWithoutComment(document.lineAt(position.line).text);
  const namespaceRefRegex = /\b([A-Za-z_][\w]*)\.[A-Za-z_][\w.]*/g;
  for (
    let match = namespaceRefRegex.exec(code);
    match;
    match = namespaceRefRegex.exec(code)
  ) {
    const namespace = match[1];
    const namespaceStart = match.index;
    const namespaceEnd = namespaceStart + namespace.length;
    if (position.character < namespaceStart || position.character >= namespaceEnd) {
      continue;
    }

    const importNamespace = importNamespaces.get(namespace);
    if (!importNamespace) {
      continue;
    }

    return new vscode.Location(document.uri, importNamespace.aliasRange);
  }

  return undefined;
}

function findImportPathReferenceAtPosition(
  position: vscode.Position,
  importNamespaces: Map<string, ImportNamespace>
): vscode.Location | undefined {
  for (const importNamespace of importNamespaces.values()) {
    if (!importNamespace.importPathRange || !importNamespace.importUri) {
      continue;
    }
    if (!importNamespace.importPathRange.contains(position)) {
      continue;
    }

    return new vscode.Location(importNamespace.importUri, new vscode.Position(0, 0));
  }

  return undefined;
}

async function findImportedDefinitionForTypeOccurrence(
  sourceDocument: vscode.TextDocument,
  occurrence: TypeOccurrence,
  position: vscode.Position,
  importNamespaces: Map<string, ImportNamespace>
): Promise<vscode.Location | undefined> {
  const namespaced = parseNamespacedTypeReferenceAtPosition(occurrence, position);
  if (!namespaced) {
    return undefined;
  }

  const importNamespace = importNamespaces.get(namespaced.namespace);
  if (!importNamespace) {
    return undefined;
  }

  if (namespaced.segment === "namespace") {
    return new vscode.Location(sourceDocument.uri, importNamespace.aliasRange);
  }

  if (!importNamespace.importUri) {
    return undefined;
  }

  let importedDocument: vscode.TextDocument;
  try {
    importedDocument = await vscode.workspace.openTextDocument(importNamespace.importUri);
  } catch {
    return undefined;
  }

  if (!isSyamlDocument(importedDocument)) {
    return undefined;
  }

  const typeDefinition = findTypeDefinitionRangeByName(importedDocument, namespaced.typeName);
  if (!typeDefinition) {
    return undefined;
  }

  return new vscode.Location(importedDocument.uri, typeDefinition);
}

function parseNamespacedTypeReferenceAtPosition(
  occurrence: TypeOccurrence,
  position: vscode.Position
):
  | {
      namespace: string;
      typeName: string;
      segment: "namespace" | "type";
    }
  | undefined {
  const dotIndex = occurrence.name.indexOf(".");
  if (dotIndex <= 0 || dotIndex >= occurrence.name.length - 1) {
    return undefined;
  }

  const offset = position.character - occurrence.range.start.character;
  if (offset < 0 || offset > occurrence.name.length) {
    return undefined;
  }

  if (offset === dotIndex) {
    return undefined;
  }

  return {
    namespace: occurrence.name.slice(0, dotIndex),
    typeName: occurrence.name.slice(dotIndex + 1),
    segment: offset < dotIndex ? "namespace" : "type"
  };
}

async function findTypedDataKeySchemaDefinition(
  document: vscode.TextDocument,
  position: vscode.Position
): Promise<vscode.Location | undefined> {
  const locations = collectDataKeyLocations(document);
  const clicked = locations.find((location) =>
    new vscode.Range(location.line, location.start, location.line, location.end).contains(
      position
    )
  );
  if (!clicked) {
    return undefined;
  }

  const explicitHints = new Map<string, string>();
  for (const location of locations) {
    if (!location.explicitTypeHint) {
      continue;
    }
    explicitHints.set(location.path, location.explicitTypeHint);
  }
  if (explicitHints.size === 0) {
    return undefined;
  }

  const schemaIndex = await buildSchemaTypeIndexWithImports(document);
  if (schemaIndex.roots.size === 0) {
    return undefined;
  }

  const typedAncestors = collectTypedAncestors(clicked.path, explicitHints);
  for (const ancestor of typedAncestors) {
    const relativeSegments = relativeJsonPathSegments(ancestor.path, clicked.path);
    if (!relativeSegments) {
      continue;
    }

    if (relativeSegments.length === 0) {
      const typeDefinition = findTypeDefinitionLocation(
        document.uri,
        schemaIndex,
        ancestor.typeName
      );
      if (typeDefinition) {
        return typeDefinition;
      }
      continue;
    }

    const propertyDefinition = findPropertyDefinitionLocationFromAncestor(
      document.uri,
      schemaIndex,
      ancestor.typeName,
      relativeSegments
    );
    if (propertyDefinition) {
      return propertyDefinition;
    }
  }

  return undefined;
}

function collectTypedAncestors(
  path: string,
  hintsByPath: Map<string, string>
): Array<{ path: string; typeName: string }> {
  const ancestors: Array<{ path: string; typeName: string }> = [];
  let current: string | undefined = path;
  while (current) {
    const typeName = hintsByPath.get(current);
    if (typeName) {
      ancestors.push({ path: current, typeName });
    }
    current = parentJsonPath(current);
  }
  return ancestors;
}

function findTypeDefinitionRangeByName(
  document: vscode.TextDocument,
  typeName: string
): vscode.Range | undefined {
  const definitions = collectTypeDefinitions(document);
  return definitions.find((definition) => definition.name === typeName)?.range;
}

function findTypeDefinitionLocation(
  fallbackUri: vscode.Uri,
  schemaIndex: SchemaTypeIndex,
  typeName: string
): vscode.Location | undefined {
  const definition = schemaIndex.roots.get(typeName);
  if (!definition?.definitionRange) {
    return undefined;
  }
  return new vscode.Location(definition.definitionUri ?? fallbackUri, definition.definitionRange);
}

function findPropertyDefinitionLocationFromAncestor(
  fallbackUri: vscode.Uri,
  schemaIndex: SchemaTypeIndex,
  ancestorType: string,
  segments: JsonPathSegment[]
): vscode.Location | undefined {
  const root = createSchemaNode();
  root.typeName = ancestorType;

  let current = root;
  for (const segment of segments) {
    const next = descendSchemaForInference(schemaIndex, current, segment, new Set<string>());
    if (!next) {
      return undefined;
    }
    current = next;
  }

  if (!current.definitionRange) {
    return undefined;
  }
  return new vscode.Location(current.definitionUri ?? fallbackUri, current.definitionRange);
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

async function previewImportAsSyaml(
  document: vscode.TextDocument,
  extensionPath: string,
  previewProvider: SyamlPreviewContentProvider
): Promise<void> {
  const sourceFormat = detectImportSourceFormat(document);
  if (!sourceFormat) {
    const supported = (
      Object.values(IMPORT_SOURCE_META) as (typeof IMPORT_SOURCE_META)[ImportSourceFormat][]
    )
      .map((m) => `${m.label} (${m.extensions.join(", ")})`)
      .join("; ");
    void vscode.window.showInformationMessage(
      `SYAML import preview: unsupported file type. Supported: ${supported}.`
    );
    return;
  }

  let parser: ParserCommand;
  try {
    parser = await resolveParserCommand(document, extensionPath);
  } catch {
    void vscode.window.showErrorMessage(
      "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
    );
    return;
  }

  const { cliCommand, label } = IMPORT_SOURCE_META[sourceFormat];

  // Import commands always read from disk; the file must be saved.
  if (document.uri.scheme !== "file") {
    void vscode.window.showErrorMessage(
      `SYAML import: file must be saved to disk before previewing.`
    );
    return;
  }

  let content: string;
  try {
    const output = await execFileAsync(
      parser.command,
      [...parser.argPrefix, cliCommand, document.uri.fsPath],
      { cwd: parser.cwd, timeout: 15000, maxBuffer: 4 * 1024 * 1024 }
    );
    content = output.stdout.toString();
  } catch (error) {
    const execError = error as ExecError;
    if (execError.code === "ENOENT") {
      void vscode.window.showErrorMessage(
        "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
      );
      return;
    }
    const output = normalizeExecOutput(execError);
    void vscode.window.showErrorMessage(
      `SYAML import (${label}) failed: ${extractDiagnosticMessage(output)}`
    );
    return;
  }

  if (!content.endsWith("\n")) {
    content += "\n";
  }

  const key = encodeURIComponent(document.uri.toString());
  const previewUri = vscode.Uri.parse(`syaml-preview:/import/${key}.syaml`);
  previewProvider.setContent(previewUri, content);

  const previewDocument = await vscode.workspace.openTextDocument(previewUri);
  await vscode.window.showTextDocument(previewDocument, {
    preview: true,
    viewColumn: vscode.ViewColumn.Beside
  });
}

async function saveImportAsSyaml(
  document: vscode.TextDocument,
  extensionPath: string
): Promise<void> {
  const sourceFormat = detectImportSourceFormat(document);
  if (!sourceFormat) {
    const supported = (
      Object.values(IMPORT_SOURCE_META) as (typeof IMPORT_SOURCE_META)[ImportSourceFormat][]
    )
      .map((m) => `${m.label} (${m.extensions.join(", ")})`)
      .join("; ");
    void vscode.window.showInformationMessage(
      `SYAML import: unsupported file type. Supported: ${supported}.`
    );
    return;
  }

  if (document.uri.scheme !== "file") {
    void vscode.window.showErrorMessage(
      "SYAML import: file must be saved to disk before converting."
    );
    return;
  }

  let parser: ParserCommand;
  try {
    parser = await resolveParserCommand(document, extensionPath);
  } catch {
    void vscode.window.showErrorMessage(
      "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
    );
    return;
  }

  const { cliCommand, label } = IMPORT_SOURCE_META[sourceFormat];

  // Default save path: same directory and stem, .syaml extension.
  const sourceFsPath = document.uri.fsPath;
  const defaultSavePath = path.join(
    path.dirname(sourceFsPath),
    `${path.basename(sourceFsPath, path.extname(sourceFsPath))}.syaml`
  );

  const saveUri = await vscode.window.showSaveDialog({
    defaultUri: vscode.Uri.file(defaultSavePath),
    filters: { SYAML: ["syaml"] },
    title: `Save ${label} as SYAML`
  });
  if (!saveUri) {
    return; // User cancelled.
  }

  try {
    await execFileAsync(
      parser.command,
      [...parser.argPrefix, cliCommand, sourceFsPath, "--output", saveUri.fsPath],
      { cwd: parser.cwd, timeout: 15000, maxBuffer: 4 * 1024 * 1024 }
    );
  } catch (error) {
    const execError = error as ExecError;
    if (execError.code === "ENOENT") {
      void vscode.window.showErrorMessage(
        "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
      );
      return;
    }
    const output = normalizeExecOutput(execError);
    void vscode.window.showErrorMessage(
      `SYAML import (${label}) failed: ${extractDiagnosticMessage(output)}`
    );
    return;
  }

  const savedDoc = await vscode.workspace.openTextDocument(saveUri);
  await vscode.window.showTextDocument(savedDoc);
  void vscode.window.showInformationMessage(
    `SYAML: Saved ${label} import to ${path.basename(saveUri.fsPath)}`
  );
}

async function previewExpandedOutput(
  document: vscode.TextDocument,
  extensionPath: string,
  previewProvider: SyamlPreviewContentProvider,
  format: PreviewFormat = "yaml"
): Promise<void> {
  let parser: ParserCommand;
  try {
    parser = await resolveParserCommand(document, extensionPath);
  } catch {
    void vscode.window.showErrorMessage(
      "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
    );
    return;
  }

  const meta = PREVIEW_FORMAT_META[format];
  const result = await withInputFile(document, async (inputPath) => {
    const args = [...parser.argPrefix, "compile", inputPath, "--format", meta.cliFlag];
    try {
      const output = await execFileAsync(parser.command, args, {
        cwd: parser.cwd,
        timeout: 15000,
        maxBuffer: 4 * 1024 * 1024
      });
      return { ok: true as const, content: output.stdout.toString() };
    } catch (error) {
      const execError = error as ExecError;
      if (execError.code === "ENOENT") {
        return {
          ok: false as const,
          message:
            "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
        };
      }
      const output = normalizeExecOutput(execError);
      return { ok: false as const, message: extractDiagnosticMessage(output) };
    }
  });

  if (!result.ok) {
    void vscode.window.showErrorMessage(`SYAML preview failed: ${result.message}`);
    return;
  }

  const key = encodeURIComponent(document.uri.toString());
  const previewUri = vscode.Uri.parse(`syaml-preview:/${key}${meta.ext}`);
  const content = result.content.endsWith("\n")
    ? result.content
    : `${result.content}\n`;
  previewProvider.setContent(previewUri, content);

  const previewDocument = await vscode.workspace.openTextDocument(previewUri);
  await vscode.window.showTextDocument(previewDocument, {
    preview: true,
    viewColumn: vscode.ViewColumn.Beside
  });
}

async function saveExpandedOutput(
  document: vscode.TextDocument,
  extensionPath: string,
  format: PreviewFormat = "yaml"
): Promise<void> {
  let parser: ParserCommand;
  try {
    parser = await resolveParserCommand(document, extensionPath);
  } catch {
    void vscode.window.showErrorMessage(
      "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
    );
    return;
  }

  const meta = PREVIEW_FORMAT_META[format];

  // Compile first (handles unsaved/dirty documents via temp file).
  const result = await withInputFile(document, async (inputPath) => {
    const args = [...parser.argPrefix, "compile", inputPath, "--format", meta.cliFlag];
    try {
      const output = await execFileAsync(parser.command, args, {
        cwd: parser.cwd,
        timeout: 15000,
        maxBuffer: 4 * 1024 * 1024
      });
      return { ok: true as const, content: output.stdout.toString() };
    } catch (error) {
      const execError = error as ExecError;
      if (execError.code === "ENOENT") {
        return {
          ok: false as const,
          message: "Cannot run SYAML parser. Set syaml.parser.path or install super-yaml."
        };
      }
      const output = normalizeExecOutput(execError);
      return { ok: false as const, message: extractDiagnosticMessage(output) };
    }
  });

  if (!result.ok) {
    void vscode.window.showErrorMessage(`SYAML compile failed: ${result.message}`);
    return;
  }

  // Default save path: same directory and stem as the source .syaml file.
  const defaultSavePath =
    document.uri.scheme === "file"
      ? path.join(
          path.dirname(document.uri.fsPath),
          `${path.basename(document.uri.fsPath, ".syaml")}${meta.ext}`
        )
      : undefined;

  const saveUri = await vscode.window.showSaveDialog({
    defaultUri: defaultSavePath ? vscode.Uri.file(defaultSavePath) : undefined,
    filters: { [format.toUpperCase()]: [meta.ext.slice(1)] },
    title: `Save SYAML output as ${format.toUpperCase()}`
  });
  if (!saveUri) {
    return; // User cancelled.
  }

  const content = result.content.endsWith("\n") ? result.content : `${result.content}\n`;
  await fs.writeFile(saveUri.fsPath, content, "utf8");

  const savedDoc = await vscode.workspace.openTextDocument(saveUri);
  await vscode.window.showTextDocument(savedDoc);
  void vscode.window.showInformationMessage(
    `SYAML: Saved to ${path.basename(saveUri.fsPath)}`
  );
}

async function resolveParserCommand(
  document: vscode.TextDocument,
  extensionPath: string
): Promise<ParserCommand> {
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
    const binaryName = parserBinaryNameForPlatform(process.platform);
    const localBinaryCandidates = [
      path.join(workspace.uri.fsPath, "target", "debug", binaryName),
      path.join(workspace.uri.fsPath, "target", "release", binaryName),
      path.join(workspace.uri.fsPath, "rust", "target", "debug", binaryName),
      path.join(workspace.uri.fsPath, "rust", "target", "release", binaryName)
    ];
    for (const localBinary of localBinaryCandidates) {
      if (await fileExists(localBinary)) {
        return { cwd: workspace.uri.fsPath, command: localBinary, argPrefix: [] };
      }
    }
  }

  const bundledBinary = await resolveBundledBinary(extensionPath);
  if (bundledBinary) {
    return { cwd, command: bundledBinary, argPrefix: [] };
  }

  // Prefer a standalone parser from PATH before falling back to cargo.
  const pathBinary = await findInPath("super-yaml");
  if (pathBinary) {
    return {
      cwd,
      command: pathBinary,
      argPrefix: []
    };
  }

  throw new Error("SYAML parser binary not found");
}

async function withInputFile<T>(
  document: vscode.TextDocument,
  run: (inputPath: string) => Promise<T>
): Promise<T> {
  if (document.uri.scheme === "file" && !document.isDirty) {
    return run(document.uri.fsPath);
  }

  const tempName = `syaml-vscode-${Date.now()}-${Math.random()
    .toString(16)
    .slice(2)}.syaml`;
  let tempPath = path.join(os.tmpdir(), tempName);

  // Keep dirty file validation in the source directory so relative imports
  // (for example `./common.syaml`) resolve the same way as on-disk files.
  if (document.uri.scheme === "file") {
    const sourceDir = path.dirname(document.uri.fsPath);
    tempPath = path.join(sourceDir, `.${tempName}`);
  }

  const text = document.getText();
  try {
    await fs.writeFile(tempPath, text, "utf8");
  } catch {
    if (document.uri.scheme === "file" && !tempPath.startsWith(os.tmpdir())) {
      tempPath = path.join(os.tmpdir(), tempName);
      await fs.writeFile(tempPath, text, "utf8");
    } else {
      throw new Error("failed to create temporary SYAML input file");
    }
  }

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

function parserBinaryNameForPlatform(platform: NodeJS.Platform): string {
  return platform === "win32" ? "super-yaml.exe" : "super-yaml";
}

async function resolveBundledBinary(extensionPath: string): Promise<string | null> {
  const binaryName = parserBinaryNameForPlatform(process.platform);
  const candidate = path.join(
    extensionPath,
    "bin",
    `${process.platform}-${process.arch}`,
    binaryName
  );
  if (!(await fileExists(candidate))) {
    return null;
  }

  // VSIX extraction should preserve executable bits, but enforce it to avoid host differences.
  if (process.platform !== "win32") {
    try {
      await fs.chmod(candidate, 0o755);
    } catch {
      // Ignore chmod failures and attempt execution as-is.
    }
  }

  return candidate;
}

async function findInPath(commandName: string): Promise<string | null> {
  const pathVar = process.env.PATH;
  if (!pathVar) {
    return null;
  }

  const candidateNames =
    process.platform === "win32" ? [commandName, `${commandName}.exe`] : [commandName];
  const segments = pathVar.split(path.delimiter).filter((segment) => segment.length > 0);
  for (const segment of segments) {
    for (const candidateName of candidateNames) {
      const candidate = path.join(segment, candidateName);
      if (await fileExists(candidate)) {
        return candidate;
      }
    }
  }

  return null;
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
  const pathRange = inferPathRange(document, message);
  if (pathRange) {
    return pathRange;
  }

  const absoluteLineNumber = inferAbsoluteLineNumber(document, message);
  if (absoluteLineNumber) {
    const lineIndex = Math.max(0, Math.min(document.lineCount - 1, absoluteLineNumber - 1));
    const length = Math.max(1, document.lineAt(lineIndex).text.length);
    return new vscode.Range(lineIndex, 0, lineIndex, length);
  }

  const unknownTypeRange = inferUnknownTypeRange(document, message);
  if (unknownTypeRange) {
    return unknownTypeRange;
  }

  return new vscode.Range(0, 0, 0, Math.max(1, document.lineAt(0).text.length));
}

interface DataKeyLocation {
  path: string;
  normalizedPath: string;
  line: number;
  start: number;
  end: number;
  explicitTypeHint?: string;
}

interface SchemaKeyLocation {
  path: string;
  normalizedPath: string;
  line: number;
  start: number;
  end: number;
}

interface ParseStackEntry {
  indent: number;
  path: string;
}

interface JsonPathSegment {
  kind: "key" | "index";
  value: string | number;
}

interface SchemaNodeLite {
  typeName?: string;
  definitionRange?: vscode.Range;
  definitionUri?: vscode.Uri;
  properties: Map<string, SchemaNodeLite>;
  hasPropertiesKeyword: boolean;
  items?: SchemaNodeLite;
  hasItemsKeyword: boolean;
}

interface SchemaTypeIndex {
  roots: Map<string, SchemaNodeLite>;
}

function inferPathRange(
  document: vscode.TextDocument,
  message: string
): vscode.Range | undefined {
  const path = extractDiagnosticPathFromMessage(message);
  if (!path) {
    return undefined;
  }

  const locationMatch = findPathLocationRange(document, path);
  if (locationMatch) {
    return locationMatch;
  }

  if (path.startsWith("schema.")) {
    return undefined;
  }

  const segment = lastPathSegment(path);
  if (!segment) {
    return undefined;
  }

  const fallbackLocations = collectPathLocationsForPrefix(document, path);
  if (fallbackLocations.length === 0) {
    return undefined;
  }

  const lineRegex = new RegExp(`^\\s*${escapeRegex(segment)}(?:\\s*<[^>]+>)?\\s*:`);
  for (const loc of fallbackLocations) {
    const code = lineWithoutComment(document.lineAt(loc.line).text);
    if (lineRegex.test(code)) {
      return new vscode.Range(loc.line, loc.start, loc.line, loc.end);
    }
  }
  return undefined;
}

function extractDiagnosticPathFromMessage(message: string): string | undefined {
  const patterns = [
    /\bat\s+'?(schema\.[A-Za-z0-9_.\[\]]+)'?/i,
    /\bpath\s+'(schema\.[A-Za-z0-9_.\[\]]+)'/i,
    /normalized\s+'(\$[A-Za-z0-9_.\[\]]+)'/i,
    /\bat\s+'?(\$[A-Za-z0-9_.\[\]]+)'?/i,
    /\bpath\s+'(\$[A-Za-z0-9_.\[\]]+)'/i
  ];
  for (const pattern of patterns) {
    const match = pattern.exec(message);
    if (match) {
      return match[1];
    }
  }
  return undefined;
}

function normalizeJsonPath(path: string): string {
  return path.replace(/\[\d+\]/g, "");
}

function findPathLocationRange(
  document: vscode.TextDocument,
  path: string
): vscode.Range | undefined {
  const locations = collectPathLocationsForPrefix(document, path);
  if (locations.length === 0) {
    return undefined;
  }

  const normalizedTarget = normalizeJsonPath(path);
  const exact = locations.find((loc) => loc.path === path);
  if (exact) {
    return new vscode.Range(exact.line, exact.start, exact.line, exact.end);
  }

  const normalized = locations.find((loc) => loc.normalizedPath === normalizedTarget);
  if (normalized) {
    return new vscode.Range(
      normalized.line,
      normalized.start,
      normalized.line,
      normalized.end
    );
  }

  if (path.startsWith("schema.") && path.endsWith(".type")) {
    const shorthandParentPath = path.slice(0, -".type".length);
    const parent = locations.find((loc) => loc.path === shorthandParentPath);
    if (parent) {
      return new vscode.Range(parent.line, parent.start, parent.line, parent.end);
    }
  }

  return undefined;
}

function collectPathLocationsForPrefix(
  document: vscode.TextDocument,
  path: string
): Array<DataKeyLocation | SchemaKeyLocation> {
  if (path.startsWith("schema.")) {
    return collectSchemaKeyLocations(document);
  }
  if (path.startsWith("$")) {
    return collectDataKeyLocations(document);
  }
  return [];
}

function lastPathSegment(path: string): string | undefined {
  const withoutIndices = normalizeJsonPath(path);
  const parts = withoutIndices.split(".").filter((part) => part.length > 0);
  if (parts.length === 0) {
    return undefined;
  }
  const last = parts[parts.length - 1];
  return last === "$" ? undefined : last;
}

function collectDataKeyLocations(document: vscode.TextDocument): DataKeyLocation[] {
  const bounds = findSectionBounds(document, "data");
  if (!bounds) {
    return [];
  }

  const locations: DataKeyLocation[] = [];
  const stack: ParseStackEntry[] = [];
  const arrayCounters = new Map<string, number>();

  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const rawLine = document.lineAt(line).text;
    const code = lineWithoutComment(rawLine);
    if (code.trim().length === 0) {
      continue;
    }

    const dashMatch = /^(\s*)-(\s*)(.*)$/.exec(code);
    if (dashMatch) {
      const dashIndent = dashMatch[1].length;
      while (stack.length > 0 && stack[stack.length - 1].indent >= dashIndent) {
        stack.pop();
      }

      const parentPath = stack.length > 0 ? stack[stack.length - 1].path : "$";
      const counterKey = `${parentPath}|${dashIndent}`;
      const itemIndex = arrayCounters.get(counterKey) ?? 0;
      arrayCounters.set(counterKey, itemIndex + 1);
      const itemPath = `${parentPath}[${itemIndex}]`;
      stack.push({ indent: dashIndent, path: itemPath });

      const rest = dashMatch[3];
      const inlineKeyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(rest);
      if (!inlineKeyMatch) {
        continue;
      }

      const keyInfo = parseCanonicalKey(inlineKeyMatch[2]);
      if (!keyInfo) {
        continue;
      }
      const keyStart =
        dashIndent +
        1 +
        dashMatch[2].length +
        inlineKeyMatch[1].length +
        keyInfo.keyOffset;
      const childPath = `${itemPath}.${keyInfo.key}`;
      const explicitTypeHint = parseTypeHint(inlineKeyMatch[2])?.name;
      locations.push({
        path: childPath,
        normalizedPath: normalizeJsonPath(childPath),
        line,
        start: keyStart,
        end: keyStart + keyInfo.key.length,
        explicitTypeHint
      });
      stack.push({
        indent: dashIndent + 1 + dashMatch[2].length + inlineKeyMatch[1].length,
        path: childPath
      });
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    while (stack.length > 0 && stack[stack.length - 1].indent >= indent) {
      stack.pop();
    }

    const keyInfo = parseCanonicalKey(keyMatch[2]);
    if (!keyInfo) {
      continue;
    }

    const parentPath = stack.length > 0 ? stack[stack.length - 1].path : "$";
    const path = `${parentPath}.${keyInfo.key}`;
    const start = indent + keyInfo.keyOffset;
    const explicitTypeHint = parseTypeHint(keyMatch[2])?.name;
    locations.push({
      path,
      normalizedPath: normalizeJsonPath(path),
      line,
      start,
      end: start + keyInfo.key.length,
      explicitTypeHint
    });
    stack.push({ indent, path });
  }

  return locations;
}

function collectSchemaKeyLocations(document: vscode.TextDocument): SchemaKeyLocation[] {
  const bounds = findSectionBounds(document, "schema");
  if (!bounds) {
    return [];
  }

  const locations: SchemaKeyLocation[] = [];
  const stack: ParseStackEntry[] = [];

  for (let line = bounds.startLine; line < bounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    while (stack.length > 0 && stack[stack.length - 1].indent >= indent) {
      stack.pop();
    }

    const keyInfo = parseCanonicalKey(keyMatch[2]);
    if (!keyInfo) {
      continue;
    }

    stack.push({ indent, path: keyInfo.key });
    const pathSegments = stack.map((entry) => entry.path);
    const withoutWrapper =
      pathSegments[0] === "types" ? pathSegments.slice(1) : pathSegments;
    if (withoutWrapper.length === 0) {
      continue;
    }

    const path = `schema.${withoutWrapper.join(".")}`;
    const start = indent + keyInfo.keyOffset;
    locations.push({
      path,
      normalizedPath: normalizeJsonPath(path),
      line,
      start,
      end: start + keyInfo.key.length
    });
  }

  return locations;
}

function parseCanonicalKey(
  rawKey: string
): { key: string; keyOffset: number } | undefined {
  const trimmed = rawKey.trim();
  if (trimmed.length === 0) {
    return undefined;
  }

  const typeMatch = /^(.*?)(\s*<\s*([A-Za-z_][\w.]*)\s*>)\s*$/.exec(trimmed);
  const candidate = typeMatch ? typeMatch[1].trimEnd() : trimmed;
  if (candidate.length === 0) {
    return undefined;
  }

  const leadingWhitespace = rawKey.length - rawKey.trimStart().length;
  return {
    key: candidate,
    keyOffset: leadingWhitespace
  };
}

function collectImportNamespaces(document: vscode.TextDocument): Map<string, ImportNamespace> {
  const metaBounds = findSectionBounds(document, "meta");
  if (!metaBounds) {
    return new Map();
  }

  let importsLine: number | undefined;
  let importsIndent = 0;
  for (let line = metaBounds.startLine; line < metaBounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }
    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):/.exec(code);
    if (!keyMatch) {
      continue;
    }
    if (keyMatch[2].trim() !== "imports") {
      continue;
    }

    importsLine = line;
    importsIndent = keyMatch[1].length;
    break;
  }

  if (importsLine === undefined) {
    return new Map();
  }

  const namespaces = new Map<string, ImportNamespace>();
  let childIndent: number | undefined;
  for (let line = importsLine + 1; line < metaBounds.endLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):(.*)$/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    if (indent <= importsIndent) {
      break;
    }
    if (childIndent === undefined) {
      childIndent = indent;
    }
    if (indent !== childIndent) {
      continue;
    }

    const aliasRaw = keyMatch[2];
    const alias = aliasRaw.trim();
    if (alias.length === 0) {
      continue;
    }

    const aliasStart = indent + aliasRaw.indexOf(alias);
    const aliasRange = new vscode.Range(
      line,
      aliasStart,
      line,
      aliasStart + alias.length
    );

    const parsedImportPath = parseImportPathValue(
      document,
      line,
      indent,
      keyMatch[4],
      keyMatch[0].length - keyMatch[4].length,
      metaBounds.endLine
    );

    namespaces.set(alias, {
      alias,
      aliasRange,
      importPathRange: parsedImportPath?.range,
      importUri: resolveImportPathUri(document, parsedImportPath?.value)
    });
  }

  return namespaces;
}

function parseImportPathValue(
  document: vscode.TextDocument,
  aliasLine: number,
  aliasIndent: number,
  inlineValue: string,
  inlineValueStartCharacter: number,
  metaSectionEndLine: number
):
  | {
      value: string;
      range: vscode.Range;
    }
  | undefined {
  const inlinePath = parseStringScalarAt(
    inlineValue,
    aliasLine,
    inlineValueStartCharacter
  );
  if (inlinePath) {
    return inlinePath;
  }

  for (let line = aliasLine + 1; line < metaSectionEndLine; line += 1) {
    const code = lineWithoutComment(document.lineAt(line).text);
    if (code.trim().length === 0) {
      continue;
    }

    const keyMatch = /^(\s*)([^:#][^:]*?)(\s*):(.*)$/.exec(code);
    if (!keyMatch) {
      continue;
    }

    const indent = keyMatch[1].length;
    if (indent <= aliasIndent) {
      break;
    }

    const key = keyMatch[2].trim();
    if (key !== "path") {
      continue;
    }

    const valueStart = keyMatch[0].length - keyMatch[4].length;
    return parseStringScalarAt(keyMatch[4], line, valueStart);
  }

  return undefined;
}

function parseStringScalarAt(
  value: string,
  line: number,
  startCharacter: number
): { value: string; range: vscode.Range } | undefined {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return undefined;
  }

  const tokenOffset = value.indexOf(trimmed);
  if (tokenOffset < 0) {
    return undefined;
  }

  const singleQuoted = /^'([^']*)'$/.exec(trimmed);
  if (singleQuoted) {
    const contentStart = startCharacter + tokenOffset + 1;
    return {
      value: singleQuoted[1],
      range: new vscode.Range(
        line,
        contentStart,
        line,
        contentStart + singleQuoted[1].length
      )
    };
  }

  const doubleQuoted = /^"((?:\\.|[^"])*)"$/.exec(trimmed);
  if (doubleQuoted) {
    const unescaped = doubleQuoted[1].replace(/\\"/g, "\"").replace(/\\\\/g, "\\");
    const contentStart = startCharacter + tokenOffset + 1;
    return {
      value: unescaped,
      range: new vscode.Range(line, contentStart, line, contentStart + doubleQuoted[1].length)
    };
  }

  if (/^[^{}\[\],]+$/.test(trimmed)) {
    const valueStart = startCharacter + tokenOffset;
    return {
      value: trimmed,
      range: new vscode.Range(line, valueStart, line, valueStart + trimmed.length)
    };
  }

  return undefined;
}

function resolveImportPathUri(
  document: vscode.TextDocument,
  importPath: string | undefined
): vscode.Uri | undefined {
  if (!importPath || document.uri.scheme !== "file") {
    return undefined;
  }

  const resolvedPath = path.isAbsolute(importPath)
    ? importPath
    : path.resolve(path.dirname(document.uri.fsPath), importPath);
  return vscode.Uri.file(resolvedPath);
}

function findSectionBounds(
  document: vscode.TextDocument,
  sectionName: "meta" | "schema" | "data"
): { startLine: number; endLine: number } | undefined {
  const marker = `---${sectionName}`;
  for (let i = 0; i < document.lineCount; i += 1) {
    if (document.lineAt(i).text.trim() !== marker) {
      continue;
    }

    let endLine = document.lineCount;
    for (let j = i + 1; j < document.lineCount; j += 1) {
      const lineText = document.lineAt(j).text.trim();
      if (/^---(meta|schema|data)\s*$/.test(lineText)) {
        endLine = j;
        break;
      }
    }
    return { startLine: i + 1, endLine };
  }
  return undefined;
}

function lineWithoutComment(text: string): string {
  const commentStart = findCommentStart(text);
  if (commentStart >= 0) {
    return text.slice(0, commentStart);
  }
  return text;
}

function isBuiltinTypeName(typeName: string): boolean {
  return (
    typeName === "string" ||
    typeName === "integer" ||
    typeName === "number" ||
    typeName === "boolean" ||
    typeName === "object" ||
    typeName === "array" ||
    typeName === "null"
  );
}

function escapeRegex(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
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

function inferUnknownTypeRange(
  document: vscode.TextDocument,
  message: string
): vscode.Range | undefined {
  const unknownType = extractUnknownTypeName(message);
  if (!unknownType) {
    return undefined;
  }

  if (/\bunknown type(?: reference)? at schema\./i.test(message)) {
    for (const occurrence of collectSchemaTypeReferences(document)) {
      if (occurrence.name === unknownType) {
        return occurrence.range;
      }
    }
  }

  let firstDefinition: TypeOccurrence | undefined;
  for (const occurrence of collectTypeOccurrences(document)) {
    if (occurrence.name !== unknownType) {
      continue;
    }
    if (occurrence.kind === "reference") {
      return occurrence.range;
    }
    if (!firstDefinition) {
      firstDefinition = occurrence;
    }
  }

  return firstDefinition?.range;
}

function extractUnknownTypeName(message: string): string | undefined {
  const match = /\bunknown type(?: reference)?(?: at [^:]+:)?\s+['"]([A-Za-z_][\w.]*)['"]/i.exec(
    message
  );
  return match?.[1];
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
