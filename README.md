<div align="center">

<pre>
   ____  ____  ____  ____ _____ ___  ___
  / __ \/ __ \/ __ \/ __ `/ __ `__ \/ _ \
 / / / / /_/ / / / / /_/ / / / / / /  __/
/_/ /_/\____/_/ /_/\__,_/_/ /_/ /_/\___/
</pre>

**A quiet terminal UI for Claude Code, written in Rust**

[![Rust](https://img.shields.io/badge/rust-2024-b497ff?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Claude Code](https://img.shields.io/badge/engine-Claude%20Code-d0a9ff?style=flat-square)](https://claude.com/claude-code)
[![kitty](https://img.shields.io/badge/terminal-kitty-a6e3a1?style=flat-square)](https://sw.kovidgoyal.net/kitty)
[![license](https://img.shields.io/badge/license-AGPL--3.0-f38ba8?style=flat-square)](LICENSE)

<picture><source media="(prefers-color-scheme: dark)" srcset="assets/demo-dark.gif"><source media="(prefers-color-scheme: light)" srcset="assets/demo-light.gif"><img src="assets/demo-dark.gif" width="860" alt="noname terminal UI for Claude Code: streaming answers, tool calls and an animated starfield background"></picture>

</div>

---

## About

noname is a terminal UI (TUI) for [Claude Code](https://claude.com/claude-code), built in Rust with ratatui. It replaces the interface of Claude Code, not the engine. `claude` runs underneath, so your subscription, plugins, skills, MCP servers and hooks keep working. Made for kitty with a transparent background, works in any truecolor terminal and in Windows Terminal.

- streaming text, thinking and tools, diffs that write themselves in
- inline permissions and questions, rewind with `esc esc`, `/btw` side questions
- live context and limit bars, cost under every answer
- themes (default, dracula, tokyo-night, catppuccin mocha, or your own), animated backgrounds, self updates
- image paste, `@` file mentions, background shells and subagents, `/diff`, `/mcp`, `/resume`
- Linux, macOS and Windows; optional Codex engine with `--codex`, opencode engine with `--opencode` and Antigravity engine with `--agy`

## Install

```sh
curl -fsSL https://github.com/Silvertree2010/noname/releases/latest/download/install.sh | sh
```
```powershell
irm https://github.com/Silvertree2010/noname/releases/latest/download/install.ps1 | iex
```

Also: Homebrew (`brew tap Silvertree2010/noname && brew trust Silvertree2010/noname && brew install noname`), [Scoop](https://github.com/Silvertree2010/scoop-noname), `.deb` and `.rpm` from the [releases](https://github.com/Silvertree2010/noname/releases/latest), or `cargo install --git https://github.com/Silvertree2010/noname`. AUR is coming. Needs a logged-in [Claude Code](https://claude.com/claude-code).

## Usage

```sh
noname              # new session
noname -c           # continue the latest session
noname -r <id>      # resume a session
noname --codex      # use Codex as the engine
noname --opencode   # use opencode as the engine (one `opencode run` per message)
noname --agy        # use the Antigravity CLI as the engine (one `agy -p` per message)
```

With `--opencode`, a slash command that is not one of noname's own (`/researcher`, `/deep-research`, your own commands and skills) runs as `opencode run --command NAME ARGS`. Commands that need a shell still need a permission: `opencode run` cannot ask, so allow them in opencode's config or pick the auto or bypass mode.

### Antigravity (`--agy`)

Standard setup for Google's Antigravity CLI (`agy`):

1. Install `agy` and run it **once** by hand to sign in. noname runs `agy -p` without a terminal, so it only works with the cached sign-in; without it the run ends with `authentication required`, shown in the chat.
2. Start `noname --agy` in your project folder. Add `-c` or `--conversation ID` to resume, and `--model NAME`, `--effort low|medium|high|xhigh|max` or `--agent NAME` as you would on `agy` (`agy models` and `agy agents` list them). `--antigravity` and `NONAME_ENGINE=agy` do the same as `--agy`.
3. Pick the mode with Shift+Tab or `/noname`:

| noname mode | `agy` flag |
|---|---|
| default | none: shell commands are asked |
| accept edits | `--mode accept-edits` |
| plan | `--mode plan` |
| auto | `--mode accept-edits` |
| bypass | `--dangerously-skip-permissions` |

`agy -p` cannot ask for permission. In default mode a command that needs approval is skipped, the run still ends well, and noname shows the note from `agy` under the answer. Allow what you trust once in `~/.gemini/antigravity-cli/settings.json` (replace the examples with your own commands) so default mode works without bypass:

```json
{ "permissions": { "allow": ["command(git)", "command(regex:npm run (build|lint|test))", "write_file(src/)"] } }
```

Slash commands and skills (`/researcher`, your own) go to `agy` as the prompt, which expands them itself. `agy` menu commands such as `/model`, `/usage` and `/permissions` are not available; use the startup flags. An image you paste is saved as a file and its path is added to the prompt, because print mode takes text only. The footer shows no usage bars: `agy` reports tokens per answer, not limits.

To report a problem, start with `NONAME_ENGINE_LOG=/tmp/agy-events.log noname --agy`: every raw event line from `agy` is appended to that file (works for `--codex` and `--opencode` too). Attach it, and the output of `agy --version`.

Other flags go straight to `claude`.

| key | | command | |
|---|---|---|---|
| `enter` | send | `/noname` | settings |
| `shift+enter` | new line | `/theme` | color theme |
| `esc` | stop | `/background` | animated background |
| `esc esc` | rewind | `/default-permission` | starting mode |
| `shift+tab` | cycle mode (manual, accept edits, plan, auto) | `/resume` | older sessions |
| `ctrl+o` | details | `/btw` | side question |
| `→` | accept suggestion | `/model` | cycle models |
| `ctrl+v` | paste image | | |
| `ctrl+backspace` | delete word | `/tasks` | background tasks |
| `ctrl+g` | edit in $EDITOR | `/mcp` | mcp servers |
| `ctrl+b` | move to background | `/diff` | workspace diff |
| `@` | mention a file | `/effort` | effort level |
| `↓` | tasks | `/rename` | name the session |
| `drag` | select and copy | `double / triple click` | copy word / line |
| `ctrl+z` / `ctrl+y` | undo / redo input | `/copy` | copy last answer |
| `ctrl+f` | find in chat | `/copy code` | copy last code block |
| `alt+↑` / `alt+↓` | jump between prompts | `/voice` | dictate (see below) |
| `ctrl+o` on a paste | unfold pasted text | | |

Every Claude Code command and skill works as usual, except Claude Code's own `/voice`, which only works in its interactive screen; noname has its own.

### Voice

`/voice whisper` or `/voice parakeet` picks a speech model and downloads it once, together with [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (checked against pinned sha-256 sums, kept in `~/.local/share/noname/voice`, `%LOCALAPPDATA%\noname\voice` on Windows). After that `/voice` starts recording; `enter` puts the text in the input box, `esc` cancels. Everything runs on your machine. Whisper base is 148 MB and knows many languages; Parakeet TDT 0.6B v3 is 356 MB, 25 European languages, and more accurate. Linux needs `pw-record`, `parecord` or `arecord`; macOS is not supported yet (whisper.cpp publishes no macOS build).

Don't need it? Build with `cargo build --release --no-default-features` and the voice code is left out of the program entirely.

### Session files on /clear

Claude Code keeps every session on disk (transcript, tool results, file history, plans). In `/noname`, **session files on /clear** decides what happens to them when `/clear` starts a new session: `off` (the default, Claude keeps them), `move` or `delete`. With `move` they go to one dated folder per session inside the **move folder** (pick that row, or type `/clear-folder <path>`), in the same layout as under `~/.claude`, so a session can be copied back. A moved or deleted session no longer shows up in `/resume`. Your memory and other sessions are never touched.

### Custom themes

Drop a `<name>.theme` file into `~/.config/noname/themes/` (`%APPDATA%\noname\themes\` on Windows) and it shows up in `/theme`. Missing keys fall back to the default theme, and a file named like a built-in theme replaces it.

```ini
desc    = black, steel, red
fg      = #ececec
bright  = #ffffff
muted   = #b4b4b4
faint   = #7a7a7a
accent  = #e0362b
accent2 = #c9ccd1
soft    = #d6d9de
cyan    = #9aa7b4
green   = #a9c4b0
red     = #ff6b5e
pink    = #d08a8a
base    = #0a0a0a
```

## License

[AGPL-3.0](LICENSE). noname bundles nothing; Claude Code, Codex and the background programs run from your own installation.
