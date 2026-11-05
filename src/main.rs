use clap::Parser;
use bcl::{parse_file, parse_stdin, parse_str, Context, FlowRuntime, run_actions};
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
    /// Execute one BCL browser action
    Exec(ExecArgs),
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Path to the YAML file containing the flow definition (use - for stdin)
    pub path: PathBuf,
    /// Target to execute
    #[arg(short, long, default_value = "main")]
    pub target: String,
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

#[derive(Parser, Debug)]
#[command(trailing_var_arg = true)]
pub struct ExecArgs {
    /// BCL action to execute (required unless --stdin is used)
    #[arg(value_name = "ACTION")]
    pub action: Option<String>,
    /// The action's primary value (required unless --stdin is used)
    #[arg(value_name = "VALUE")]
    pub value: Option<String>,
    /// Optional action parameters in `--name value` or `--name=value` form
    #[arg(value_name = "OPTION", num_args = 0.., allow_hyphen_values = true)]
    pub options: Vec<String>,
    /// Read actions from stdin (one per line)
    #[arg(long = "stdin", short = 's', help = "Read multiple actions from stdin, one per line")]
    pub from_stdin: bool,
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
        eprintln!("Loading target '{}' from {}", args.target, source);
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

    let actions = flow_file.get(&args.target).ok_or_else(|| {
        if args.verbose {
            eprintln!("Target '{}' not found in {:?}", args.target, args.path);
            eprintln!("Available flows: {:?}", flow_file.flows.keys().collect::<Vec<_>>());
        }
        anyhow::anyhow!("Target '{}' not found", args.target)
    })?;

    if args.verbose {
        eprintln!("Found target '{}' with {} actions", args.target, actions.len());
    }

    let mut context = Context::from_env();
    apply_vars(&mut context, &args.vars);

    if args.dry_run {
        println!("[DRY RUN] Target: {}", args.target);
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
        eprintln!("Executing target '{}'...", args.target);
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
                                flow_name: args.target.clone(),
                                actions_executed: actions.len(),
                                error: None,
                            };
                            println!("{}", serde_json::to_string_pretty(&json).unwrap());
                        }
                        OutputFormat::Pretty | OutputFormat::Simple => {
                            if args.verbose {
                                println!("Target '{}' completed successfully ({} actions)", args.target, actions.len());
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

exec bcl run -t "$FLOW_NAME" - "${{EXTRA_ARGS[@]}}" <<< "$YAML_CONTENT"
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

fn parse_exec_scalar(action: &str, field: Option<&str>, value: &str) -> anyhow::Result<serde_yaml::Value> {
    let bool_field = matches!(
        (action, field),
        ("type", Some("secret"))
            | ("read", Some("html" | "value"))
            | ("define" | "set", Some("overwrite"))
            | ("tab", Some("close"))
    );
    if bool_field {
        let parsed = value.parse::<bool>().map_err(|_| {
            anyhow::anyhow!("Expected true or false for parameter '{}', got {:?}", field.unwrap(), value)
        })?;
        return Ok(serde_yaml::Value::Bool(parsed));
    }

    let integer_field = matches!(field, Some("timeout" | "duration" | "max" | "interval" | "port"));
    let signed_integer_field = matches!(field, Some("x" | "y"));
    let numeric_primary = field.is_none() && matches!(action, "tab" | "wait" | "retry");
    let define_value = matches!((action, field), ("define" | "set", Some("value")));

    if integer_field || numeric_primary {
        if let Ok(parsed) = value.parse::<u64>() {
            return Ok(serde_yaml::Value::Number(parsed.into()));
        }
    } else if signed_integer_field {
        if let Ok(parsed) = value.parse::<i64>() {
            return Ok(serde_yaml::Value::Number(parsed.into()));
        }
    } else if define_value {
        if value == "true" || value == "false" {
            return Ok(serde_yaml::Value::Bool(value == "true"));
        }
        if let Ok(parsed) = value.parse::<i64>() {
            return Ok(serde_yaml::Value::Number(parsed.into()));
        }
    }

    Ok(serde_yaml::Value::String(value.to_string()))
}

fn parse_action_line(line: &str) -> anyhow::Result<serde_yaml::Mapping> {
    // Parse line with support for quoted strings
    let parts = parse_shell_words(line)?;
    if parts.is_empty() {
        return Err(anyhow::anyhow!("Empty action line"));
    }
    let action_name = &parts[0];
    let value = parts.get(1).map(|s| s.as_str()).unwrap_or("");
    let options: Vec<&str> = parts[2..].iter().map(|s| s.as_str()).collect();

    let mut action = serde_yaml::Mapping::new();
    action.insert(
        serde_yaml::Value::String(action_name.to_string()),
        parse_exec_scalar(action_name, None, value)?,
    );

    let mut index = 0;
    while index < options.len() {
        let option = options[index]
            .strip_prefix("--")
            .ok_or_else(|| anyhow::anyhow!("Expected an option beginning with '--', got {:?}", options[index]))?;
        if option.is_empty() {
            return Err(anyhow::anyhow!("Expected an option name after '--'"));
        }

        let (field, field_value, consumed) = if let Some((field, val)) = option.split_once('=') {
            (field, val, 1)
        } else {
            let val = options.get(index + 1).ok_or_else(|| {
                anyhow::anyhow!("Option '--{}' requires a value", option)
            })?;
            (option, *val, 2)
        };
        if field.is_empty() {
            return Err(anyhow::anyhow!("Expected an option name after '--'"));
        }

        let field = field.replace('-', "_");
        action.insert(
            serde_yaml::Value::String(field.clone()),
            parse_exec_scalar(action_name, Some(&field), field_value)?,
        );
        index += consumed;
    }

    Ok(action)
}

fn parse_shell_words(s: &str) -> anyhow::Result<Vec<String>> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            c if c.is_whitespace() && !in_single_quote && !in_double_quote => {
                if !current.is_empty() {
                    result.push(current);
                    current = String::new();
                }
            }
            c => {
                current.push(c);
            }
        }
    }

    if in_single_quote {
        return Err(anyhow::anyhow!("Unclosed single quote"));
    }
    if in_double_quote {
        return Err(anyhow::anyhow!("Unclosed double quote"));
    }

    if !current.is_empty() {
        result.push(current);
    }

    Ok(result)
}

