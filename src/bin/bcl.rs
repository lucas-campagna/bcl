use clap::Parser;
use bcl::{parse_file, Context, FlowRuntime, run_actions};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "bcl")]
#[command(about = "Execute browser automation flows", long_about = None)]
struct Args {
    /// Name of the flow to execute (e.g., auth from clear.yml)
    flow_name: String,
    /// Path to the YAML file containing the flow definition
    path: PathBuf,
    /// Output format: simple, pretty, json
    #[arg(short, long, default_value = "simple")]
    output: OutputFormat,
    /// Verbose mode - show debug info and full error traces
    #[arg(short, long)]
    verbose: bool,
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

async fn run_flow(flow_name: &str, path: &PathBuf, output: OutputFormat, verbose: bool) -> anyhow::Result<()> {
    if verbose {
        eprintln!("Loading flow '{}' from {:?}", flow_name, path);
        eprintln!("Connecting to Playwright at: {}", get_browser_url());
    }

    let flow_file = parse_file(path).map_err(|e| {
        if verbose {
            eprintln!("Failed to parse YAML: {}", e);
        }
        anyhow::anyhow!("Failed to parse YAML: {}", e)
    })?;

    let actions = flow_file.get(flow_name).ok_or_else(|| {
        if verbose {
            eprintln!("Flow '{}' not found in {:?}", flow_name, path);
            eprintln!("Available flows: {:?}", flow_file.flows.keys().collect::<Vec<_>>());
        }
        anyhow::anyhow!("Flow '{}' not found", flow_name)
    })?;

    if verbose {
        eprintln!("Found flow '{}' with {} actions", flow_name, actions.len());
    }

    let mut context = Context::from_env();

    if verbose {
        eprintln!("Executing flow '{}'...", flow_name);
    }

    let runtime = FlowRuntime::connect(&get_browser_url()).await;

    match runtime {
        Ok(_rt) => {
            let result = run_actions(&get_browser_url(), actions, &mut context, verbose, &flow_file).await;

            match result {
                Ok(_) => {
                    match output {
                        OutputFormat::Json => {
                            let json = JsonOutput {
                                success: true,
                                flow_name: flow_name.to_string(),
                                actions_executed: actions.len(),
                                error: None,
                            };
                            println!("{}", serde_json::to_string_pretty(&json).unwrap());
                        }
                        OutputFormat::Pretty | OutputFormat::Simple => {
                            if verbose {
                                println!("Flow '{}' completed successfully ({} actions)", flow_name, actions.len());
                            } else {
                                println!("OK");
                            }
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    if verbose {
                        eprintln!("Flow execution failed: {}", e);
                    }
                    Err(anyhow::anyhow!("Flow execution failed: {}", e))
                }
            }
        }
        Err(e) => {
            if verbose {
                eprintln!("Connection to Playwright failed: {}", e);
            }
            Err(anyhow::anyhow!("Connection to Playwright failed: {}", e))
        }
    }
}

fn main() {
    let args = Args::parse();

    let result = tokio::runtime::Runtime::new().unwrap().block_on(run_flow(&args.flow_name, &args.path, args.output, args.verbose));

    if let Err(e) = result {
        if args.verbose {
            eprintln!("Error: {}", e);
            if let Some(source) = std::error::Error::source(&*e) {
                eprintln!("Caused by: {}", source);
            }
        }
        std::process::exit(1);
    }
}
