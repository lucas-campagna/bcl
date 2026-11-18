use clap::Parser;
use bcl::{parse_file, parse_stdin, parse_str, Context, FlowRuntime, run_actions};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub struct HelpArgs {
    /// Command to show help for
    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,
    /// Show examples for the command
    #[arg(long)]
    pub examples: bool,
}

#[derive(Parser, Debug)]
#[command(name = "bcl")]
#[command(about = "Browser Control Language — execute YAML-based browser automation flows", long_about = None)]
#[command(disable_help_subcommand = true)]
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
    /// Show help for commands
    Help(HelpArgs),
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
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
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
    let structured = matches!(action, "headers") || matches!(field, Some("values" | "headers"));
    if structured {
        let trimmed = value.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                if let Ok(yaml) = serde_yaml::to_value(json) {
                    return Ok(yaml);
                }
            }
        }
    }

    let bool_field = matches!(
        (action, field),
        ("type", Some("secret"))
            | ("read", Some("html" | "value"))
            | ("define" | "set", Some("overwrite"))
            | ("tab", Some("close"))
            | ("screenshot", Some("full_page"))
            | ("reload", Some("ignore_cache"))
            | ("viewport", Some("mobile" | "reset"))
            | ("headers", Some("clear"))
    );
    if bool_field {
        let parsed = value.parse::<bool>().map_err(|_| {
            anyhow::anyhow!("Expected true or false for parameter '{}', got {:?}", field.unwrap(), value)
        })?;
        return Ok(serde_yaml::Value::Bool(parsed));
    }

    let integer_field = matches!(
        field,
        Some("timeout" | "duration" | "max" | "interval" | "port" | "width" | "height" | "quality")
    );
    let float_field = matches!(field, Some("device_scale_factor"));
    let signed_integer_field = matches!(field, Some("x" | "y"));
    let numeric_primary = field.is_none() && matches!(action, "tab" | "wait" | "retry");
    let define_value = matches!((action, field), ("define" | "set", Some("value")));

    if integer_field || numeric_primary {
        if let Ok(parsed) = value.parse::<u64>() {
            return Ok(serde_yaml::Value::Number(parsed.into()));
        }
    } else if float_field {
        if let Ok(parsed) = value.parse::<f64>() {
            return Ok(serde_yaml::Value::from(parsed));
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
    let value_arg = parts.get(1).map(|s| s.as_str()).unwrap_or("");
    let value = if value_arg.starts_with("--") { "" } else { value_arg };
    let option_start = if value_arg.starts_with("--") { 1 } else { 2 };
    let options: Vec<&str> = parts
        .get(option_start..)
        .unwrap_or(&[])
        .iter()
        .map(|s| s.as_str())
        .collect();

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
        let value_arg = args.value.as_deref().unwrap_or("");
        let value = if value_arg.starts_with("--") { "" } else { value_arg };
        let options: Vec<&str> = if value_arg.starts_with("--") {
            std::iter::once(value_arg)
                .chain(args.options.iter().map(String::as_str))
                .collect()
        } else {
            args.options.iter().map(String::as_str).collect()
        };
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
                let val = options
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("Option '--{}' requires a value", option))?;
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

fn print_command_summary() {
    println!("BCL Actions (use in YAML flows):");
    println!();
    println!("  goto        Navigate to a URL");
    println!("  click       Click an element");
    println!("  type        Type text into an input");
    println!("  press       Press a keyboard key");
    println!("  wait        Wait for element, URL, or duration");
    println!("  read        Read element text, HTML, attribute, or value");
    println!("  hover       Hover over an element");
    println!("  select      Select option(s) from a dropdown");
    println!("  check       Check a checkbox");
    println!("  uncheck     Uncheck a checkbox");
    println!("  dblclick    Double-click an element");
    println!("  right-click Right-click an element");
    println!("  clear       Clear an input field");
    println!("  upload      Upload file(s)");
    println!("  drag        Drag and drop");
    println!("  scroll      Scroll an element");
    println!("  dialog      Handle browser dialogs (accept/dismiss/prompt)");
    println!("  download    Wait for and save a download");
    println!("  log         Log a message");
    println!("  call        Call another flow");
    println!("  js          Execute JavaScript");
    println!("  define      Define a variable");
    println!("  if          Conditional execution");
    println!("  for         Loop over items");
    println!("  while       Loop while condition is true");
    println!("  retry       Retry actions on failure");
    println!("  assert      Assert a condition");
    println!("  return      Return values from a flow");
    println!("  fail        Fail with a message");
    println!("  save        Save context to file");
    println!("  load        Load context from file");
    println!("  request     Make an HTTP request");
    println!("  tab         Switch or close tabs");
    println!("  screenshot  Capture a screenshot to a file or variable");
    println!("  back        Navigate back in history");
    println!("  forward     Navigate forward in history");
    println!("  reload      Reload the current page");
    println!("  viewport    Set or reset the browser viewport");
    println!("  cookies     Get, set, delete, or clear cookies");
    println!("  headers     Set or clear extra HTTP headers");
    println!();
    println!("CLI Commands:");
    println!("  run         Run a BCL flow from a YAML file");
    println!("  validate    Validate a flow YAML file (syntax and structure)");
    println!("  list        List all flows defined in a YAML file");
    println!("  compile     Compile a YAML flow into a standalone shell script");
    println!("  exec        Execute a single BCL browser action");
    println!("  help        Show this help message");
    println!();
    println!("Run 'bcl help <action>' for detailed help on an action.");
    println!("Run 'bcl help <action> --examples' for usage examples.");
    println!("Run 'bcl help cli' for CLI command help.");
}

fn print_command_help(command: &str) {
    match command {
        "goto" => {
            println!("goto - Navigate to a URL");
            println!();
            println!("USAGE:");
            println!("    goto: <URL>");
            println!("    goto:");
            println!("      url: <URL>");
            println!("      wait_until: <load|domcontentloaded|networkidle>  # optional");
            println!("      tab: <index|new>                                 # optional");
            println!();
            println!("OPTIONS:");
            println!("    url            The URL to navigate to");
            println!("    wait_until     When to consider navigation complete (default: load)");
            println!("    tab            Target tab: index or 'new' for new tab");
        }
        "click" => {
            println!("click - Click an element");
            println!();
            println!("USAGE:");
            println!("    click: <SELECTOR>");
            println!("    click:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
            println!();
            println!("OPTIONS:");
            println!("    target       Element selector (XPath or CSS)");
            println!("    timeout      Max wait time in milliseconds");
            println!("    shadow_root  Shadow DOM path for nested elements");
            println!("    iframe       Iframe selector path");
        }
        "type" => {
            println!("type - Type text into an input");
            println!();
            println!("USAGE:");
            println!("    type: <TEXT>");
            println!("    type:");
            println!("      value: <TEXT>");
            println!("      target: <SELECTOR>     # optional");
            println!("      secret: true|false     # optional (default: false)");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
            println!();
            println!("OPTIONS:");
            println!("    value        Text to type (use secret: true for passwords)");
            println!("    target       Element selector (defaults to focused element)");
            println!("    secret       If true, value is treated as sensitive");
            println!("    timeout      Max wait time in milliseconds");
        }
        "press" => {
            println!("press - Press a keyboard key");
            println!();
            println!("USAGE:");
            println!("    press: <KEY>");
            println!();
            println!("KEY EXAMPLES:");
            println!("    Enter, Tab, Escape, ArrowUp, ArrowDown, ArrowLeft, ArrowRight");
            println!("    Control+A, Control+C, Control+V, Meta+Enter");
            println!("    F1-F12, Home, End, PageUp, PageDown, Delete, Backspace");
        }
        "wait" => {
            println!("wait - Wait for element, URL, or duration");
            println!();
            println!("USAGE:");
            println!("    wait: <MS>");
            println!("    wait:");
            println!("      duration: <MS>         # wait for N milliseconds");
            println!("      target: <SELECTOR>     # wait for element");
            println!("      url: <URL>             # wait for URL to match");
            println!("      timeout: <MS>          # optional, max wait time");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
            println!();
            println!("OPTIONS:");
            println!("    duration     Milliseconds to wait");
            println!("    target       Wait for element to be visible");
            println!("    url          Wait for URL to match pattern");
            println!("    timeout      Max wait time (default: 30000)");
        }
        "read" => {
            println!("read - Read element text, HTML, attribute, or value");
            println!();
            println!("USAGE:");
            println!("    read: <SELECTOR>");
            println!("    read:");
            println!("      target: <SELECTOR>");
            println!("      to: <VAR_NAME>         # store result in variable");
            println!("      html: true|false       # get outerHTML instead of text");
            println!("      attribute: <NAME>      # get attribute value");
            println!("      value: true|false      # get input value");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
            println!();
            println!("OPTIONS:");
            println!("    target       Element selector");
            println!("    to           Variable name to store result");
            println!("    html         If true, return outerHTML");
            println!("    attribute    Get specific attribute (e.g., href, src)");
            println!("    value        If true, return input value");
        }
        "hover" => {
            println!("hover - Hover over an element");
            println!();
            println!("USAGE:");
            println!("    hover: <SELECTOR>");
            println!("    hover:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "select" => {
            println!("select - Select option(s) from a dropdown");
            println!();
            println!("USAGE:");
            println!("    select: <VALUE>");
            println!("    select:");
            println!("      value: <VALUE>");
            println!("      target: <SELECTOR>     # optional");
            println!("      values: [<VAL1>, <VAL2>]  # multiple values");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
            println!();
            println!("OPTIONS:");
            println!("    value        Single value to select");
            println!("    values       Array of values for multi-select");
            println!("    target       Select element selector");
        }
        "check" => {
            println!("check - Check a checkbox or radio button");
            println!();
            println!("USAGE:");
            println!("    check: <SELECTOR>");
            println!("    check:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "uncheck" => {
            println!("uncheck - Uncheck a checkbox");
            println!();
            println!("USAGE:");
            println!("    uncheck: <SELECTOR>");
            println!("    uncheck:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "dblclick" => {
            println!("dblclick - Double-click an element");
            println!();
            println!("USAGE:");
            println!("    dblclick: <SELECTOR>");
            println!("    dblclick:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "right-click" | "right_click" => {
            println!("right-click - Right-click an element (context menu)");
            println!();
            println!("USAGE:");
            println!("    right-click: <SELECTOR>");
            println!("    right-click:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "clear" => {
            println!("clear - Clear an input field");
            println!();
            println!("USAGE:");
            println!("    clear: <SELECTOR>");
            println!("    clear:");
            println!("      target: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "upload" => {
            println!("upload - Upload file(s) to an input");
            println!();
            println!("USAGE:");
            println!("    upload: <FILE_PATH>");
            println!("    upload:");
            println!("      files: [<PATH1>, <PATH2>]  # single or multiple");
            println!("      target: <SELECTOR>         # optional");
            println!("      timeout: <MS>              # optional");
            println!("      shadow_root: <PATH>        # optional");
            println!("      iframe: <PATH>             # optional");
        }
        "drag" => {
            println!("drag - Drag and drop");
            println!();
            println!("USAGE:");
            println!("    drag:");
            println!("      source: <SELECTOR>");
            println!("      to: <SELECTOR>");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "scroll" => {
            println!("scroll - Scroll an element");
            println!();
            println!("USAGE:");
            println!("    scroll: <SELECTOR>");
            println!("    scroll:");
            println!("      target: <SELECTOR>");
            println!("      x: <PIXELS>            # optional");
            println!("      y: <PIXELS>            # optional");
            println!("      timeout: <MS>          # optional");
            println!("      shadow_root: <PATH>    # optional");
            println!("      iframe: <PATH>         # optional");
        }
        "dialog" => {
            println!("dialog - Handle browser dialogs");
            println!();
            println!("USAGE:");
            println!("    dialog: accept");
            println!("    dialog: dismiss");
            println!("    dialog:");
            println!("      mode: accept|dismiss|prompt");
            println!("      prompt_text: <TEXT>    # for prompt mode");
            println!("      timeout: <MS>          # optional");
        }
        "download" => {
            println!("download - Wait for and save a download");
            println!();
            println!("USAGE:");
            println!("    download:");
            println!("      path: <FILE_PATH>");
            println!("      timeout: <MS>          # optional");
            println!("      to: <VAR_NAME>         # optional, store path in variable");
        }
        "log" => {
            println!("log - Log a message");
            println!();
            println!("USAGE:");
            println!("    log: <MESSAGE>");
            println!("    log:");
            println!("      text: <MESSAGE>");
        }
        "call" => {
            println!("call - Call another flow");
            println!();
            println!("USAGE:");
            println!("    call: <FLOW_NAME>");
            println!("    call:");
            println!("      flow: <FLOW_NAME>");
            println!("      params:                # optional");
            println!("        key: value");
        }
        "js" => {
            println!("js - Execute JavaScript");
            println!();
            println!("USAGE:");
            println!("    js: <CODE>");
            println!("    js:");
            println!("      code: <CODE>");
            println!("      to: <VAR_NAME>         # optional, store result");
        }
        "define" => {
            println!("define - Define a variable");
            println!();
            println!("USAGE:");
            println!("    define: <VALUE>");
            println!("    define:");
            println!("      var: <VAR_NAME>");
            println!("      value: <VALUE>");
            println!("      overwrite: true|false  # optional (default: true)");
        }
        "if" => {
            println!("if - Conditional execution");
            println!();
            println!("USAGE:");
            println!("    if:");
            println!("      defined: <VAR>         # variable is defined");
            println!("      not_defined: <VAR>     # variable is not defined");
            println!("      equals:                # value equals");
            println!("        left: <VALUE>");
            println!("        right: <VALUE>");
            println!("      visible: <SELECTOR>    # element is visible");
            println!("      and: [<COND>, ...]     # all conditions true");
            println!("      or: [<COND>, ...]      # any condition true");
            println!("      then:                  # actions if true");
            println!("        - <ACTION>");
            println!("      else:                  # optional, actions if false");
            println!("        - <ACTION>");
        }
        "for" => {
            println!("for - Loop over items");
            println!();
            println!("USAGE:");
            println!("    for:");
            println!("      items: <LIST|VAR>      # list or variable containing list");
            println!("      as: <VAR_NAME>         # loop variable name");
            println!("      count: <N>             # optional, loop N times");
            println!("      do:                    # actions to repeat");
            println!("        - <ACTION>");
        }
        "while" => {
            println!("while - Loop while condition is true");
            println!();
            println!("USAGE:");
            println!("    while:");
            println!("      condition: <CONDITION>  # same as 'if' conditions");
            println!("      max: <N>                # optional, max iterations");
            println!("      do:                     # actions to repeat");
            println!("        - <ACTION>");
        }
        "retry" => {
            println!("retry - Retry actions on failure");
            println!();
            println!("USAGE:");
            println!("    retry:");
            println!("      attempts: <N>");
            println!("      interval: <MS>         # optional, delay between retries");
            println!("      do:                    # actions to retry");
            println!("        - <ACTION>");
        }
        "assert" => {
            println!("assert - Assert a condition, fail if false");
            println!();
            println!("USAGE:");
            println!("    assert:");
            println!("      condition: <CONDITION>  # same as 'if' conditions");
            println!("      message: <TEXT>         # optional, custom failure message");
        }
        "return" => {
            println!("return - Return values from a flow");
            println!();
            println!("USAGE:");
            println!("    return:");
            println!("      key: value");
        }
        "fail" => {
            println!("fail - Fail with a message");
            println!();
            println!("USAGE:");
            println!("    fail: <MESSAGE>");
            println!("    fail:");
            println!("      message: <MESSAGE>");
        }
        "save" => {
            println!("save - Save context to file");
            println!();
            println!("USAGE:");
            println!("    save: <FILE_PATH>");
            println!("    save:");
            println!("      path: <FILE_PATH>");
        }
        "load" => {
            println!("load - Load context from file");
            println!();
            println!("USAGE:");
            println!("    load: <FILE_PATH>");
            println!("    load:");
            println!("      path: <FILE_PATH>");
        }
        "request" => {
            println!("request - Make an HTTP request");
            println!();
            println!("USAGE:");
            println!("    request:");
            println!("      url: <URL>");
            println!("      method: <GET|POST|...> # optional (default: GET)");
            println!("      headers:               # optional");
            println!("        Key: Value");
            println!("      body: <STRING>         # optional");
            println!("      to: <VAR_NAME>         # optional, store response");
            println!("      timeout: <MS>          # optional");
        }
        "tab" => {
            println!("tab - Switch or close tabs");
            println!();
            println!("USAGE:");
            println!("    tab: <INDEX>");
            println!("    tab:");
            println!("      index: <N>             # switch to tab N");
            println!("      close: true|false      # close current tab");
        }
        "screenshot" => {
            println!("screenshot - Capture a screenshot");
            println!();
            println!("USAGE:");
            println!("    screenshot: <FILE_PATH>");
            println!("    screenshot:");
            println!("      path: <FILE_PATH>      # optional if 'to' is set");
            println!("      format: png|jpeg       # optional (default: png)");
            println!("      quality: <0-100>       # optional (jpeg only)");
            println!("      full_page: true|false  # optional (default: false)");
            println!("      to: <VAR_NAME>         # optional, store base64 data");
        }
        "back" | "forward" => {
            println!("{} - Navigate in browser history", command);
            println!();
            println!("USAGE:");
            println!("    {}:", command);
        }
        "reload" => {
            println!("reload - Reload the current page");
            println!();
            println!("USAGE:");
            println!("    reload:");
            println!("    reload:");
            println!("      ignore_cache: true|false  # optional (default: false)");
        }
        "viewport" => {
            println!("viewport - Set or reset the browser viewport");
            println!();
            println!("USAGE:");
            println!("    viewport:");
            println!("      width: <PIXELS>");
            println!("      height: <PIXELS>");
            println!("      device_scale_factor: <N>  # optional (default: 1)");
            println!("      mobile: true|false        # optional (default: false)");
            println!();
            println!("    viewport:");
            println!("      reset: true              # restore the default viewport");
        }
        "cookies" => {
            println!("cookies - Manage browser cookies");
            println!();
            println!("USAGE:");
            println!("    cookies: get|set|delete|clear");
            println!("    cookies:");
            println!("      action: get|set|delete|clear");
            println!("      name: <NAME>              # for set/delete");
            println!("      value: <VALUE>            # for set");
            println!("      url: <URL>                # optional");
            println!("      domain: <DOMAIN>          # optional");
            println!("      path: <PATH>              # optional");
            println!("      to: <VAR_NAME>            # optional, for get");
        }
        "headers" => {
            println!("headers - Set or clear extra HTTP headers");
            println!();
            println!("USAGE:");
            println!("    headers:");
            println!("      Header-Name: <VALUE>");
            println!("    headers: clear");
            println!();
            println!("In bcl exec, pass a JSON object as the value:");
            println!("    bcl exec headers '{{\"X-Key\": \"value\"}}'");
        }
        "cli" => {
            println!("CLI Commands:");
            println!();
            println!("  run       Run a BCL flow from a YAML file");
            println!("  validate  Validate a flow YAML file (syntax and structure)");
            println!("  list      List all flows defined in a YAML file");
            println!("  compile   Compile a YAML flow into a standalone shell script");
            println!("  exec      Execute a single BCL browser action");
            println!("  help      Show this help message");
            println!();
            println!("Run 'bcl help <command>' for detailed help on a CLI command.");
            println!("Run 'bcl help <command> --examples' for usage examples.");
        }
        "run" | "validate" | "list" | "compile" | "exec" | "help" => {
            // Show CLI command help for these commands
            print_cli_command_help(command);
        }
        _ => {
            eprintln!("Unknown action: {}", command);
            eprintln!("Run 'bcl help' for a list of available actions.");
            std::process::exit(1);
        }
    }
}

fn print_cli_command_help(command: &str) {
    match command {
        "run" => {
            println!("bcl run - Run a BCL flow");
            println!();
            println!("USAGE:");
            println!("    bcl run <PATH> [OPTIONS]");
            println!();
            println!("ARGUMENTS:");
            println!("    <PATH>    Path to the YAML file containing the flow definition (use - for stdin)");
            println!();
            println!("OPTIONS:");
            println!("    -t, --target <TARGET>    Target flow to execute (default: main)");
            println!("    -o, --output <FORMAT>    Output format: simple, pretty, json (default: simple)");
            println!("    -v, --verbose            Verbose mode — show debug info and full error traces");
            println!("    --var <KEY=VALUE>        Override or set context variables (repeatable)");
            println!("    --dry-run                Parse and resolve variables, print actions, do not connect to browser");
            println!("    -h, --help               Print help");
        }
        "validate" => {
            println!("bcl validate - Validate a flow YAML file");
            println!();
            println!("USAGE:");
            println!("    bcl validate <PATH>");
            println!();
            println!("ARGUMENTS:");
            println!("    <PATH>    Path to the YAML file to validate (use - for stdin)");
            println!();
            println!("OPTIONS:");
            println!("    -h, --help    Print help");
        }
        "list" => {
            println!("bcl list - List all flows in a YAML file");
            println!();
            println!("USAGE:");
            println!("    bcl list <PATH>");
            println!();
            println!("ARGUMENTS:");
            println!("    <PATH>    Path to the YAML file (use - for stdin)");
            println!();
            println!("OPTIONS:");
            println!("    -h, --help    Print help");
        }
        "compile" => {
            println!("bcl compile - Compile a YAML flow into a standalone shell script");
            println!();
            println!("USAGE:");
            println!("    bcl compile <YAML_PATH> [OPTIONS]");
            println!();
            println!("ARGUMENTS:");
            println!("    <YAML_PATH>    Path to the YAML file to compile");
            println!();
            println!("OPTIONS:");
            println!("    -n, --name <NAME>    Optional name for the output file (without extension)");
            println!("    -h, --help         Print help");
        }
        "exec" => {
            println!("bcl exec - Execute a single BCL browser action");
            println!();
            println!("USAGE:");
            println!("    bcl exec <ACTION> [VALUE] [OPTIONS]");
            println!("    bcl exec --stdin");
            println!();
            println!("ARGUMENTS:");
            println!("    <ACTION>    BCL action to execute (e.g., goto, click, read, type, wait)");
            println!("    <VALUE>     The action's primary value (e.g., URL for goto, selector for click)");
            println!();
            println!("OPTIONS:");
            println!("    --<OPTION> <VALUE>    Action-specific options (e.g., --html true, --to VAR_NAME)");
            println!("    -s, --stdin           Read multiple actions from stdin, one per line");
            println!("    -h, --help            Print help");
            println!();
            println!("EXAMPLES:");
            println!("    bcl exec goto https://example.com");
            println!("    bcl exec click \"//button[@id='submit']\"");
            println!("    bcl exec read \"//body\" --html true --to BODY_HTML");
            println!("    bcl exec type \"//input[@name='q']\" \"search term\"");
            println!("    bcl exec wait 2000");
        }
        "help" => {
            println!("bcl help - Show help for commands");
            println!();
            println!("USAGE:");
            println!("    bcl help [COMMAND] [--examples]");
            println!();
            println!("ARGUMENTS:");
            println!("    [COMMAND]    Command to show help for (run, validate, list, compile, exec, help)");
            println!();
            println!("OPTIONS:");
            println!("    --examples    Show usage examples for the command");
            println!("    -h, --help    Print help");
        }
        _ => {
            eprintln!("Unknown CLI command: {}", command);
            eprintln!("Run 'bcl help cli' for a list of available commands.");
            std::process::exit(1);
        }
    }
}

fn print_cli_command_examples(command: &str) {
    match command {
        "run" => {
            println!("bcl run - Examples");
            println!();
            println!("Run the default 'main' flow:");
            println!("    bcl run script.yml");
            println!();
            println!("Run a specific flow target:");
            println!("    bcl run script.yml -t login");
            println!();
            println!("Run with verbose output:");
            println!("    bcl run script.yml -v");
            println!();
            println!("Run with JSON output:");
            println!("    bcl run script.yml -o json");
            println!();
            println!("Run with custom variables:");
            println!("    bcl run script.yml --var USERNAME=admin --var PASSWORD=secret");
            println!();
            println!("Dry run (no browser connection):");
            println!("    bcl run script.yml --dry-run");
            println!();
            println!("Read from stdin:");
            println!("    cat script.yml | bcl run -");
        }
        "validate" => {
            println!("bcl validate - Examples");
            println!();
            println!("Validate a YAML file:");
            println!("    bcl validate script.yml");
            println!();
            println!("Validate from stdin:");
            println!("    cat script.yml | bcl validate -");
        }
        "list" => {
            println!("bcl list - Examples");
            println!();
            println!("List flows in a file:");
            println!("    bcl list script.yml");
            println!();
            println!("List flows from stdin:");
            println!("    cat script.yml | bcl list -");
        }
        "compile" => {
            println!("bcl compile - Examples");
            println!();
            println!("Compile to default output name:");
            println!("    bcl compile script.yml");
            println!();
            println!("Compile with custom output name:");
            println!("    bcl compile script.yml -n my-script");
            println!();
            println!("Run the generated script:");
            println!("    ./my-script login");
            println!("    ./my-script login --verbose");
        }
        "exec" => {
            println!("bcl exec - Examples");
            println!();
            println!("Navigate to a URL:");
            println!("    bcl exec goto https://example.com");
            println!();
            println!("Click an element:");
            println!("    bcl exec click \"//button[@id='submit']\"");
            println!();
            println!("Read element text/HTML:");
            println!("    bcl exec read \"//body\" --html true --to BODY_HTML");
            println!("    bcl exec read \"//input[@name='q']\" --value true --to INPUT_VALUE");
            println!();
            println!("Type into an input:");
            println!("    bcl exec type \"//input[@name='q']\" \"search term\"");
            println!();
            println!("Wait:");
            println!("    bcl exec wait 2000");
            println!();
            println!("Multiple actions from stdin:");
            println!("    echo 'goto https://example.com' | bcl exec --stdin");
            println!("    echo 'click \"//a[@href]\"' | bcl exec --stdin");
        }
        "help" => {
            println!("bcl help - Examples");
            println!();
            println!("Show command summary:");
            println!("    bcl help");
            println!();
            println!("Show detailed help for 'run':");
            println!("    bcl help run");
            println!();
            println!("Show examples for 'exec':");
            println!("    bcl help exec --examples");
        }
        _ => {
            eprintln!("Unknown CLI command: {}", command);
            eprintln!("Run 'bcl help cli' for a list of available commands.");
            std::process::exit(1);
        }
    }
}

fn print_command_examples(command: &str) {
    match command {
        "goto" => {
            println!("goto - Examples");
            println!();
            println!("Simple navigation:");
            println!("    goto: https://example.com");
            println!();
            println!("With options:");
            println!("    goto:");
            println!("      url: https://example.com");
            println!("      wait_until: networkidle");
        }
        "click" => {
            println!("click - Examples");
            println!();
            println!("Click by XPath:");
            println!("    click: \"//button[@id='submit']\"");
            println!();
            println!("Click with options:");
            println!("    click:");
            println!("      target: \"//a[@class='link']\"");
            println!("      timeout: 5000");
        }
        "type" => {
            println!("type - Examples");
            println!();
            println!("Type into focused element:");
            println!("    type: \"search query\"");
            println!();
            println!("Type into specific element:");
            println!("    type:");
            println!("      target: \"//input[@name='q']\"");
            println!("      value: \"search term\"");
            println!();
            println!("Type password (hidden in logs):");
            println!("    type:");
            println!("      target: \"//input[@type='password']\"");
            println!("      value: \"secret123\"");
            println!("      secret: true");
        }
        "press" => {
            println!("press - Examples");
            println!();
            println!("Press Enter:");
            println!("    press: Enter");
            println!();
            println!("Press Ctrl+A (select all):");
            println!("    press: Control+A");
            println!();
            println!("Press Escape:");
            println!("    press: Escape");
        }
        "wait" => {
            println!("wait - Examples");
            println!();
            println!("Wait 2 seconds:");
            println!("    wait: 2000");
            println!();
            println!("Wait for element:");
            println!("    wait:");
            println!("      target: \"//div[@id='content']\"");
            println!();
            println!("Wait for URL to contain 'success':");
            println!("    wait:");
            println!("      url: \"*success*\"");
            println!("      timeout: 10000");
        }
        "read" => {
            println!("read - Examples");
            println!();
            println!("Read element text:");
            println!("    read: \"//h1\"");
            println!("    # result in LAST_READ variable");
            println!();
            println!("Read HTML:");
            println!("    read:");
            println!("      target: \"//div[@id='content']\"");
            println!("      html: true");
            println!("      to: PAGE_HTML");
            println!();
            println!("Read attribute:");
            println!("    read:");
            println!("      target: \"//a[@class='link']\"");
            println!("      attribute: href");
            println!("      to: LINK_URL");
            println!();
            println!("Read input value:");
            println!("    read:");
            println!("      target: \"//input[@name='email']\"");
            println!("      value: true");
            println!("      to: EMAIL_VALUE");
        }
        "hover" => {
            println!("hover - Examples");
            println!();
            println!("Hover over menu:");
            println!("    hover: \"//nav//a[@class='dropdown']\"");
        }
        "select" => {
            println!("select - Examples");
            println!();
            println!("Select single option:");
            println!("    select:");
            println!("      target: \"//select[@name='country']\"");
            println!("      value: \"US\"");
            println!();
            println!("Multi-select:");
            println!("    select:");
            println!("      target: \"//select[@multiple]\"");
            println!("      values: [\"US\", \"CA\", \"MX\"]");
        }
        "check" => {
            println!("check - Examples");
            println!();
            println!("Check checkbox:");
            println!("    check: \"//input[@id='terms']\"");
        }
        "uncheck" => {
            println!("uncheck - Examples");
            println!();
            println!("Uncheck checkbox:");
            println!("    uncheck: \"//input[@id='newsletter']\"");
        }
        "dblclick" => {
            println!("dblclick - Examples");
            println!();
            println!("Double-click to edit:");
            println!("    dblclick: \"//td[@class='editable']\"");
        }
        "right-click" | "right_click" => {
            println!("right-click - Examples");
            println!();
            println!("Right-click for context menu:");
            println!("    right-click: \"//img[@id='photo']\"");
        }
        "clear" => {
            println!("clear - Examples");
            println!();
            println!("Clear input field:");
            println!("    clear: \"//input[@name='search']\"");
        }
        "upload" => {
            println!("upload - Examples");
            println!();
            println!("Upload single file:");
            println!("    upload:");
            println!("      target: \"//input[@type='file']\"");
            println!("      files: \"/path/to/file.pdf\"");
            println!();
            println!("Upload multiple files:");
            println!("    upload:");
            println!("      target: \"//input[@type='file'][@multiple]\"");
            println!("      files: [\"/path/1.jpg\", \"/path/2.jpg\"]");
        }
        "drag" => {
            println!("drag - Examples");
            println!();
            println!("Drag and drop:");
            println!("    drag:");
            println!("      source: \"//div[@id='item1']\"");
            println!("      to: \"//div[@id='dropzone']\"");
        }
        "scroll" => {
            println!("scroll - Examples");
            println!();
            println!("Scroll to bottom:");
            println!("    scroll:");
            println!("      target: \"//div[@class='scrollable']\"");
            println!("      y: 10000");
            println!();
            println!("Scroll to specific position:");
            println!("    scroll:");
            println!("      target: \"//div[@id='panel']\"");
            println!("      x: 500");
            println!("      y: 300");
        }
        "dialog" => {
            println!("dialog - Examples");
            println!();
            println!("Accept alert/confirm:");
            println!("    dialog: accept");
            println!();
            println!("Dismiss confirm:");
            println!("    dialog: dismiss");
            println!();
            println!("Respond to prompt:");
            println!("    dialog:");
            println!("      mode: prompt");
            println!("      prompt_text: \"My response\"");
        }
        "download" => {
            println!("download - Examples");
            println!();
            println!("Download file:");
            println!("    download:");
            println!("      path: \"/downloads/file.pdf\"");
            println!("      to: DOWNLOAD_PATH");
        }
        "log" => {
            println!("log - Examples");
            println!();
            println!("Log message:");
            println!("    log: \"Starting login flow\"");
        }
        "call" => {
            println!("call - Examples");
            println!();
            println!("Call another flow:");
            println!("    call: login");
            println!();
            println!("Call with parameters:");
            println!("    call:");
            println!("      flow: login");
            println!("      params:");
            println!("        username: \"admin\"");
            println!("        password: \"secret\"");
        }
        "js" => {
            println!("js - Examples");
            println!();
            println!("Execute JS and store result:");
            println!("    js:");
            println!("      code: \"document.title\"");
            println!("      to: PAGE_TITLE");
            println!();
            println!("Execute JS with arguments:");
            println!("    js:");
            println!("      code: |");
            println!("        (selector) => document.querySelector(selector).click()");
            println!("      # Note: use with 'call' to pass arguments");
        }
        "define" => {
            println!("define - Examples");
            println!();
            println!("Define variable:");
            println!("    define:");
            println!("      var: BASE_URL");
            println!("      value: \"https://api.example.com\"");
            println!();
            println!("Define with overwrite control:");
            println!("    define:");
            println!("      var: COUNTER");
            println!("      value: 0");
            println!("      overwrite: false");
        }
        "if" => {
            println!("if - Examples");
            println!();
            println!("Check if variable defined:");
            println!("    if:");
            println!("      defined: USER_TOKEN");
            println!("      then:");
            println!("        - log: \"Already logged in\"");
            println!("      else:");
            println!("        - call: login");
            println!();
            println!("Check element visibility:");
            println!("    if:");
            println!("      visible: \"//div[@id='modal']\"");
            println!("      then:");
            println!("        - click: \"//button[@class='close']\"");
        }
        "for" => {
            println!("for - Examples");
            println!();
            println!("Loop over list:");
            println!("    define:");
            println!("      var: urls");
            println!("      value: [\"a.com\", \"b.com\", \"c.com\"]");
            println!("    for:");
            println!("      items: urls");
            println!("      as: url");
            println!("      do:");
            println!("        - goto: url");
            println!("        - wait: 1000");
            println!();
            println!("Loop N times:");
            println!("    for:");
            println!("      count: 3");
            println!("      as: i");
            println!("      do:");
            println!("        - click: \"//button[@class='load-more']\"");
            println!("        - wait: 2000");
        }
        "while" => {
            println!("while - Examples");
            println!();
            println!("Loop while element exists:");
            println!("    while:");
            println!("      condition:");
            println!("        visible: \"//button[@class='load-more']\"");
            println!("      max: 10");
            println!("      do:");
            println!("        - click: \"//button[@class='load-more']\"");
            println!("        - wait: 2000");
        }
        "retry" => {
            println!("retry - Examples");
            println!();
            println!("Retry click up to 3 times:");
            println!("    retry:");
            println!("      attempts: 3");
            println!("      interval: 1000");
            println!("      do:");
            println!("        - click: \"//button[@id='flaky']\"");
        }
        "assert" => {
            println!("assert - Examples");
            println!();
            println!("Assert element visible:");
            println!("    assert:");
            println!("      condition:");
            println!("        visible: \"//div[@class='success']\"");
            println!("      message: \"Login failed - success message not shown\"");
        }
        "return" => {
            println!("return - Examples");
            println!();
            println!("Return values from flow:");
            println!("    return:");
            println!("      user_id: 123");
            println!("      token: \"abc123\"");
        }
        "fail" => {
            println!("fail - Examples");
            println!();
            println!("Fail with custom message:");
            println!("    fail: \"Critical element not found\"");
        }
        "save" => {
            println!("save - Examples");
            println!();
            println!("Save session to file:");
            println!("    save: \"session.yml\"");
        }
        "load" => {
            println!("load - Examples");
            println!();
            println!("Load session from file:");
            println!("    load: \"session.yml\"");
        }
        "request" => {
            println!("request - Examples");
            println!();
            println!("GET request:");
            println!("    request:");
            println!("      url: \"https://api.example.com/users\"");
            println!("      to: API_RESPONSE");
            println!();
            println!("POST request with JSON body:");
            println!("    request:");
            println!("      url: \"https://api.example.com/login\"");
            println!("      method: POST");
            println!("      headers:");
            println!("        Content-Type: application/json");
            println!("      body: '{{\"username\":\"admin\",\"password\":\"secret\"}}'");
            println!("      to: LOGIN_RESPONSE");
        }
        "tab" => {
            println!("tab - Examples");
            println!();
            println!("Switch to tab 1 (0-indexed):");
            println!("    tab: 1");
            println!();
            println!("Close current tab:");
            println!("    tab:");
            println!("      close: true");
        }
        "screenshot" => {
            println!("screenshot - Examples");
            println!();
            println!("Save a viewport screenshot:");
            println!("    screenshot: /tmp/page.png");
            println!();
            println!("Full-page screenshot stored in a variable:");
            println!("    screenshot:");
            println!("      full_page: true");
            println!("      to: SHOT_B64");
            println!();
            println!("CLI:");
            println!("    bcl exec screenshot /tmp/page.png");
            println!("    bcl exec screenshot --full_page true --to SHOT_B64");
        }
        "back" | "forward" => {
            println!("{} - Examples", command);
            println!();
            println!("    {}:", command);
            println!();
            println!("CLI:");
            println!("    bcl exec {}", command);
        }
        "reload" => {
            println!("reload - Examples");
            println!();
            println!("    reload:");
            println!();
            println!("Bypass the cache:");
            println!("    reload:");
            println!("      ignore_cache: true");
            println!();
            println!("CLI:");
            println!("    bcl exec reload --ignore_cache true");
        }
        "viewport" => {
            println!("viewport - Examples");
            println!();
            println!("Set viewport size:");
            println!("    viewport:");
            println!("      width: 1280");
            println!("      height: 720");
            println!();
            println!("CLI:");
            println!("    bcl exec viewport 1280 --height 720");
        }
        "cookies" => {
            println!("cookies - Examples");
            println!();
            println!("Read cookies into a variable:");
            println!("    cookies: get");
            println!("      to: COOKIES");
            println!();
            println!("Set a cookie:");
            println!("    cookies:");
            println!("      action: set");
            println!("      name: session");
            println!("      value: abc123");
            println!("      url: https://example.com");
            println!();
            println!("CLI:");
            println!("    bcl exec cookies set --name session --value abc123 --url https://example.com");
        }
        "headers" => {
            println!("headers - Examples");
            println!();
            println!("Add a header:");
            println!("    headers:");
            println!("      X-API-Key: secret");
            println!();
            println!("Clear all extra headers:");
            println!("    headers: clear");
            println!();
            println!("CLI:");
            println!("    bcl exec headers '{{\"X-API-Key\":\"secret\"}}'");
        }
        "cli" => {
            println!("CLI Commands - Examples");
            println!();
            println!("Run a flow:");
            println!("    bcl run script.yml");
            println!();
            println!("Run specific target:");
            println!("    bcl run script.yml -t login");
            println!();
            println!("Validate YAML:");
            println!("    bcl validate script.yml");
            println!();
            println!("List flows:");
            println!("    bcl list script.yml");
            println!();
            println!("Compile to script:");
            println!("    bcl compile script.yml");
            println!();
            println!("Execute single action:");
            println!("    bcl exec goto https://example.com");
        }
        "run" | "validate" | "list" | "compile" | "exec" | "help" => {
            // Show CLI command examples for these commands
            print_cli_command_examples(command);
        }
        _ => {
            eprintln!("Unknown action: {}", command);
            eprintln!("Run 'bcl help' for a list of available actions.");
            std::process::exit(1);
        }
    }
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
        Cli::Help(ref help_args) => {
            if let Some(ref command) = help_args.command {
                if help_args.examples {
                    print_command_examples(command);
                } else {
                    print_command_help(command);
                }
            } else {
                print_command_summary();
            }
            Ok(())
        }
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
    fn exec_accepts_options_without_primary_value() {
        let cli = Cli::try_parse_from([
            "bcl",
            "exec",
            "screenshot",
            "--full_page",
            "true",
            "--to",
            "SHOT_B64",
        ])
        .unwrap();

        match cli {
            Cli::Exec(args) => {
                let flow_file = parse_exec_action(&args).unwrap();
                match &flow_file.get("main").unwrap()[0] {
                    bcl::flow::Action::Screenshot { path, to, full_page, .. } => {
                        assert!(path.is_none());
                        assert_eq!(to.as_deref(), Some("SHOT_B64"));
                        assert!(*full_page);
                    }
                    _ => panic!("Expected Screenshot action"),
                }
            }
            _ => panic!("Expected exec command"),
        }
    }

    #[test]
    fn exec_rejects_unknown_actions_and_missing_option_values() {
        let unknown = ExecArgs {
            action: Some("not_an_action".to_string()),
            value: Some("value".to_string()),
            options: vec![],
            from_stdin: false,
        };
        assert!(parse_exec_action(&unknown).is_err());

        let missing_option_value = ExecArgs {
            action: Some("read".to_string()),
            value: Some("//body".to_string()),
            options: vec!["--html".to_string()],
            from_stdin: false,
        };
        assert!(parse_exec_action(&missing_option_value).is_err());

        let chained = ExecArgs {
            action: Some("read".to_string()),
            value: Some("//body".to_string()),
            options: vec!["click".to_string(), "#submit".to_string()],
            from_stdin: false,
        };
        assert!(parse_exec_action(&chained).is_err());
    }
}