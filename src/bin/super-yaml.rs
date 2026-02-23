use std::{collections::{BTreeMap, BTreeSet, HashSet}, env, path::{Path, PathBuf}, process::ExitCode};

use super_yaml::{
    collect_import_graph, discover_module_members, generate_html_docs_site,
    compile_document_from_path_with_fetch, from_json_schema_path,
    generate_html_docs_from_path, generate_proto_types_from_path,
    generate_rust_types_and_data_from_path, generate_rust_types_from_path,
    generate_typescript_types_and_data_from_path, generate_typescript_types_from_path,
    EnvProvider, ProcessEnvProvider,
};
use super_yaml::{parse_document, to_json_schema};

#[derive(Clone, Copy, Debug)]
enum OutputFormat {
    Json,
    JsonSchema,
    Yaml,
    Rust,
    TypeScript,
    Proto,
    FunctionalJson,
    HtmlDocs,
}

#[derive(Debug)]
struct CompileOptions {
    pretty: bool,
    format: OutputFormat,
    allowed_env_keys: HashSet<String>,
    cache_dir: Option<PathBuf>,
    update_imports: bool,
    skip_data: bool,
}

#[derive(Debug)]
struct DocsOptions {
    output_dir: PathBuf,
    follow_imports: bool,
}

/// Env provider that allows only explicitly listed process env keys.
struct AllowListEnvProvider {
    allowed_env_keys: HashSet<String>,
    process_env: ProcessEnvProvider,
}

impl AllowListEnvProvider {
    fn new(allowed_env_keys: HashSet<String>) -> Self {
        Self {
            allowed_env_keys,
            process_env: ProcessEnvProvider,
        }
    }
}

impl EnvProvider for AllowListEnvProvider {
    fn get(&self, key: &str) -> Option<String> {
        if self.allowed_env_keys.contains(key) {
            self.process_env.get(key)
        } else {
            None
        }
    }
}

