# Conditionals

Execute actions based on conditions:

```yaml
if:
  url: https://example.com/login
then:
  - click: #submit
else:
  - log: Already logged in
```

## Condition Types

| Condition | Description |
|-----------|-------------|
| `url` | Match current URL |
| `xpath` | Match XPath selector exists |
| `css` | Match CSS selector exists |
| `defined` | Variable is defined |
| `not` | Negate a condition |

## Examples

### URL Condition

```yaml
if:
  url: https://example.com/login
then:
  - call: login
```

### Existence Condition

```yaml
if:
  xpath: //*[@id="error-message"]
then:
  - log: Error detected
```

### Negation

```yaml
if:
  not:
    defined: TOKEN
then:
  - ask: Please provide token
    to: TOKEN
```
