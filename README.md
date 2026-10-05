# BCL - Browser Control Language

A Rust-based browser automation tool that executes flow definitions written in YAML.

## Quick Start

```bash
# Build
cargo build --release

# Run a flow
bcl run login assets/brokers/clear.yml

# Dry-run (no browser)
bcl run login assets/brokers/clear.yml --dry-run

# Validate a flow file
bcl validate assets/brokers/clear.yml

# List flows in a file
bcl list assets/brokers/clear.yml
```

## CLI Usage

```bash
bcl run <flow_name> <path_to_yaml> [options]
bcl validate <path_to_yaml>
bcl list <path_to_yaml>
```

### Run Options

- `-o, --output <format>` - Output format: `simple`, `pretty`, `json` (default: simple)
- `-v, --verbose` - Show debug info and full error traces
- `--var KEY=VALUE` - Override or set context variables (repeatable)
- `--dry-run` - Parse and resolve variables, print actions, do not connect to browser

### Environment Variables

- `BROWSER_URL` - Playwright browser URL (default: `http://127.0.0.1:9222`)

## Documentation

- [Flow Definition](docs/flow-definition.md)
- [Available Actions](docs/actions.md)
- [Selectors](docs/selectors.md)
- [Variables](docs/variables.md)
- [Conditionals](docs/conditionals.md)
- [Shadow DOM & Iframes](docs/shadow-dom-iframes.md)
- [Error Handling](docs/error-handling.md)

## Example Flow

```yaml
login:
  - goto: https://example.com/login
    wait_until: commit
  - type: $USERNAME
    xpath: //*[@id="username"]
  - type: $PASSWORD
    xpath: //*[@id="password"]
    secret: true
  - click: //*[@id="submit"]
  - wait:
      url: https://example.com/dashboard

main:
  - if:
      url: https://example.com/login
    then:
      - call: login
  - goto: https://example.com/home
```

## Architecture

- **flow/** - YAML parsing and flow definition types
- **executor/** - Action execution engine
- **runtime/** - Playwright connection and browser runtime
- **Context** - Variable and environment management
