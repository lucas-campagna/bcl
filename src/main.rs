use clap::Parser;
use bcl::{parse_file, Context, FlowRuntime, run_actions};
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
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Name of the flow to execute
    pub flow_name: String,
    /// Path to the YAML file containing the flow definition
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
        eprintln!("Loading flow '{}' from {:?}", args.flow_name, args.path);
        eprintln!("Connecting to Playwright at: {}", get_browser_url());
    }

    let flow_file = parse_file(&args.path).map_err(|e| {
        if args.verbose {
            eprintln!("Failed to parse YAML: {}", e);
        }
        anyhow::anyhow!("Failed to parse YAML: {}", e)
    })?;

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
    let flow_file = parse_file(path).map_err(|e| {
        if verbose {
            eprintln!("Failed to parse YAML: {}", e);
        }
        anyhow::anyhow!("Failed to parse YAML: {}", e)
    })?;

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
    let flow_file = parse_file(path).map_err(|e| anyhow::anyhow!("Failed to parse YAML: {}", e))?;

    println!("Flows in {:?}:", path);
    let mut names: Vec<_> = flow_file.flows.keys().collect();
    names.sort();
    for name in names {
        let count = flow_file.flows.get(name).map(|a| a.len()).unwrap_or(0);
        println!("  {} ({} action(s))", name, count);
    }
    println!("({} total)", flow_file.flows.len());
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
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