fn parse_exec_action(args: &ExecArgs) -> anyhow::Result<bcl::FlowFile> {
    let mut actions = Vec::new();

    if args.from_stdin {
        use std::io::{self, BufRead};
        let stdin = io::stdin();
        for (line_num, line) in stdin.lock().lines().enumerate() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let action = parse_action_line(line)
                .map_err(|e| anyhow::anyhow!("Line {}: {}", line_num + 1, e))?;
            actions.push(serde_yaml::Value::Mapping(action));
        }
        if actions.is_empty() {
            return Err(anyhow::anyhow!("No actions provided via stdin"));
        }
    } else {
        let action_name = args.action.as_deref().unwrap_or("");
        let value = args.value.as_deref().unwrap_or("");
        let mut action = serde_yaml::Mapping::new();
        action.insert(
            serde_yaml::Value::String(action_name.to_string()),
            parse_exec_scalar(action_name, None, value)?,
        );

        let mut index = 0;
        while index < args.options.len() {
            let option = args.options[index]
                .strip_prefix("--")
                .ok_or_else(|| anyhow::anyhow!("Expected an option beginning with '--', got {:?}", args.options[index]))?;
            if option.is_empty() {
                return Err(anyhow::anyhow!("Expected an option name after '--'"));
            }

            let (field, field_value, consumed) = if let Some((field, val)) = option.split_once('=') {
                (field, val, 1)
            } else {
                let val = args.options.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!("Option '--{}' requires a value", option)
                })?;
                (option, val.as_str(), 2)
            };
            if field.is_empty() {
                return Err(anyhow::anyhow!("Expected an option name after '--'"));
            }

            let field = field.replace('-', "_");
            action.insert(
                serde_yaml::Value::String(field.clone()),
                parse_exec_scalar(action_name, Some(&field), field_value)?,
            );
            index += consumed;
        }
        actions.push(serde_yaml::Value::Mapping(action));
    }

    let mut flow = serde_yaml::Mapping::new();
    flow.insert(
        serde_yaml::Value::String("main".to_string()),
        serde_yaml::Value::Sequence(actions),
    );

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(flow))?;
    parse_str(&yaml).map_err(|e| anyhow::anyhow!("Invalid exec action: {}", e))
}

