<p align="center"><img src="assets/cloverleaf.svg" width="96" alt="cloverleaf logo"></p>

# cloverleaf

a native gui for [opencode](https://opencode.ai). the built in one is a website and half of it
doesn't work, so this is a real window instead: rust + [egui](https://github.com/emilk/egui),
no webview, no electron.

it doesn't reimplement opencode. it starts `opencode serve` in the background and talks to its
http api, so your sessions, config, keys and models are the same ones the tui uses.

status: early, but usable day to day. linux/wayland is what it's built and tested on.

## what works

- all your existing sessions in a sidebar, grouped by project, with a filter
- live streaming: text, thinking, and tool calls on a timeline rail. edits open a colored diff,
  everything else is a one-liner you click to expand
- markdown, tables, syntax highlighted code blocks
- permission prompts (allow once / always / reject) and the agent's multiple-choice questions
- model, agent and reasoning-effort pickers
- `/commands` (yours plus `/new` `/models` `/agents` `/sessions` `/compact`), and `!cmd` to run shell
- command palette for everything
- settings: accent color, window opacity, ui scale, provider keys and browser logins, mcp servers,
  plugins, skills, saved permissions
- real window transparency, so compositor blur (hyprland etc) shows through the background while
  text stays solid

not there yet: `@file` mentions, image paste, undo/revert, session rename/fork, subagent sessions,
an embedded terminal.

## build + run

you need a rust toolchain and `opencode` on your `PATH`.

```sh
git clone https://github.com/ZIZIES/cloverleaf
cd cloverleaf
cargo build --release
./target/release/cloverleaf            # uses the current directory as the project
./target/release/cloverleaf ~/code/x   # or point it at one
```

## keys

| key | does |
| --- | --- |
| `enter` / `shift+enter` | send / newline |
| `ctrl+k` | command palette |
| `ctrl+n` | new session |
| `ctrl+b` | toggle sidebar |
| `ctrl+,` | settings |
| `esc` | stop the running session |

## where things live

- `~/.config/cloverleaf/state.json`: cloverleaf's own settings (accent, opacity, scale, default
  model/agent, mcp servers you added from the settings window). nothing secret goes in here.
- everything else (sessions, api keys, providers, plugins) belongs to opencode and stays where
  opencode keeps it.

### about api keys

cloverleaf never stores your keys and never reads them back. a key you paste in settings is sent
once to the opencode server on your own machine, which stores it, and is wiped from the ui. the
providers tab only asks the server whether something is connected.

the server cloverleaf spawns listens on `127.0.0.1` on a random port with a random password made
fresh each launch, and is killed when the window closes.

## env vars

| var | does |
| --- | --- |
| `CLOVERLEAF_SERVER` | attach to an already running `opencode serve` at this url instead of spawning one |
| `CLOVERLEAF_PASSWORD` | password for that server |

there are also a few `CLOVERLEAF_SHOT` / `CLOVERLEAF_OPEN` / `CLOVERLEAF_PROMPT` hooks used for
scripted testing, see `debug_hooks` in `src/app.rs`.

## code map

about 4k lines, plain threads and channels, no async runtime.

| file | what |
| --- | --- |
| `src/main.rs` | window setup |
| `src/server.rs` | spawns `opencode serve`, kills it on exit |
| `src/api.rs` | blocking http client + the server-sent-events reader thread |
| `src/model.rs` | structs parsed out of the api's json |
| `src/app.rs` | state, event handling, actions |
| `src/ui.rs` | drawing: sidebar, transcript, composer, palette |
| `src/settings.rs` | the settings window |
| `src/theme.rs`, `src/logo.rs` | palette + fonts, the clover |

opencode's http api is marked experimental upstream. this was written against opencode 2.2.5 and
may need touching up when that api moves.

## credits

- palette is from [ampersand](https://github.com/ZIZIES/ampersand)
- fonts: [Inter](https://github.com/rsms/inter) and
  [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), both under the SIL Open Font
  License (texts in `assets/licenses/`)

## license

Apache-2.0, see [LICENSE](LICENSE).
