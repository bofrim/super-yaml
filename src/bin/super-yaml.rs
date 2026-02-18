use std::{collections::HashSet, env, path::PathBuf, process::ExitCode};

use super_yaml::{
    compile_document_from_path, generate_rust_types_from_path, validate_document_from_path,
    EnvProvider, ProcessEnvProvider,
};

#[derive(Clone, Copy, Debug)]
enum OutputFormat {
    Json,
    Yaml,
    Rust,
}

#[derive(Debug)]
struct CompileOptions {
    pretty: bool,
    format: OutputFormat,
    allowed_env_keys: HashSet<String>,
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
            run_compile(&file, &env_provider, options.pretty, options.format)
        }
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn run_validate(file: &PathBuf, env: &dyn EnvProvider) -> Result<(), String> {
    validate_document_from_path(file, env).map_err(|e| e.to_string())?;
    println!("OK");
    Ok(())
}

fn run_compile(
    file: &PathBuf,
    env: &dyn EnvProvider,
    pretty: bool,
    format: OutputFormat,
) -> Result<(), String> {
    let output = match format {
        OutputFormat::Json => compile_document_from_path(file, env)
            .map_err(|e| e.to_string())?
            .to_json_string(pretty),
        OutputFormat::Yaml => Ok(compile_document_from_path(file, env)
            .map_err(|e| e.to_string())?
            .to_yaml_string()),
        OutputFormat::Rust => generate_rust_types_from_path(file),
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
            "--json" => {
                format = OutputFormat::Json;
                i += 1;
            }
            "--format" => {
                if i + 1 >= args.len() {
                    return Err(
                        "missing value for --format (expected json, yaml, or rust)".to_string()
                    );
                }
                format = match args[i + 1].as_str() {
                    "json" => OutputFormat::Json,
                    "yaml" => OutputFormat::Yaml,
                    "rust" => OutputFormat::Rust,
                    other => {
                        return Err(format!(
                            "invalid --format value '{other}' (expected json, yaml, or rust)"
                        ))
                    }
                };
                i += 2;
            }
            "--allow-env" => parse_allow_env_option(args, &mut i, &mut allowed_env_keys)?,
            other => {
                return Err(format!("unknown option '{other}'"));
            }
        }
    }

    Ok(CompileOptions {
        pretty,
        format,
        allowed_env_keys,
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
        "  super-yaml compile <file> [--pretty] [--format json|yaml|rust] [--allow-env KEY]..."
    );
    eprintln!("  super-yaml compile <file> [--yaml|--json|--rust] [--allow-env KEY]...");
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
    fn parse_compile_rust_shortcut() {
        let args = vec!["--rust".to_string()];
        let options = parse_compile_options(&args).unwrap();
        assert!(!options.pretty);
        assert!(matches!(options.format, OutputFormat::Rust));
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
}
