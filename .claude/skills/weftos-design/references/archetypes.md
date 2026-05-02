# Surface archetypes — TOML skeletons

Mirrors `docs/DESIGN.md` §4. Drop one of these in,
substitute substrate paths, render.

## 1. `app-window` — full app

```toml
[[surfaces]]
id     = "app://weftos.example/main"
modes  = ["desktop"]
inputs = ["pointer", "hybrid"]
title  = "Example"
subscriptions = ["substrate/example/*"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

  [[surfaces.root.children]]
  type  = "ui://dock"
  id    = "/root/dock"
  attrs = { position = "left" }
  # sections as children, each {type="ui://pressable", attrs.label, on_tap}

  [[surfaces.root.children]]
  type = "ui://stack"
  id   = "/root/content"
  attrs = { axis = "vertical" }
  # main content

[surfaces.empty_state]
type = "ui://stack"
attrs = { axis = "vertical" }
# italic dim text + optional remediation pressable

[surfaces.offline_state]
type = "ui://chip"
bindings = { tone = '"crit"', label = '"◉ Demo mode — kernel daemon offline"' }
```

## 2. `chip-detail` — tray-chip subsystem inspector

```toml
[[surfaces]]
id     = "weftos-chip/example"
modes  = ["desktop"]
inputs = ["pointer"]
title  = "Example"
subscriptions = ["substrate/example/status"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

  [[surfaces.root.children]]
  type     = "ui://chip"
  id       = "/root/state"
  bindings = { label = "$substrate/example/status.state", tone = "$substrate/example/status.state" }

  # additional gauges / plots / streams as appropriate
```

## 3. `tile-grid` — launcher / dashboard

```toml
[[surfaces]]
id     = "app://weftos.launcher/main"
modes  = ["desktop"]
inputs = ["pointer"]
title  = "Launcher"

[surfaces.root]
type  = "ui://grid"
id    = "/root"
attrs = { cols = 6, gap = 12 }

# tiles as children:
  [[surfaces.root.children]]
  type    = "ui://pressable"
  id      = "/root/files"
  attrs   = { label = "Files" }
  on_tap  = { verb = "ui.app.open", params = { id = "app://weftos.files" } }
```

## 4. `list-detail` — explorer / settings / picker

```toml
[[surfaces]]
id     = "app://weftos.example/main"
modes  = ["desktop"]
title  = "Example"

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "horizontal" }

  [[surfaces.root.children]]
  type     = "ui://tree"
  id       = "/root/list"
  bindings = { root = "$substrate/example" }
  attrs    = { depth_limit = 3, lazy = true }

  [[surfaces.root.children]]
  type  = "ui://stack"
  id    = "/root/detail"
  attrs = { axis = "vertical" }
  # bound to selected node from /root/list
```

## 5. `stream` — terminal / logs / chat

```toml
[[surfaces]]
id     = "app://weftos.example/main"
modes  = ["desktop"]
title  = "Example stream"
subscriptions = ["derived/example/*"]

[surfaces.root]
type  = "ui://stack"
id    = "/root"
attrs = { axis = "vertical" }

  [[surfaces.root.children]]
  type  = "ui://strip"
  id    = "/root/filters"
  # filter chips

  [[surfaces.root.children]]
  type     = "ui://stream"
  id       = "/root/tail"
  bindings = { lines = "$derived/example.lines" }
  attrs    = { tail = true, filter_chips = ["info", "warn", "error"] }
```
