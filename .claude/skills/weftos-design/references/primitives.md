# WeftOS composer primitives — full prop reference

Authoritative when-to-use rules live in `docs/DESIGN.md` §3. This file
is the prop table + decision flow + per-primitive examples that
authors actually consult while writing TOML.

The `surface_host::compose()` runtime at
`crates/clawft-gui-egui/src/surface_host/compose.rs` is the sole
consumer; props not listed here are silently ignored.

---

## Common attributes (every primitive)

| key | type | required | notes |
|---|---|:---:|---|
| `type` | `"ui://<name>"` | ✓ | Primitive selector. |
| `id` | path string `/root/...` | ✓ | Stable identity for focus, dispatch, audit. |
| `attrs` | inline table | — | Static props (size, axis, mode). |
| `bindings` | inline table | — | Each value is either `"$path/to/substrate.field"` or `'"literal"'`. |
| `children` | array of primitives | — | Container only. |
| `on_tap` | `{ verb, params }` | — | Affordance dispatch (`pressable`, `toggle`, `select`, `slider`, `field`). |
| `empty_state` | child primitive | container | Required on user-visible app surfaces (DESIGN.md §5). |
| `loading_state` | child primitive | container | Required on user-visible app surfaces. |
| `offline_state` | child primitive | container | Required on user-visible app surfaces. |

---

## Layout

### `ui://stack`
`attrs.axis = "vertical" | "horizontal"`. Default container.

### `ui://grid`
`attrs.cols = N`, `attrs.gap = px`. Tiles; uniform width.

### `ui://tabs`
`attrs.position = "top" | "left"`. ≤ 6 children. Each child should
have `attrs.label`.

### `ui://strip`
Horizontal rail of small chips. Wraps disabled.

### `ui://dock`
App-level navigation. One per surface. `attrs.position = "left" | "top"`.

### `ui://sheet`
Bottom sheet. `attrs.height = px | "auto"`.

### `ui://modal`
Blocking. `attrs.title` + body children. ESC closes (DESIGN.md §7).

### `ui://sidebar`
Desktop-level collapsible nav. One per desktop. **Region, not window.**
The sidebar occupies a permanent vertical strip from the very top edge
to the very bottom edge of the screen — no top margin, no bottom
margin, no rounded corners, no drop shadow. Tray and identity strip
both sit *to the right of* the sidebar; nothing overlaps it. Width is
deducted from the total desktop area; the wallpaper does not paint
behind it.

Children are `{kind, label, glyph, target | children}`:

| key | type | notes |
|---|---|---|
| `attrs.collapsed` | bool | starts collapsed (icon-only rail) |
| `attrs.hidden` | bool | starts slid out off-screen-left |
| `attrs.width` | px | default `220` expanded, `48` collapsed |
| `children[].kind` | `"leaf" \| "group"` | leaf opens an app, group expands in place |
| `children[].label` | string | required |
| `children[].glyph` | string | required (visible in collapsed rail) |
| `children[].target` | `{verb, params}` | required for `leaf` (typically `ui.app.open`) |
| `children[].children` | array | required for `group`, recursive |
| `children[].expanded` | bool | for `group`, default `false` |

State lives in substrate at `substrate/desktop/sidebar/{collapsed, hidden, expanded[]}`
so it survives daemon restart. When `hidden=true`, a 6px edge-handle
remains pinned to the left screen edge so the user can slide the
sidebar back in.

---

## Data display

### `ui://chip`
| key | type | notes |
|---|---|---|
| `bindings.label` | string | shown text |
| `bindings.tone` | `"ok" \| "warn" \| "crit" \| "neutral"` | maps to palette state colors |
| `bindings.glyph` | string | optional leading glyph |

### `ui://gauge`
| key | type | notes |
|---|---|---|
| `attrs.min` `attrs.max` | f64 | bounds |
| `bindings.value` | number | substrate-bound scalar |
| `bindings.label` | string | caption |

### `ui://table`
| key | type | notes |
|---|---|---|
| `attrs.columns` | `[{key, label, kind}]` | `kind` = `"text" \| "number" \| "chip" \| "date"` |
| `bindings.rows` | array of records | substrate-bound |
| `attrs.sortable` | bool | default `true` |
| `attrs.row_action` | `{verb, params}` | optional per-row tap |

### `ui://tree`
| key | type | notes |
|---|---|---|
| `bindings.root` | substrate path | tree root |
| `attrs.depth_limit` | int | default `3` |
| `attrs.lazy` | bool | default `true` (uses `substrate.list { depth: 1 }`) |

### `ui://plot`
| key | type | notes |
|---|---|---|
| `bindings.series` | array of (t, v) | substrate stream |
| `attrs.window_secs` | int | rolling window |
| `attrs.y_min` `attrs.y_max` | f64 | bounds; auto if absent |

### `ui://heatmap`
2-D scalar field. `attrs.cols`, `attrs.rows`, `bindings.values`
(row-major flat array).

### `ui://waveform`
Audio PCM. `bindings.pcm`, `attrs.sample_rate`.

### `ui://stream`
| key | type | notes |
|---|---|---|
| `bindings.lines` | array of strings | latest-first |
| `attrs.tail` | bool | auto-scroll to bottom |
| `attrs.filter_chips` | `[string]` | renders a `ui://strip` above |

### `ui://canvas`
Escape hatch. Author-provided render closure. Each use is a TODO.

### `ui://media`
| key | type | notes |
|---|---|---|
| `bindings.src` | path or url | image / audio / video |
| `attrs.mime` | string | hints the player |

---

## Input

### `ui://pressable`
| key | type | notes |
|---|---|---|
| `attrs.label` | string | required |
| `on_tap` | `{verb, params}` | dispatched on click |
| `attrs.dangerous` | bool | wraps tap in confirm-modal |

### `ui://toggle`
| key | type | notes |
|---|---|---|
| `bindings.value` | bool | substrate-bound |
| `on_change` | `{verb, params}` | dispatched with new value |

### `ui://slider`
| key | type | notes |
|---|---|---|
| `attrs.min` `attrs.max` `attrs.step` | f64 | bounds |
| `bindings.value` | number | |
| `on_change` | `{verb, params}` | |

### `ui://select`
| key | type | notes |
|---|---|---|
| `attrs.options` | `[{value, label}]` | ≤ 12 |
| `bindings.value` | string | |
| `on_change` | `{verb, params}` | |

### `ui://field`
| key | type | notes |
|---|---|---|
| `attrs.placeholder` | string | |
| `attrs.kind` | `"text" \| "number" \| "secret"` | |
| `bindings.value` | string | |
| `on_change` | `{verb, params}` | |

---

## Adoption: `ui://foreign`

Composer escape into native egui. **Discouraged.** Carry a
`# WEFTOS-DESIGN: TODO graduate to ui://X` comment. Audit script
flags missing comments.

---

## Decision flow (cheat-sheet)

```
What am I rendering?
├── A container of children → stack | grid | tabs | dock | strip | sheet | modal
├── A scalar value
│   ├── bounded with known range → gauge
│   └── unbounded → chip (with text)
├── A list of records
│   ├── ≥ 4 rows → table
│   └── < 4 rows → stack of chips
├── A hierarchy of paths → tree
├── A time series → plot
├── A 2-D field → heatmap
├── Audio PCM → waveform
├── A log/append-only stream → stream
├── A file/image/video → media
├── Tappable
│   ├── boolean toggle → toggle
│   ├── bounded number → slider
│   ├── 1-of-N (≤12) → select
│   ├── free text → field
│   └── general → pressable
└── Nothing fits → canvas (last resort) or foreign (TODO)
```
