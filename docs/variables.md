# Variables

Use `$VAR_NAME` syntax for variable substitution.

## Variable Sources

Variables can come from:
- Environment variables
- `define` action
- `ask` action
- `read` action (with `to` parameter)

## Usage

```yaml
- define:
    var: USERNAME
    value: john
- type: $USERNAME
  xpath: //*[@id="input"]
```

## Accessing Properties

Access nested YAML/object properties with dot notation:

```yaml
- define:
    var: CONFIG
    value:
      user:
        name: john
- log: $CONFIG.user.name
```

## Array Index

Access array elements with index notation:

```yaml
- define:
    var: ITEMS
    value: [a, b, c]
- log: $ITEMS[0]
```

## Built-in Variables

- `$url` - Current page URL
