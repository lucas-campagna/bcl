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
| `screenshot` | Capture a screenshot to a file or variable |
| `back` | Navigate back in browser history |
| `forward` | Navigate forward in browser history |
| `reload` | Reload the current page |
| `viewport` | Set or reset the browser viewport |
| `cookies` | Get, set, delete, or clear cookies |
| `headers` | Set or clear extra HTTP headers |
| `request` | Make HTTP requests |

## Action Details

### goto

Navigate to a URL with optional wait condition:

```yaml
- goto: https://example.com
  wait_until: commit
```

### type

Type text into an element. The `target` field accepts CSS or XPath selectors (automatically detected):

```yaml
- type: $USERNAME
  target: #username
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
- wait: https://example.com/dashboard
# or
- wait: //*[@id="loaded"]
# or
- wait: 5000
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

Execute JavaScript. The value of the final expression is the result; a top-level `return` is not needed. Use `to` to store the result in a variable:

```yaml
- js: document.title
  to: PAGE_TITLE

# Multi-line scripts also return their final expression's value
- js: |
    const sum = (a, b) => a + b
    sum(1, sum(2, 3))
  to: RESULT
```

### define

Define a variable:

```yaml
- define: MY_VAR
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

### screenshot

Capture a screenshot. Use `path` to write a file, `to` to store the base64 payload in a variable, or both:

```yaml
# save a viewport screenshot
- screenshot: ./page.png

# capture the full page and store the base64 data
- screenshot:
    full_page: true
    to: SCREENSHOT_B64

# JPEG with quality (PNG is the default)
- screenshot: ./page.jpg
  format: jpeg
  quality: 80
```

**Fields:**
- `path` (or the action value) — output file path; optional when `to` is set
- `to` — variable name for the base64 screenshot data
- `format` — `png` (default) or `jpeg`
- `quality` — JPEG quality from 0–100
- `full_page` — capture the full scrollable page instead of just the viewport

CLI examples:

```bash
bcl exec screenshot ./page.png
bcl exec screenshot --full_page true --to SCREENSHOT_B64
```

### back / forward

Move through the browser's navigation history:

```yaml
- back:
- forward:
```

CLI examples:

```bash
bcl exec back
bcl exec forward
```

### reload

Reload the current page. Set `ignore_cache` to bypass the browser cache:

```yaml
- reload:
- reload:
    ignore_cache: true
```

CLI example:

```bash
bcl exec reload --ignore_cache true
```

### viewport

Set the browser viewport, or restore the default with `reset: true`:

```yaml
- viewport:
    width: 1280
    height: 720
    device_scale_factor: 2
    mobile: false

- viewport:
    reset: true
```

**Fields:**
- `width` / `height` — viewport dimensions in pixels; required unless `reset` is true
- `device_scale_factor` — device pixel ratio (default: 1)
- `mobile` — enable mobile emulation (default: false)
- `reset` — clear the viewport override and restore the default

CLI example:

```bash
bcl exec viewport 1280 --height 720 --device_scale_factor 2
```

### cookies

Get, set, delete, or clear cookies. `get` stores a JSON value in the variable named by `to`:

```yaml
# read cookies for the current page URL
- cookies: get
  to: COOKIES

# set a cookie
- cookies:
    action: set
    name: session
    value: abc123
    url: https://example.com

# delete one cookie
- cookies:
    action: delete
    name: session
    url: https://example.com

# clear all browser cookies
- cookies: clear
```

**Fields:**
- `action` (or the action value) — `get`, `set`, `delete`, or `clear` (default: `get`)
- `name` / `value` — cookie name and value; `name` is required for `set` and `delete`
- `url` — restrict the operation to a URL; when omitted for `get`, all cookies are returned
- `domain` / `path` — optional cookie scope
- `to` — variable name for cookies returned by `get`

Aliases `list`, `add`, `remove`, and `clear_all` are also accepted.

CLI examples:

```bash
bcl exec cookies get --to COOKIES
bcl exec cookies set --name session --value abc123 --url https://example.com
bcl exec cookies clear
```

### headers

Set extra HTTP headers for browser requests, or clear them. A YAML mapping sets headers directly; `clear` removes all extra headers:

```yaml
- headers:
    X-API-Key: secret
    Accept: application/json

- headers: clear
```

In `bcl exec`, pass a JSON object as the action value:

```bash
bcl exec headers '{"X-API-Key":"secret","Accept":"application/json"}'
bcl exec headers clear
```

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

### request

Make HTTP requests. Supports all standard methods (GET, POST, PUT, DELETE, PATCH, HEAD, OPTIONS). Response body is parsed as JSON and converted to YAML for variable access; if parsing fails, the raw string is stored.

```yaml
# simple GET request
- request: https://api.example.com/users
  to: USERS

# GET with headers and response stored in variable
- request: https://api.example.com/users
  headers:
    Authorization: "Bearer $TOKEN"
  to: RESPONSE

# POST with body
- request: https://api.example.com/users
  method: POST
  body: '{"name": "test"}'
  headers:
    Content-Type: "application/json"
  to: RESULT

# with custom timeout (milliseconds)
- request: https://api.example.com/slow
  timeout: 30000
  to: SLOW_RESPONSE

# with error handling
- request: https://api.example.com/maybe-fail
  on_error:
    - log: Request failed, continuing...
```

**Fields:**
- URL (required) — supplied as the value of `request`
- `method` — HTTP method (default: GET)
- `headers` — map of header names to values (variables supported)
- `body` — request body (variables supported)
- `to` — variable name to store the response
- `timeout` — request timeout in milliseconds
- `on_error` — actions to run if the request fails
