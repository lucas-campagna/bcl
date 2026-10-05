use clap::Parser;
use bcl::{parse_file, parse_stdin, Context, FlowRuntime, run_actions};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "bcl")]
#[command(about = "Browser Control Language — execute YAML-based browser automation flows", long_about = None)]
pub enum Cli {
    /// Run a flow
    Run(RunArgs),
    /// Validate a flow YAML file (checks syntax and structure)
    Validate {
        /// Path to the YAML file
        path: PathBuf,
    },
    /// List all flows defined in a YAML file
    List {
        /// Path to the YAML file
        path: PathBuf,
    },
    /// Compile a YAML flow file into a standalone shell script
    Compile(CompileArgs),
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Name of the flow to execute
    pub flow_name: String,
    /// Path to the YAML file containing the flow definition (use - for stdin)
    pub path: PathBuf,
    /// Output format: simple, pretty, json
    #[arg(short, long, default_value = "simple")]
    pub output: OutputFormat,
    /// Verbose mode — show debug info and full error traces
    #[arg(short, long)]
    pub verbose: bool,
    /// Override or set context variables (repeatable)
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,
    /// Dry-run — parse and resolve variables, print actions, do not connect to browser
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser, Debug)]
pub struct CompileArgs {
    /// Path to the YAML file to compile
    pub yaml_path: PathBuf,
    /// Optional name for the output file (without extension)
    /// If not provided, uses the YAML filename without extension
    #[arg(short, long)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub enum OutputFormat {
    #[default]
    Simple,
    Pretty,
    Json,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "simple" => Ok(OutputFormat::Simple),
            "pretty" => Ok(OutputFormat::Pretty),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("Invalid output format: {}. Use: simple, pretty, json", s)),
        }
    }
}

#[derive(Serialize)]
struct JsonOutput {
    success: bool,
    flow_name: String,
    actions_executed: usize,
    error: Option<String>,
}

fn get_browser_url() -> String {
    std::env::var("BROWSER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9222".to_string())
}

fn parse_var(s: &str) -> Option<(String, String)> {
    s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))
}

fn apply_vars(context: &mut Context, vars: &[String]) {
    for var in vars {
        if let Some((key, value)) = parse_var(var) {
            let yaml_value = Context::try_parse_yaml(value.as_str());
            context.set_yaml(&key, yaml_value);
        }
    }
}

