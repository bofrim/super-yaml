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
  const validator = new SyamlValidator(diagnostics, context.extensionPath);
  const semanticProvider = new SyamlSemanticTokensProvider();
  const typeDefinitionProvider = new SyamlTypeDefinitionProvider();
  const typeReferenceProvider = new SyamlTypeReferenceProvider();
  const inlayHintsProvider = new SyamlInlayHintsProvider();

  context.subscriptions.push(diagnostics);
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

interface TypeOccurrence {
  name: string;
  range: vscode.Range;
  kind: "definition" | "reference";
}

class SyamlTypeDefinitionProvider implements vscode.DefinitionProvider {
  provideDefinition(
    document: vscode.TextDocument,
    position: vscode.Position
  ): vscode.ProviderResult<vscode.Definition> {
    if (!isSyamlDocument(document)) {
      return undefined;
    }

    const occurrences = collectTypeOccurrences(document);
    const target = findTypeOccurrenceAtPosition(occurrences, position);
    if (!target) {
      return findTypedDataKeySchemaDefinition(document, position);
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
  provideInlayHints(
    document: vscode.TextDocument,
    range: vscode.Range
  ): vscode.ProviderResult<vscode.InlayHint[]> {
    if (!isSyamlDocument(document)) {
      return [];
    }

    const schemaIndex = buildSchemaTypeIndex(document);
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
        inferredType,
        vscode.InlayHintKind.Type
      );
      hint.paddingLeft = true;
      hint.tooltip = `Inferred from ${ancestor.typeName}`;
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

function collectTypeOccurrences(document: vscode.TextDocument): TypeOccurrence[] {
  return [
    ...collectTypeDefinitions(document),
    ...collectTypeHintReferences(document)
  ];
}

function collectTypeDefinitions(document: vscode.TextDocument): TypeOccurrence[] {
  const bounds = findSectionBounds(document, "schema");
  if (!bounds) {
    return [];
  }

  const occurrences: TypeOccurrence[] = [];
  let inTypesSection = false;
  let typesIndent = -1;
  let typeEntryIndent: number | undefined;

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

    if (!inTypesSection) {
      if (keyName === "types") {
        inTypesSection = true;
        typesIndent = indent;
        typeEntryIndent = undefined;
      }
      continue;
    }

    if (indent <= typesIndent) {
      if (keyName === "types") {
        inTypesSection = true;
        typesIndent = indent;
        typeEntryIndent = undefined;
      } else {
        inTypesSection = false;
      }
      continue;
    }

    if (typeEntryIndent === undefined) {
      typeEntryIndent = indent;
    }
    if (indent !== typeEntryIndent) {
      continue;
    }

    const type = parseTypeDefinitionName(rawKey);
    if (!type) {
      continue;
    }

    const start = indent + type.offset;
    occurrences.push({
      name: type.name,
      range: new vscode.Range(line, start, line, start + type.name.length),
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
    if (fullPath[0] !== "types" || fullPath.length < 2) {
      continue;
    }

    const typeName = fullPath[1];
    const relPath = fullPath.slice(2);
    const typeRoot = ensureTypeRoot(roots, typeName);

    if (relPath.length >= 2 && relPath[relPath.length - 2] === "properties") {
      const propertyNode = ensureSchemaNodeByRelPath(typeRoot, relPath);
      propertyNode.definitionRange = new vscode.Range(
        line,
        keyStart,
        line,
        keyStart + keyName.length
      );
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

  const quoted = /^(["'])([A-Za-z_][\w.]*)\1$/.exec(valueText);
  if (quoted) {
    return quoted[2];
  }
  if (/^[A-Za-z_][\w.]*$/.test(valueText)) {
    return valueText;
  }

  const inlineType =
    /(?:^|[{,]\s*)type\s*:\s*(?:"([A-Za-z_][\w.]*)"|'([A-Za-z_][\w.]*)'|([A-Za-z_][\w.]*))(?:\s*[,}].*|\s*$)/.exec(
      valueText
    );
  if (!inlineType) {
    return undefined;
  }
  return inlineType[1] ?? inlineType[2] ?? inlineType[3];
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

function findTypeOccurrenceAtPosition(
  occurrences: TypeOccurrence[],
  position: vscode.Position
): TypeOccurrence | undefined {
  return occurrences.find((occurrence) => occurrence.range.contains(position));
}

function findTypedDataKeySchemaDefinition(
  document: vscode.TextDocument,
  position: vscode.Position
): vscode.Location | undefined {
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

  const schemaIndex = buildSchemaTypeIndex(document);
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
      const typeDefinition = findTypeDefinitionRangeByName(document, ancestor.typeName);
      if (typeDefinition) {
        return new vscode.Location(document.uri, typeDefinition);
      }
      continue;
    }

    const propertyDefinition = findPropertyDefinitionRangeFromAncestor(
      schemaIndex,
      ancestor.typeName,
      relativeSegments
    );
    if (propertyDefinition) {
      return new vscode.Location(document.uri, propertyDefinition);
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

function findPropertyDefinitionRangeFromAncestor(
  schemaIndex: SchemaTypeIndex,
  ancestorType: string,
  segments: JsonPathSegment[]
): vscode.Range | undefined {
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

  return current.definitionRange;
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

  const bundledBinary = await resolveBundledBinary(extensionPath);
  if (bundledBinary) {
    return { cwd, command: bundledBinary, argPrefix: [] };
  }

  if (workspace) {
    const binaryName = parserBinaryNameForPlatform(process.platform);
    const localBinaryCandidates = [
      path.join(workspace.uri.fsPath, "target", "debug", binaryName),
      path.join(workspace.uri.fsPath, "rust", "target", "debug", binaryName)
    ];
    for (const localBinary of localBinaryCandidates) {
      if (await fileExists(localBinary)) {
        return { cwd: workspace.uri.fsPath, command: localBinary, argPrefix: [] };
      }
    }
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
  if (!absoluteLineNumber) {
    return new vscode.Range(0, 0, 0, Math.max(1, document.lineAt(0).text.length));
  }

  const lineIndex = Math.max(0, Math.min(document.lineCount - 1, absoluteLineNumber - 1));
  const length = Math.max(1, document.lineAt(lineIndex).text.length);
  return new vscode.Range(lineIndex, 0, lineIndex, length);
}

interface DataKeyLocation {
  path: string;
  normalizedPath: string;
  line: number;
  start: number;
  end: number;
  explicitTypeHint?: string;
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
  const path = extractJsonPathFromMessage(message);
  if (!path) {
    return undefined;
  }

  const normalizedTarget = normalizeJsonPath(path);
  const locations = collectDataKeyLocations(document);
  if (locations.length === 0) {
    return undefined;
  }

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

  const segment = lastPathSegment(path);
  if (!segment) {
    return undefined;
  }

  const lineRegex = new RegExp(`^\\s*${escapeRegex(segment)}(?:\\s*<[^>]+>)?\\s*:`);
  for (const loc of locations) {
    const code = lineWithoutComment(document.lineAt(loc.line).text);
    if (lineRegex.test(code)) {
      return new vscode.Range(loc.line, loc.start, loc.line, loc.end);
    }
  }
  return undefined;
}

function extractJsonPathFromMessage(message: string): string | undefined {
  const patterns = [
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

function findSectionBounds(
  document: vscode.TextDocument,
  sectionName: "front_matter" | "schema" | "data"
): { startLine: number; endLine: number } | undefined {
  const marker = `---${sectionName}`;
  for (let i = 0; i < document.lineCount; i += 1) {
    if (document.lineAt(i).text.trim() !== marker) {
      continue;
    }

    let endLine = document.lineCount;
    for (let j = i + 1; j < document.lineCount; j += 1) {
      const lineText = document.lineAt(j).text.trim();
      if (/^---(front_matter|schema|data)\s*$/.test(lineText)) {
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
