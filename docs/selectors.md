# Selectors

BCL auto-detects selector type based on the string format:

- **XPath**: strings starting with `/` or `//`
- **CSS**: strings containing `#`, `.`, `[`, or spaces

```yaml
# Auto-detected as XPath
- click: //*[@id="button"]

# Auto-detected as CSS
- click: #button
```

## Explicit Selector Type

You can also explicitly specify the selector type:

```yaml
type:
  value: text
  target: //*[@id="input"]
```

## Multiple Elements

When a selector matches multiple elements, the action is applied to each:

```yaml
- type: $TOKEN
  target: /html/body//input
```

This types the value into each matched input element.
