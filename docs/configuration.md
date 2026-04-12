# amux Configuration

amux reads its configuration from a TOML file. The preferred location is:

    ~/.amux/config.toml

This path is the same on every platform — no need to remember OS-specific
directories. On first launch, amux writes a fully-populated default config
here with every setting and its default value.

**Fallback paths** (checked if `~/.amux/config.toml` doesn't exist):

- **Linux**: `~/.config/amux/config.toml`
- **macOS**: `~/Library/Application Support/amux/config.toml`
- **Windows**: `%APPDATA%\amux\config.toml`

The config file is optional — amux runs with sensible defaults if no file
exists. Fields are deserialized via `serde` with `#[serde(default)]` on the
root struct, so you can omit any field you don't want to customize.

Changing the config file requires restarting amux to take effect.

---

## Top-level fields

```toml
# Visual
font_family  = "JetBrains Mono"
font_size    = 14.0
theme_source = "default"        # or "ghostty"
menu_bar_style = "menubar"      # "menubar", "hamburger", or "none"

# Shell
shell        = "pwsh"           # optional override; see Shell resolution below

# Nested tables (each documented below)
[notifications]
[browser]
[colors]
[keybindings]
```

### `font_family` / `font_size`

The terminal font. Resolved against system-installed fonts by
[`cosmic-text`](https://github.com/pop-os/cosmic-text). Any TrueType/OpenType
font installed on the system is available.

Defaults to a platform-appropriate monospace font at 14pt.

### `theme_source`

Controls where amux pulls its color palette from.

| Value       | Behavior                                                                                              |
|-------------|-------------------------------------------------------------------------------------------------------|
| `"default"` | Uses amux's built-in Monokai Classic palette — the same palette cmux ships by default. See `crates/amux-app/src/theme.rs` for exact values. **Default.** |
| `"ghostty"` | Reads `~/.config/ghostty/config` (or the platform equivalent) and derives terminal + chrome colors from Ghostty's theme. Fonts from Ghostty are also picked up and override `font_family` / `font_size` if present. |

If `theme_source = "ghostty"` but no Ghostty config is found, amux logs a
warning and falls back to `"default"`.

### `menu_bar_style`

Controls amux's in-window menu chrome on Windows and Linux. macOS always
uses the native NSApp menu bar at the top of the screen regardless of this
setting — it's the idiomatic macOS pattern and removing it would break
user expectations.

| Value         | Behavior                                                                                 |
|---------------|------------------------------------------------------------------------------------------|
| `"menubar"`   | Traditional two-strip layout: a dedicated `File Edit View` strip above the icon row. Costs ~24px of vertical chrome. **Default on Windows/Linux.** |
| `"hamburger"` | Single `≡` button collapsed into the icon row. Clicking it opens a flat popup listing every command grouped by section. Zero extra vertical chrome. |
| `"none"`      | No in-window menu chrome. Menu actions are reachable only via keyboard shortcuts. **Default on macOS.** |

### `shell`

Override the shell amux spawns in new panes. Accepts either:

- A bare binary name: `"pwsh"`, `"bash"`, `"fish"`. amux resolves it against
  `PATH`, honoring `PATHEXT` on Windows.
- An absolute path: `"/opt/homebrew/bin/fish"`,
  `"C:\\Program Files\\PowerShell\\7\\pwsh.exe"`.

When unset (the default):

- **Unix**: uses `$SHELL`, falling back to `/bin/bash`.
- **Windows**: prefers `pwsh.exe` (PowerShell 7) if it's on `PATH`,
  otherwise falls back to `$COMSPEC` (typically `cmd.exe`).

---

## `[colors]` — Palette overrides

Apply specific color overrides on top of whichever theme source is active.
Colors use 6-digit hex notation (`"#rrggbb"`). The parser is
`ColorsConfig::parse_hex` in `amux-core/src/config.rs`, which accepts
only RGB — 8-digit RGBA is not supported.

```toml
[colors]
# Terminal base colors
background    = "#101218"
foreground    = "#d0d4e0"

# Terminal cursor
cursor_fg     = "#101218"
cursor_bg     = "#61afef"

# Terminal selection highlight
selection_fg  = "#ffffff"
selection_bg  = "#2a3550"

# ANSI palette overrides (indices 0-15)
palette = [
    "#0f1117",  # 0  black
    "#e06c75",  # 1  red
    "#98c379",  # 2  green
    "#e5c07b",  # 3  yellow
    "#61afef",  # 4  blue
    "#c678dd",  # 5  magenta
    "#56b6c2",  # 6  cyan
    "#abb2bf",  # 7  white
    "#3a404e",  # 8  bright black
    "#e88088",  # 9  bright red
    "#a8d18e",  # 10 bright green
    "#edcd8f",  # 11 bright yellow
    "#77bdf2",  # 12 bright blue
    "#d08ae6",  # 13 bright magenta
    "#74c6d1",  # 14 bright cyan
    "#d0d4e0",  # 15 bright white
]
```

Any field you omit inherits from `theme_source`. The override layer is
applied after the theme source resolves, so e.g. you can use
`theme_source = "ghostty"` to pull most colors from Ghostty and only
override the selection background here.

---

## `[notifications]` — Agent notification settings

```toml
[notifications]
system_notifications    = true   # OS-native toast when the app is unfocused
auto_reorder_workspaces = true   # bump notifying workspaces to the top of the sidebar
dock_badge              = true   # show unread count on macOS dock / Windows taskbar
custom_command          = "/usr/bin/true"   # optional shell command run on each notification

[notifications.sound]
sound              = "system"    # "system" → OS default, "none" → silent,
                                  # or an absolute path to a .wav / .ogg / .mp3 file
play_when_focused  = true        # set false to silence when the app is in the foreground
```

Notifications are delivered via OSC 9 / OSC 99 / OSC 777 sequences from
agents (Claude Code, Gemini CLI, Codex CLI) or via the `amux notify` CLI.
Each delivered notification rings the configured sound and highlights the
originating pane with a blue ring.

The authoritative schema lives in `NotificationConfig` /
`NotificationSoundConfig` in `crates/amux-core/src/config.rs`. amux does
not ship named built-in sounds (there is no `"chime"` / `"ding"` / `"ping"`);
use `"system"` for the OS default or point `sound` at a file on disk.

---

## `[browser]` — In-app browser settings

```toml
[browser]
search_engine              = "google"   # google, duckduckgo, bing, kagi, startpage
open_terminal_links_in_app = false      # false → system browser, true → new browser tab
user_agent                 = "Mozilla/5.0 ..."   # optional override
download_dir               = "/Users/alice/Downloads"  # optional; defaults to system
                                                       # Downloads. `~` is NOT expanded —
                                                       # use an absolute path
```

The in-app browser is a wry/WebView2 pane that shares workspace layout with
terminal panes. Each browser pane persists cookies and localStorage per
workspace profile.

---

## `[keybindings]` — Shortcut overrides

```toml
[keybindings]
# Full platform defaults live in `crates/amux-core/src/config.rs`.
# User entries here are merged on top — set any action to override just
# that binding without re-declaring the full table.

new_workspace = "Ctrl+Shift+N"
new_tab       = "Ctrl+Shift+T"
close_tab     = "Ctrl+Shift+W"
split_right   = "Ctrl+Shift+D"
split_down    = "Ctrl+Shift+E"
# ...
```

See `crates/amux-core/src/config.rs` — the complete list of actions lives
in the `Action` enum, and the default bindings are produced by
`KeybindingsConfig::platform_defaults()` and merged with user overrides in
`KeybindingsConfig::resolved()`.

The binding syntax accepts modifier + key combos joined by `+`, where
modifiers are `Ctrl`, `Cmd`, `Alt`, `Shift`, `Super` and the key is any
[`egui::Key`](https://docs.rs/egui/latest/egui/enum.Key.html) name
(e.g. `A`, `F5`, `Enter`, `Escape`, `ArrowLeft`).

---

## Layering, precedence, and reloads

When amux starts up, it resolves the final theme and keybindings by
applying layers in this order, lowest priority first:

1. **Built-in defaults** — hard-coded in `amux-app/src/theme.rs` and
   `amux-core/src/config.rs::KeybindingsConfig::platform_defaults()`.
2. **Config file** — `~/.amux/config.toml` (preferred) or the platform-
   specific fallback (`dirs::config_dir()/amux/config.toml`). On first
   launch, amux writes a default config with every field populated.
3. **Theme source overlay** — if `theme_source = "ghostty"`, Ghostty's
   config overlays the built-in theme.
4. **`[colors]` / `[keybindings]` overrides** — per-field overrides from
   the config file are applied on top of the theme source.

Later layers only override the specific fields they set. Unset fields
inherit from below. This means you can start with a Ghostty theme and
override only a single selection color, or start from the amux default and
remap just one keybinding.

### Reloading config

amux does **not** watch the config file for changes. After editing, quit and
relaunch amux. Session restore preserves layout, working directories,
scrollback, and notification history across the restart.

---

## File locations (quick reference)

The main config lives at `~/.amux/config.toml` on all platforms. Other
files use platform-specific paths via `dirs::config_dir()` /
`dirs::data_dir()`:

| Purpose                  | Linux                                  | macOS                                                   | Windows                          |
|--------------------------|----------------------------------------|---------------------------------------------------------|----------------------------------|
| amux config (preferred)  | `~/.amux/config.toml`                  | `~/.amux/config.toml`                                   | `~/.amux/config.toml`            |
| amux config (fallback)   | `~/.config/amux/config.toml`           | `~/Library/Application Support/amux/config.toml`        | `%APPDATA%\amux\config.toml`     |
| amux session state       | `~/.local/share/amux/session.json`     | `~/Library/Application Support/amux/session.json`       | `%APPDATA%\amux\session.json`    |
| Ghostty config (if used) | `~/.config/ghostty/config`             | `~/Library/Application Support/com.mitchellh.ghostty/config` | `%APPDATA%\ghostty\config`  |
| Shell integration scripts| `~/.config/amux/shell/`                | `~/Library/Application Support/amux/shell/`             | `%APPDATA%\amux\shell\`          |
| Agent wrapper scripts    | `~/.config/amux/bin/`                  | `~/Library/Application Support/amux/bin/`               | `%APPDATA%\amux\bin\`            |

---

## Minimal working example

```toml
# ~/.amux/config.toml

font_family  = "JetBrains Mono"
font_size    = 14.0
theme_source = "default"

# Override just the selection highlight to something more visible
[colors]
selection_bg = "#3a4568"
```
