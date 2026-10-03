# Shadow DOM & Iframes

Interact with elements inside Shadow DOM or iframes.

## Shadow DOM

Use `shadow_root` to traverse into shadow roots:

```yaml
click:
  xpath: //button
  shadow_root: //*[@id="host"]
```

## Iframes

Use `iframe` to target elements inside iframes:

```yaml
click:
  xpath: //button
  iframe: //*[@id="frame"]
```

## Combined

Both can be used together:

```yaml
click:
  xpath: //button
  shadow_root: //*[@id="shadow-host"]
  iframe: //*[@id="content-frame"]
```
