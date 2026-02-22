use std::{collections::HashSet, env, path::PathBuf, process::ExitCode};

use super_yaml::{
    compile_document_from_path_with_fetch, generate_proto_types_from_path,
    generate_rust_types_from_path, generate_typescript_types_from_path, EnvProvider,
    ProcessEnvProvider,
};

#[derive(Clone, Copy, Debug)]
enum OutputFormat {
    Json,
    Yaml,
    Rust,
    TypeScript,
    Proto,
}

#[derive(Debug)]
struct CompileOptions {
    pretty: bool,
    format: OutputFormat,
    allowed_env_keys: HashSet<String>,
    cache_dir: Option<PathBuf>,
    update_imports: bool,
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
    if args.len() < 3 {
        return Err("not enough arguments".to_string());
    }

    let command = args[1].as_str();
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
            )
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
) -> Result<(), String> {
    let output = match format {
        OutputFormat::Json => {
            let compiled = compile_document_from_path_with_fetch(file, env, cache_dir, update_imports)
                .map_err(|e| e.to_string())?;
            for warning in &compiled.warnings {
                eprintln!("warning: {warning}");
            }
            compiled.to_json_string(pretty)
        }
        OutputFormat::Yaml => {
            let compiled = compile_document_from_path_with_fetch(file, env, cache_dir, update_imports)
                .map_err(|e| e.to_string())?;
            for warning in &compiled.warnings {
                eprintln!("warning: {warning}");
            }
            Ok(compiled.to_yaml_string())
        }
        OutputFormat::Rust => generate_rust_types_from_path(file),
        OutputFormat::TypeScript => generate_typescript_types_from_path(file),
        OutputFormat::Proto => generate_proto_types_from_path(file),
    }
    .map_err(|e| e.to_string())?;

    println!("{output}");
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
            "--format" => {
                if i + 1 >= args.len() {
                    return Err(
                        "missing value for --format (expected json, yaml, rust, ts, typescript, or proto)"
                            .to_string(),
                    );
                }
                format = match args[i + 1].as_str() {
                    "json" => OutputFormat::Json,
                    "yaml" => OutputFormat::Yaml,
                    "rust" => OutputFormat::Rust,
                    "ts" | "typescript" => OutputFormat::TypeScript,
                    "proto" => OutputFormat::Proto,
                    other => {
                        return Err(format!(
                            "invalid --format value '{other}' (expected json, yaml, rust, ts, typescript, or proto)"
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

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  super-yaml validate <file> [--allow-env KEY]...");
    eprintln!(
        "  super-yaml compile <file> [--pretty] [--format json|yaml|rust|ts|typescript|proto] [--allow-env KEY]..."
    );
    eprintln!("  super-yaml compile <file> [--yaml|--json|--rust|--ts|--proto] [--allow-env KEY]...");
    eprintln!();
    eprintln!("import options:");
    eprintln!("  --update-imports       force re-fetch of all URL imports (bypass lockfile cache)");
    eprintln!("  --cache-dir <path>     override default URL import cache directory");
    eprintln!();
    eprintln!(
        "note: environment access is disabled by default; use --allow-env to permit specific keys."
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_compile_options, parse_validate_options, OutputFormat};

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
}
