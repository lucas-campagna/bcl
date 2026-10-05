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
| `select` | Select an option from a dropdown |
| `check` | Check a checkbox or radio button |
| `uncheck` | Uncheck a checkbox |
| `dblclick` | Double-click an element |
| `right_click` | Right-click an element |
| `clear` | Clear an input field |
| `upload` | Upload files to a file input |
| `drag` | Drag an element to a target |
| `scroll` | Scroll an element into view |
| `if` | Conditional execution |
| `read` | Read element content, attribute, or value |
| `log` | Log a message |
| `call` | Call another flow |
| `js` | Execute JavaScript |
| `define` | Define a variable |
| `for` | Iterate over a list or repeat N times |
| `while` | Loop while a condition is true |
| `retry` | Retry a block on failure |
| `assert` | Assert a condition is true |
| `return` | Return values from a called flow |
| `fail` | Explicitly fail the flow |
| `save` | Persist context to a JSON file |
| `load` | Load context from a JSON file |
| `dialog` | Auto-handle browser dialogs |
| `download` | Wait for and save a file download |
| `dialog` | Auto-handle browser dialogs |
| `download` | Wait for and save a file download |

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

### select

Select an option from a `<select>` dropdown:

```yaml
- select: "Option Label"
  target: //*[@id="country"]
# multi-select with values list:
- select: a
  target: //select[@id="lang"]
  values:
    - en
    - pt
```

### check

Check a checkbox or radio button:

```yaml
- check: //*[@id="terms"]
```

### uncheck

Uncheck a checkbox:

```yaml
- uncheck: //*[@id="newsletter"]
```

### dblclick

Double-click an element:

```yaml
- dblclick: //*[@id="edit-btn"]
```

### right_click

Right-click (context menu) an element:

```yaml
- right_click: //*[@id="row"]
```

### clear

Clear an input field:

```yaml
- clear: //*[@id="search"]
```

### upload

Upload files to a file input element:

```yaml
- upload: "/path/to/file.pdf"
  target: //*[@type="file"]
# multiple files:
- upload:
    - "/path/a.pdf"
    - "/path/b.png"
  target: //*[@type="file"]
```

### drag

Drag an element and drop it onto a target:

```yaml
- drag: "#source"
  to: "#dropzone"
```

### scroll

Scroll an element into view (and optionally scroll by wheel):

```yaml
- scroll: //*[@id="section"]
# with wheel scroll delta:
- scroll: //*[@id="section"]
  x: 0
  y: 200
```

### read

Read element text, HTML, an attribute, or an input value:

```yaml
# read text content (default)
- read: //*[@id="result"]
  to: RESULT

# read inner HTML
- read: //*[@id="content"]
  to: HTML_CONTENT
  html: true

# read an element attribute
- read: //*[@id="link"]
  to: LINK_HREF
  attribute: href

# read an input field's current value
- read: //*[@id="field"]
  to: FIELD_VALUE
  value: true
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

### dialog

Auto-handle browser dialogs (alerts, confirmations, prompts, beforeunload). Place the `dialog` action before the action that triggers the dialog — it spawns a background handler that will accept or dismiss the next dialog that appears:

```yaml
- dialog: accept
# with prompt text for alert() / prompt():
- dialog: accept
  prompt_text: "confirmed"
  timeout: 5000
# or dismiss:
- dialog: dismiss
```

### download

Wait for a file download and save it to a path:

```yaml
- download: ./report.pdf
  timeout: 30000
  to: SAVED_PATH
```

The `to` field is optional — if provided, the save path is stored in the named variable.

### for

Iterate over a list or repeat a number of times:

```yaml
# iterate over a list
- for: ["a", "b", "c"]
  as: ITEM
  do:
    - click: "#item-$ITEM"

# repeat N times
- for: 3
  as: I
  do:
    - log: $I
```

### while

Loop while a condition is true:

```yaml
- while:
    condition:
      xpath: //button[@id='more']
  do:
    - click: //button[@id='more']
  max: 50
```

### retry

Retry a block of actions on failure:

```yaml
- retry: 3
  interval: 1000
  do:
    - click: "#flaky-button"
```

### assert

Assert that a condition is true, failing the flow otherwise:

```yaml
- assert:
    condition:
      xpath: //*[@id='success']
  message: "Login did not succeed"
```

### return

Return values from a called flow (callers receive them via `$__return.KEY`):

```yaml
- return:
    token: $TOKEN
    status: ok
```

### fail

Explicitly fail the flow with a message:

```yaml
- fail: Something went wrong
```

### save / load

Persist the entire context to a JSON file, or load it back:

```yaml
- save: ./state.json
# later, in another flow:
- load: ./state.json
```
