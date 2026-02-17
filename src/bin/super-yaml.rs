use std::{env, fs, path::PathBuf, process::ExitCode};

use super_yaml::{
    compile_document_to_json, compile_document_to_yaml, validate_document, ProcessEnvProvider,
};

#[derive(Clone, Copy)]
enum OutputFormat {
    Json,
    Yaml,
}

fn main() -> ExitCode {
    let env_provider = ProcessEnvProvider;
    match run(env::args().collect(), &env_provider) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>, env: &ProcessEnvProvider) -> Result<(), String> {
    if args.len() < 3 {
        return Err("not enough arguments".to_string());
    }

    let command = args[1].as_str();
    let file = PathBuf::from(&args[2]);

    match command {
        "validate" => run_validate(&file, env),
        "compile" => {
            let (pretty, format) = parse_compile_options(&args[3..])?;
            run_compile(&file, env, pretty, format)
        }
        _ => Err(format!("unknown command '{command}'")),
    }
}

fn run_validate(file: &PathBuf, env: &ProcessEnvProvider) -> Result<(), String> {
    let input =
        fs::read_to_string(file).map_err(|e| format!("failed to read {}: {e}", file.display()))?;
    validate_document(&input, env).map_err(|e| e.to_string())?;
    println!("OK");
    Ok(())
}

fn run_compile(
    file: &PathBuf,
    env: &ProcessEnvProvider,
    pretty: bool,
    format: OutputFormat,
) -> Result<(), String> {
    let input =
        fs::read_to_string(file).map_err(|e| format!("failed to read {}: {e}", file.display()))?;
    let output = match format {
        OutputFormat::Json => compile_document_to_json(&input, env, pretty),
        OutputFormat::Yaml => compile_document_to_yaml(&input, env),
    }
    .map_err(|e| e.to_string())?;

    println!("{output}");
    Ok(())
}

fn parse_compile_options(args: &[String]) -> Result<(bool, OutputFormat), String> {
    let mut pretty = false;
    let mut format = OutputFormat::Json;
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
            "--json" => {
                format = OutputFormat::Json;
                i += 1;
            }
            "--format" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --format (expected json or yaml)".to_string());
                }
                format = match args[i + 1].as_str() {
                    "json" => OutputFormat::Json,
                    "yaml" => OutputFormat::Yaml,
                    other => {
                        return Err(format!(
                            "invalid --format value '{other}' (expected json or yaml)"
                        ))
                    }
                };
                i += 2;
            }
            other => {
                return Err(format!("unknown option '{other}'"));
            }
        }
    }

    Ok((pretty, format))
}

fn print_usage() {
    eprintln!("usage:");
    eprintln!("  super-yaml validate <file>");
    eprintln!("  super-yaml compile <file> [--pretty] [--format json|yaml]");
    eprintln!("  super-yaml compile <file> [--yaml|--json]");
}

#[cfg(test)]
mod tests {
    use super::{parse_compile_options, OutputFormat};

    #[test]
    fn parse_compile_yaml_format() {
        let args = vec!["--format".to_string(), "yaml".to_string()];
        let (_pretty, format) = parse_compile_options(&args).unwrap();
        assert!(matches!(format, OutputFormat::Yaml));
    }

    #[test]
    fn parse_compile_pretty_and_yaml_shortcut() {
        let args = vec!["--pretty".to_string(), "--yaml".to_string()];
        let (pretty, format) = parse_compile_options(&args).unwrap();
        assert!(pretty);
        assert!(matches!(format, OutputFormat::Yaml));
    }
}