fn main() -> ExitCode {
    match run(env::args().collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    if args.len() < 2 {
        return Err("not enough arguments".to_string());
    }

    let command = args[1].as_str();

    if command == "from-json-schema" {
        if args.len() < 3 {
            return Err("from-json-schema requires an input file".to_string());
        }
        let file = PathBuf::from(&args[2]);
        let output_path = parse_from_json_schema_options(&args[3..])?;
        return run_from_json_schema(&file, output_path.as_deref());
    }

    if args.len() < 3 {
        return Err("not enough arguments".to_string());
    }

    let file = PathBuf::from(&args[2]);

    match command {
        "validate" => {
            let allowed_env_keys = parse_validate_options(&args[3..])?;
            let env_provider = AllowListEnvProvider::new(allowed_env_keys);
            run_validate(&file, &env_provider)
        }
        "compile" => {
            let options = parse_compile_options(&args[3..])?;
            let env_provider = AllowListEnvProvider::new(options.allowed_env_keys);
            run_compile(
                &file,
                &env_provider,
                options.pretty,
                options.format,
                options.cache_dir,
                options.update_imports,
                options.skip_data,
            )
        }
        "docs" => {
            let parsed_options = parse_docs_options(&args[3..])?;
            run_docs(&file, &parsed_options)
        }
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn run_validate(file: &PathBuf, env: &dyn EnvProvider) -> Result<(), String> {
    let compiled = super_yaml::compile_document_from_path(file, env).map_err(|e| e.to_string())?;
    for warning in &compiled.warnings {
        eprintln!("warning: {warning}");
    }
    println!("OK");
    Ok(())
}

fn run_compile(
    file: &PathBuf,
    env: &dyn EnvProvider,
    pretty: bool,
    format: OutputFormat,
    cache_dir: Option<PathBuf>,
    update_imports: bool,
    skip_data: bool,
) -> Result<(), String> {
    let output = match format {
        OutputFormat::Json => {
            let compiled =
                compile_document_from_path_with_fetch(file, env, cache_dir, update_imports)
                    .map_err(|e| e.to_string())?;
            for warning in &compiled.warnings {
                eprintln!("warning: {warning}");
            }
            compiled.to_json_string(pretty)
        }
        OutputFormat::Yaml => {
            let compiled =
                compile_document_from_path_with_fetch(file, env, cache_dir, update_imports)
                    .map_err(|e| e.to_string())?;
            for warning in &compiled.warnings {
                eprintln!("warning: {warning}");
            }
            Ok(compiled.to_yaml_string())
        }
        OutputFormat::Rust => {
            if skip_data {
                generate_rust_types_from_path(file)
            } else {
                generate_rust_types_and_data_from_path(file, env)
            }
        }
        OutputFormat::TypeScript => {
            if skip_data {
                generate_typescript_types_from_path(file)
            } else {
                generate_typescript_types_and_data_from_path(file, env)
            }
        }
        OutputFormat::Proto => generate_proto_types_from_path(file),
        OutputFormat::FunctionalJson => {
            let input = std::fs::read_to_string(file)
                .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
            let parsed = parse_document(&input).map_err(|e| e.to_string())?;
            match parsed.functional {
                Some(ref func_doc) => {
                    super_yaml::functional::functional_to_json(func_doc, pretty)
                }
                None => Ok("{}".to_string()),
            }
        }
        OutputFormat::JsonSchema => {
            let input = std::fs::read_to_string(file)
                .map_err(|e| format!("failed to read '{}': {e}", file.display()))?;
            let parsed = parse_document(&input).map_err(|e| e.to_string())?;
            to_json_schema(&parsed.schema, pretty)
        }
        OutputFormat::HtmlDocs => generate_html_docs_from_path(file),
    }
    .map_err(|e| e.to_string())?;

    println!("{output}");
    Ok(())
}

fn run_docs(input_path: &PathBuf, options: &DocsOptions) -> Result<(), String> {
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();

    let base_dir: PathBuf;

    let file_name = input_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if file_name == "module.syaml" {
        // It's a module manifest: discover all member .syaml files
        base_dir = input_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let members = discover_module_members(input_path).map_err(|e| e.to_string())?;
        for member in members {
            if options.follow_imports {
                let graph = collect_import_graph(&member).map_err(|e| e.to_string())?;
                for path in graph.into_keys() {
                    roots.insert(path);
                }
            }
            roots.insert(member);
        }
    } else if input_path.is_dir() {
        base_dir = input_path.to_path_buf();

        let entries = std::fs::read_dir(input_path)
            .map_err(|e| format!("failed to read directory '{}': {e}", input_path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("failed to read directory entry: {e}"))?;
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "syaml" {
                        if options.follow_imports {
                            let graph = collect_import_graph(&path).map_err(|e| e.to_string())?;
                            for p in graph.into_keys() {
                                roots.insert(p);
                            }
                        }
                        roots.insert(path);
                    }
                }
            }
        }
    } else {
        // Regular .syaml file
        base_dir = input_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        if options.follow_imports {
            let graph = collect_import_graph(input_path).map_err(|e| e.to_string())?;
            for path in graph.into_keys() {
                roots.insert(path);
            }
        }
        roots.insert(input_path.clone());
    }

    let roots_vec: Vec<PathBuf> = roots.into_iter().collect();
    let files: BTreeMap<String, String> =
        generate_html_docs_site(&roots_vec, &base_dir).map_err(|e| e.to_string())?;

    let output_dir = &options.output_dir;
    let count = files.len();

    for (relative_path, content) in &files {
        let dest = output_dir.join(relative_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create directory '{}': {e}", parent.display()))?;
        }
        std::fs::write(&dest, content)
            .map_err(|e| format!("failed to write '{}': {e}", dest.display()))?;
        eprintln!("wrote: {}", dest.display());
    }

    eprintln!("Generated {count} files in {}", output_dir.display());
    Ok(())
}

fn parse_validate_options(args: &[String]) -> Result<HashSet<String>, String> {
    let mut allowed_env_keys = HashSet::new();
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--allow-env" => parse_allow_env_option(args, &mut i, &mut allowed_env_keys)?,
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(allowed_env_keys)
}

fn parse_compile_options(args: &[String]) -> Result<CompileOptions, String> {
    let mut pretty = false;
    let mut format = OutputFormat::Json;
    let mut allowed_env_keys = HashSet::new();
    let mut cache_dir: Option<PathBuf> = None;
    let mut update_imports = false;
    let mut skip_data = false;
    let mut i = 0usize;

    while i < args.len() {
        match args[i].as_str() {
            "--pretty" => {
                pretty = true;
                i += 1;
            }
            "--yaml" => {
                format = OutputFormat::Yaml;
                i += 1;
            }
            "--rust" => {
                format = OutputFormat::Rust;
                i += 1;
            }
            "--ts" => {
                format = OutputFormat::TypeScript;
                i += 1;
            }
            "--json" => {
                format = OutputFormat::Json;
                i += 1;
            }
            "--proto" => {
                format = OutputFormat::Proto;
                i += 1;
            }
            "--json-schema" => {
                format = OutputFormat::JsonSchema;
                i += 1;
            }
            "--functional-json" => {
                format = OutputFormat::FunctionalJson;
                i += 1;
            }
            "--html" => {
                format = OutputFormat::HtmlDocs;
                i += 1;
            }
            "--format" => {
                if i + 1 >= args.len() {
                    return Err(
                        "missing value for --format (expected json, json-schema, yaml, rust, ts, typescript, proto, or html)"
                            .to_string(),
                    );
                }
                format = match args[i + 1].as_str() {
                    "json" => OutputFormat::Json,
                    "json-schema" => OutputFormat::JsonSchema,
                    "yaml" => OutputFormat::Yaml,
                    "rust" => OutputFormat::Rust,
                    "ts" | "typescript" => OutputFormat::TypeScript,
                    "proto" => OutputFormat::Proto,
                    "functional-json" => OutputFormat::FunctionalJson,
                    "html" => OutputFormat::HtmlDocs,
                    other => {
                        return Err(format!(
                            "invalid --format value '{other}' (expected json, json-schema, yaml, rust, ts, typescript, proto, or html)"
                        ))
                    }
                };
                i += 2;
            }
            "--allow-env" => parse_allow_env_option(args, &mut i, &mut allowed_env_keys)?,
            "--update-imports" => {
                update_imports = true;
                i += 1;
            }
            "--skip-data" => {
                skip_data = true;
                i += 1;
            }
            "--cache-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --cache-dir".to_string());
                }
                cache_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => {
                return Err(format!("unknown option '{other}'"));
            }
        }
    }

    Ok(CompileOptions {
        pretty,
        format,
        allowed_env_keys,
        cache_dir,
        update_imports,
        skip_data,
    })
}

