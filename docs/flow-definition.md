# Flow Definition

Flows are defined in YAML files with named action sequences:

```yaml
flow-name:
  - action: value
    option: value

another-flow:
  - goto: https://example.com
  - click: #button
```

Each flow is a list of actions executed sequentially. Flows can call other flows using the `call` action.