async fn run_flow(args: &RunArgs) -> anyhow::Result<()> {
    if args.verbose {
        let source = if args.path == PathBuf::from("-") {
            "stdin".to_string()
        } else {
            format!("{:?}", args.path)
        };
        eprintln!("Loading flow '{}' from {}", args.flow_name, source);
        eprintln!("Connecting to Playwright at: {}", get_browser_url());
    }

    let flow_file = if args.path == PathBuf::from("-") {
        parse_stdin().map_err(|e| {
            if args.verbose {
                eprintln!("Failed to read YAML from stdin: {}", e);
            }
            anyhow::anyhow!("Failed to read YAML from stdin: {}", e)
        })?
    } else {
        parse_file(&args.path).map_err(|e| {
            if args.verbose {
                eprintln!("Failed to parse YAML: {}", e);
            }
            anyhow::anyhow!("Failed to parse YAML: {}", e)
        })?
    };

    let actions = flow_file.get(&args.flow_name).ok_or_else(|| {
        if args.verbose {
            eprintln!("Flow '{}' not found in {:?}", args.flow_name, args.path);
            eprintln!("Available flows: {:?}", flow_file.flows.keys().collect::<Vec<_>>());
        }
        anyhow::anyhow!("Flow '{}' not found", args.flow_name)
    })?;

    if args.verbose {
        eprintln!("Found flow '{}' with {} actions", args.flow_name, actions.len());
    }

    let mut context = Context::from_env();
    apply_vars(&mut context, &args.vars);

    if args.dry_run {
        println!("[DRY RUN] Flow: {}", args.flow_name);
        println!("[DRY RUN] Actions ({} total):", actions.len());
        for (i, action) in actions.iter().enumerate() {
            println!("[DRY RUN]   {}. {:?}", i + 1, action);
        }
        println!("[DRY RUN] Context vars:");
        for (k, v) in context.vars() {
            println!("[DRY RUN]   {} = {:?}", k, v);
        }
        return Ok(());
    }

    if args.verbose {
        eprintln!("Executing flow '{}'...", args.flow_name);
    }

    let runtime = FlowRuntime::connect(&get_browser_url()).await;

    match runtime {
        Ok(_rt) => {
            let result = run_actions(&get_browser_url(), actions, &mut context, args.verbose, &flow_file).await;

            match result {
                Ok(_) => {
                    match args.output {
                        OutputFormat::Json => {
                            let json = JsonOutput {
                                success: true,
                                flow_name: args.flow_name.clone(),
                                actions_executed: actions.len(),
                                error: None,
                            };
                            println!("{}", serde_json::to_string_pretty(&json).unwrap());
                        }
                        OutputFormat::Pretty | OutputFormat::Simple => {
                            if args.verbose {
                                println!("Flow '{}' completed successfully ({} actions)", args.flow_name, actions.len());
                            } else {
                                println!("OK");
                            }
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    if args.verbose {
                        eprintln!("Flow execution failed: {}", e);
                    }
                    Err(anyhow::anyhow!("Flow execution failed: {}", e))
                }
            }
        }
        Err(e) => {
            if args.verbose {
                eprintln!("Connection to Playwright failed: {}", e);
            }
            Err(anyhow::anyhow!("Connection to Playwright failed: {}", e))
        }
    }
}

fn validate_file(path: &PathBuf, verbose: bool) -> anyhow::Result<()> {
    let flow_file = if path == &PathBuf::from("-") {
        parse_stdin().map_err(|e| {
            if verbose {
                eprintln!("Failed to read YAML from stdin: {}", e);
            }
            anyhow::anyhow!("Failed to read YAML from stdin: {}", e)
        })?
    } else {
        parse_file(path).map_err(|e| {
            if verbose {
                eprintln!("Failed to parse YAML: {}", e);
            }
            anyhow::anyhow!("Failed to parse YAML: {}", e)
        })?
    };

    let mut warnings = Vec::new();

    for (flow_name, actions) in &flow_file.flows {
        for (i, action) in actions.iter().enumerate() {
            bcl::flow::validator::validate_action(action, flow_name, i + 1, &mut warnings);
        }
    }

    if warnings.is_empty() {
        println!("OK — {} flow(s) valid: {:?}", flow_file.flows.len(), flow_file.flows.keys().collect::<Vec<_>>());
    } else {
        println!("Validation complete — {} flow(s), {} warning(s):", flow_file.flows.len(), warnings.len());
        for w in &warnings {
            println!("  WARNING: {}", w);
        }
    }
    Ok(())
}

fn list_flows(path: &PathBuf) -> anyhow::Result<()> {
    let flow_file = if path == &PathBuf::from("-") {
        parse_stdin().map_err(|e| anyhow::anyhow!("Failed to read YAML from stdin: {}", e))?
    } else {
        parse_file(path).map_err(|e| anyhow::anyhow!("Failed to parse YAML: {}", e))?
    };

    let source = if path == &PathBuf::from("-") {
        "stdin".to_string()
    } else {
        format!("{:?}", path)
    };

    println!("Flows in {}:", source);
    let mut names: Vec<_> = flow_file.flows.keys().collect();
    names.sort();
    for name in names {
        let count = flow_file.flows.get(name).map(|a| a.len()).unwrap_or(0);
        println!("  {} ({} action(s))", name, count);
    }
    println!("({} total)", flow_file.flows.len());
    Ok(())
}

fn compile_flow(yaml_path: &PathBuf, name: Option<&str>) -> anyhow::Result<()> {
    let yaml_content = std::fs::read_to_string(yaml_path)
        .map_err(|e| anyhow::anyhow!("Failed to read YAML file: {}", e))?;

    let flow_file = parse_file(yaml_path)
        .map_err(|e| anyhow::anyhow!("Failed to parse YAML: {}", e))?;

    let flows: Vec<String> = flow_file.flows.keys().cloned().collect();
    if flows.is_empty() {
        return Err(anyhow::anyhow!("No flows found in YAML file"));
    }

    let encoded_yaml = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &yaml_content);

    let output_name = name.map(|s| s.to_string()).unwrap_or_else(|| {
        yaml_path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string()
    });

    let flows_list = flows.iter()
        .map(|f| format!("  {}   - Execute the {} flow", f, f))
        .collect::<Vec<_>>()
        .join("\n");

    let flows_pattern = flows.iter()
        .map(|f| format!("\"{}\"", f))
        .collect::<Vec<_>>()
        .join("|");

    let script = format!(r#"#!/bin/bash
# Auto-generated by bcl compile
# Source: {:?}
# Flows: {}

set -e

show_help() {{
    cat << EOF
Usage: $(basename "$0") <flow> [options]

Available flows:
{}

Options:
  --verbose, -v     Enable verbose output
  --dry-run         Parse and print actions without executing
  --help, -h        Show this help

Environment variables are passed directly to bcl run.

Examples:
  USERNAME=me PASSWORD=secret $(basename "$0") {}
  QUERY="hello" $(basename "$0") {} --verbose
EOF
}}

ENCODED_YAML='{}'

FLOW_NAME=""
VERBOSE=false
DRY_RUN=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        {})
            FLOW_NAME="$1"
            shift
            ;;
        --verbose|-v)
            VERBOSE=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --help|-h)
            show_help
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

if [[ -z "$FLOW_NAME" ]]; then
    echo "Error: flow name required"
    show_help
    exit 1
fi

YAML_CONTENT=$(echo "$ENCODED_YAML" | base64 -d)

EXTRA_ARGS=()
[[ "$VERBOSE" == "true" ]] && EXTRA_ARGS+=(--verbose)
[[ "$DRY_RUN" == "true" ]] && EXTRA_ARGS+=(--dry-run)

exec bcl run "$FLOW_NAME" - "${{EXTRA_ARGS[@]}}" <<< "$YAML_CONTENT"
"#, yaml_path, flows.join(", "), flows_list, flows[0], flows[0], encoded_yaml, flows_pattern);

    let output_path = std::path::Path::new(&output_name);
    std::fs::write(output_path, &script)?;
    
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_path, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("Generated: {}", output_name);
    Ok(())
}

fn main() {
    let args = Cli::parse();

    let result = match args {
        Cli::Run(ref run_args) => {
            tokio::runtime::Runtime::new().unwrap().block_on(run_flow(run_args))
        }
        Cli::Validate { ref path } => {
            validate_file(path, true)
        }
        Cli::List { ref path } => {
            list_flows(path)
        }
        Cli::Compile(ref args) => {
            compile_flow(&args.yaml_path, args.name.as_deref())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