async fn exec_action(args: &ExecArgs) -> anyhow::Result<()> {
    let flow_file = parse_exec_action(args)?;
    let actions = flow_file
        .get("main")
        .ok_or_else(|| anyhow::anyhow!("No valid actions were provided"))?;
    
    let mut context = Context::from_env();

    run_actions(
        &get_browser_url(),
        actions,
        &mut context,
        false,
        &flow_file,
    )
    .await
    .map_err(|e| anyhow::anyhow!("Exec action failed: {}", e))?;

    println!("OK");
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
        Cli::Exec(ref args) => tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(exec_action(args)),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_exec_action, Cli, ExecArgs, RunArgs};
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn run_defaults_to_main_target_and_accepts_path_positionally() {
        let cli = Cli::try_parse_from(["bcl", "run", "script.yml"]).unwrap();

        match cli {
            Cli::Run(args) => {
                assert_eq!(args.path, PathBuf::from("script.yml"));
                assert_eq!(args.target, "main");
            }
            _ => panic!("Expected run command"),
        }
    }

    #[test]
    fn run_accepts_short_and_long_target_options() {
        for option in ["-t", "--target"] {
            let cli = Cli::try_parse_from(["bcl", "run", "script.yml", option, "login"]).unwrap();

            match cli {
                Cli::Run(RunArgs { target, .. }) => assert_eq!(target, "login"),
                _ => panic!("Expected run command"),
            }
        }
    }

    #[test]
    fn exec_accepts_options_as_separate_or_equals_arguments() {
        let cli = Cli::try_parse_from([
            "bcl",
            "exec",
            "read",
            "//body",
            "--html=true",
            "--to=BODY_HTML",
        ])
        .unwrap();

        match cli {
            Cli::Exec(args) => {
                let flow_file = parse_exec_action(&args).unwrap();
                let actions = flow_file.get("main").unwrap();
                assert_eq!(actions.len(), 1);
                match &actions[0] {
                    bcl::flow::Action::Read { target, html, to, .. } => {
                        assert_eq!(target.as_deref(), Some("//body"));
                        assert!(*html);
                        assert_eq!(to.as_deref(), Some("BODY_HTML"));
                    }
                    _ => panic!("Expected Read action"),
                }
            }
            _ => panic!("Expected exec command"),
        }

        let cli = Cli::try_parse_from([
            "bcl",
            "exec",
            "read",
            "//*[@id=\"link\"]",
            "--html",
            "true",
            "--to",
            "LINK_REF",
            "--attribute",
            "href",
        ])
        .unwrap();
        match cli {
            Cli::Exec(args) => {
                let flow_file = parse_exec_action(&args).unwrap();
                match &flow_file.get("main").unwrap()[0] {
                    bcl::flow::Action::Read {
                        target,
                        html,
                        to,
                        attribute,
                        ..
                    } => {
                        assert_eq!(target.as_deref(), Some("//*[@id=\"link\"]"));
                        assert!(*html);
                        assert_eq!(to.as_deref(), Some("LINK_REF"));
                        assert_eq!(attribute.as_deref(), Some("href"));
                    }
                    _ => panic!("Expected Read action"),
                }
            }
            _ => panic!("Expected exec command"),
        }
    }

    #[test]
    fn exec_accepts_action_without_options() {
        let cli = Cli::try_parse_from(["bcl", "exec", "goto", "https://example.com"]).unwrap();

        match cli {
            Cli::Exec(args) => {
                let flow_file = parse_exec_action(&args).unwrap();
                assert!(matches!(
                    &flow_file.get("main").unwrap()[0],
                    bcl::flow::Action::Goto { url, .. } if url == "https://example.com"
                ));
            }
            _ => panic!("Expected exec command"),
        }
    }

    #[test]
    fn exec_rejects_unknown_actions_and_missing_option_values() {
        let unknown = ExecArgs {
            action: "not_an_action".to_string(),
            value: "value".to_string(),
            options: vec![],
        };
        assert!(parse_exec_action(&unknown).is_err());

        let missing_option_value = ExecArgs {
            action: "read".to_string(),
            value: "//body".to_string(),
            options: vec!["--html".to_string()],
        };
        assert!(parse_exec_action(&missing_option_value).is_err());

        let chained = ExecArgs {
            action: "read".to_string(),
            value: "//body".to_string(),
            options: vec!["click".to_string(), "#submit".to_string()],
        };
        assert!(parse_exec_action(&chained).is_err());
    }
}
