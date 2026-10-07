# Shadow DOM & Iframes

Interact with elements inside Shadow DOM or iframes.

## Shadow DOM

Use `shadow_root` to traverse into shadow roots:

```yaml
click:
  target: //button
  shadow_root: //*[@id="host"]
```

## Iframes

Use `iframe` to target elements inside iframes:

```yaml
click:
  target: //button
  iframe: //*[@id="frame"]
```

## Combined

Both can be used together:

```yaml
click:
  target: //button
  shadow_root: //*[@id="shadow-host"]
  iframe: //*[@id="content-frame"]
```
