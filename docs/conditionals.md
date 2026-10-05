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
| `xpath` | XPath selector exists |
| `css` | CSS selector exists |
| `defined` | Variable is defined |
| `not` | Negate a condition |
| `text` | Element contains text |
| `visible` | Element is visible |
| `checked` | Checkbox/radio is checked |
| `enabled` | Element is enabled |
| `equals` | Variable equals a value |
| `and` | All conditions true |
| `or` | Any condition true |

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

### Element Visible

```yaml
if:
  visible: "#loading-spinner"
then:
  - wait: 1000
```

### Element Contains Text

```yaml
if:
  text:
    target: "#message"
    contains: "Success"
then:
  - log: Operation succeeded
```

### Variable Equals

```yaml
if:
  equals:
    var: $STATUS
    value: ok
then:
  - log: All good
```

### Combining Conditions

```yaml
if:
  and:
    - visible: "#submit-btn"
    - enabled: "#submit-btn"
then:
  - click: "#submit-btn"
```

```yaml
if:
  or:
    - xpath: //button[@id='a']
    - css: "#fallback"
then:
  - click: //button
```
