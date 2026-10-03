# Available Actions

| Action | Description |
|--------|-------------|
| `goto` | Navigate to a URL |
| `type` | Type text into an element |
| `click` | Click an element |
| `press` | Press a keyboard key |
| `wait` | Wait for URL, element, or duration |
| `ask` | Prompt user for input |
| `hover` | Hover over an element |
| `if` | Conditional execution |
| `read` | Read element content |
| `log` | Log a message |
| `call` | Call another flow |
| `js` | Execute JavaScript |
| `define` | Define a variable |

## Action Details

### goto

Navigate to a URL with optional wait condition:

```yaml
- goto: https://example.com
  wait_until: commit
```

### type

Type text into an element:

```yaml
- type: $USERNAME
  xpath: //*[@id="username"]
  secret: true
```

### click

Click an element:

```yaml
- click: //*[@id="submit"]
```

### press

Press a keyboard key:

```yaml
- press: Enter
```

### wait

Wait for conditions:

```yaml
- wait:
    url: https://example.com/dashboard
# or
- wait:
    xpath: //*[@id="loaded"]
# or
- wait:
    duration: 5000
```

### ask

Prompt user for input:

```yaml
- ask: Please provide your token
  to: TOKEN
```

### hover

Hover over an element:

```yaml
- hover: //*[@id="menu"]
```

### read

Read element content:

```yaml
- read: //*[@id="result"]
  to: RESULT
  html: false
```

### log

Log a message:

```yaml
- log: User logged in successfully
```

### call

Call another flow:

```yaml
- call: login
```

### js

Execute JavaScript:

```yaml
- js: return document.title
  to: PAGE_TITLE
```

### define

Define a variable:

```yaml
- define:
    var: MY_VAR
    value: hello
    overwrite: false
```