fn parse_docs_options(args: &[String]) -> Result<DocsOptions, String> {
    let mut output_dir: Option<PathBuf> = None;
    let mut follow_imports = false;
    let mut i = 0usize;

    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --output".to_string());
                }
                output_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--follow-imports" => {
                follow_imports = true;
                i += 1;
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }

    let output_dir = output_dir.ok_or_else(|| "--output <dir> is required for docs command".to_string())?;

    Ok(DocsOptions {
        output_dir,
        follow_imports,
    })
}

fn parse_allow_env_option(
    args: &[String],
    i: &mut usize,
    allowed_env_keys: &mut HashSet<String>,
) -> Result<(), String> {
    if *i + 1 >= args.len() {
        return Err(
            "missing value for --allow-env (expected environment variable name)".to_string(),
        );
    }

    let key = args[*i + 1].trim();
    if key.is_empty() {
        return Err("--allow-env value must be non-empty".to_string());
    }

    allowed_env_keys.insert(key.to_string());
    *i += 2;
    Ok(())
}

fn parse_from_json_schema_options(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut output: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --output".to_string());
                }
                output = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(output)
}

fn run_from_json_schema(file: &PathBuf, output: Option<&Path>) -> Result<(), String> {
    let syaml = from_json_schema_path(file).map_err(|e| e.to_string())?;
    match output {
        Some(path) => {
            std::fs::write(path, &syaml).map_err(|e| format!("failed to write output: {e}"))
        }
        None => {
            print!("{syaml}");
            Ok(())
        }
    }
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  super-yaml from-json-schema <schema.json> [--output <file.syaml>]");
    eprintln!("  super-yaml validate <file> [--allow-env KEY]...");
    eprintln!(
        "  super-yaml compile <file> [--pretty] [--format json|yaml|rust|ts|typescript|proto|html] [--allow-env KEY]..."
    );
    eprintln!("  super-yaml compile <file> [--yaml|--json|--rust|--ts|--proto|--html] [--allow-env KEY]...");
    eprintln!("  super-yaml docs <path> --output <dir> [--follow-imports]");
    eprintln!();
    eprintln!("codegen options (--rust / --ts):");
    eprintln!("  --skip-data            emit type definitions only; omit data constants/fns");
    eprintln!();
    eprintln!("import options:");
    eprintln!("  --update-imports       force re-fetch of all URL imports (bypass lockfile cache)");
    eprintln!("  --cache-dir <path>     override default URL import cache directory");
    eprintln!();
    eprintln!("docs options:");
    eprintln!("  --output <dir>         directory to write generated HTML files into");
    eprintln!("  --follow-imports       also generate docs for all transitively imported files");
    eprintln!();
    eprintln!(
        "note: environment access is disabled by default; use --allow-env to permit specific keys."
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_compile_options, parse_docs_options, parse_validate_options, OutputFormat};

    #[test]
    fn parse_compile_yaml_format() {
        let args = vec!["--format".to_string(), "yaml".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::Yaml));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_pretty_and_yaml_shortcut() {
        let args = vec!["--pretty".to_string(), "--yaml".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(options.pretty);
        assert!(matches!(options.format, OutputFormat::Yaml));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_rust_format() {
        let args = vec!["--format".to_string(), "rust".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::Rust));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_ts_format() {
        let args = vec!["--format".to_string(), "ts".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::TypeScript));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_typescript_format() {
        let args = vec!["--format".to_string(), "typescript".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::TypeScript));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_rust_shortcut() {
        let args = vec!["--rust".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::Rust));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_ts_shortcut() {
        let args = vec!["--ts".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::TypeScript));
        assert!(options.allowed_env_keys.is_empty());
    }

    #[test]
    fn parse_compile_allow_env_repeatable() {
        let args = vec![
            "--allow-env".to_string(),
            "CPU_CORES".to_string(),
            "--allow-env".to_string(),
            "DB_HOST".to_string(),
        ];
        let options = parse_compile_options(&args).unwrap();
        assert!(options.allowed_env_keys.contains("CPU_CORES"));
        assert!(options.allowed_env_keys.contains("DB_HOST"));
    }

    #[test]
    fn parse_validate_allow_env_repeatable() {
        let args = vec![
            "--allow-env".to_string(),
            "CPU_CORES".to_string(),
            "--allow-env".to_string(),
            "DB_HOST".to_string(),
        ];
        let allowed = parse_validate_options(&args).unwrap();
        assert!(allowed.contains("CPU_CORES"));
        assert!(allowed.contains("DB_HOST"));
    }

    #[test]
    fn parse_allow_env_requires_value() {
        let args = vec!["--allow-env".to_string()];
        let err = parse_compile_options(&args).unwrap_err();
        assert!(err.contains("missing value for --allow-env"));
    }

    #[test]
    fn parse_compile_invalid_format_mentions_ts_options() {
        let args = vec!["--format".to_string(), "wat".to_string()];
        let err = parse_compile_options(&args).unwrap_err();
        assert!(err.contains("ts"));
        assert!(err.contains("typescript"));
    }

    #[test]
    fn parse_docs_requires_output() {
        let args = vec!["--follow-imports".to_string()];
        let err = parse_docs_options(&args).unwrap_err();
        assert!(err.contains("--output"));
    }

    #[test]
    fn parse_docs_output_and_follow_imports() {
        let args = vec![
            "--output".to_string(),
            "/tmp/docs".to_string(),
            "--follow-imports".to_string(),
        ];
        let opts = parse_docs_options(&args).unwrap();
        assert_eq!(opts.output_dir.to_str().unwrap(), "/tmp/docs");
        assert!(opts.follow_imports);
    }

    #[test]
    fn parse_docs_output_only() {
        let args = vec!["--output".to_string(), "/tmp/docs".to_string()];
        let opts = parse_docs_options(&args).unwrap();
        assert_eq!(opts.output_dir.to_str().unwrap(), "/tmp/docs");
        assert!(!opts.follow_imports);
    }
}
