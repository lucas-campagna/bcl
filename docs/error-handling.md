# Error Handling

Actions can specify timeout and error handling callbacks.

## Timeout

Set a timeout in milliseconds:

```yaml
type:
  value: text
  xpath: //*[@id="input"]
  timeout: 5000
```

## on_error

Actions to execute if the main action fails:

```yaml
type:
  value: text
  xpath: //*[@id="input"]
  on_error:
    - log: Type action failed
    - click: #fallback-input
```

## on_timeout

Actions to execute if the action times out:

```yaml
click:
  xpath: //*[@id="slow-button"]
  timeout: 3000
  on_timeout:
    - log: Click timed out
    - wait:
        duration: 1000
    - click: //*[@id="slow-button"]
```

## Action Chaining

Error handlers can contain multiple actions:

```yaml
type:
  value: text
  xpath: //*[@id="input"]
  on_error:
    - log: First error handler action
    - log: Second error handler action
    - click: #fallback
```
