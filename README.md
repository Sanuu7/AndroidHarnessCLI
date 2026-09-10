# Android Harness CLI

An agent for the terminal, built for the phone in your pocket.

This is the interface layer of [AndroidHarness](https://github.com/Sanuu7/AndroidHarness)
ported to Rust: a full-screen TUI that runs natively in Termux, sized for a
narrow screen, animated like something you would want to look at, and compiled
to a single static binary.

**Current state: the UI.** The transcript, tool cards, animations, composer,
command palette, and layout all work, but an agent does not run behind them
yet. The replies are scripted so the motion can be judged before the engine
lands. Everything the UI needs from an engine (streamed tokens, tool start and
finish, usage) already flows through a channel, so wiring the real thing in is
a matter of replacing one module.

```
◆ harness                              workspace
▌you refactor the auth module

  ⠹ thinking                                          ← shimmer travels the label

▌ Looking at the auth flow, the token check uses `<=`
  instead of `<`, so an expired token passes for one
  extra second.

   diff
  - if (now <= expiry) return true
  + if (now < expiry) return true

▣ bash · gradle :app:testDebugUnitTest       ✓ 8.4s
▣ read_file · TokenValidator.kt                  ⠹
    $ sed -n '40,60p' TokenValidator.kt
    fun validate(token: Token, now: Long): Boolean
──────────────────────────────────────────────────────
› ask anything  ·  / for commands
⠹ glm-5.3-flash      ↑12k ↓1.2k  10%  $.0031
```

## What it does today

- Boot animation: the wordmark arrives letter by letter with a band of light
  running through it, a rule draws itself underneath, then the chat slides in.
- Streamed replies with markdown: headers, bullets, quotes, rules, fenced code
  with a code surface, inline bold, italic, and `code`.
- Thinking state with a braille spinner and a shimmer that scans the label.
- Tool cards that collapse to one line and reveal their output with a height
  animation. The most recent one toggles with `ctrl+e`.
- Composer: multi-line, wrapping, cursor that survives wide characters,
  history, word delete, and a `/` palette that filters as you type.
- Status bar: model, tokens in and out, context percent, and a cost readout
  that flashes when it changes.
- Layout that sheds pieces as the screen narrows: 60+ columns get everything,
  42+ keep the token counts, below that the header goes away entirely.
- Color depth handling: truecolor where available, 256-color and 16-color
  fallbacks detected from `COLORTERM` and `TERM`.

## Install

One line, in Termux:

```bash
curl -fsSL https://raw.githubusercontent.com/Sanuu7/AndroidHarnessCLI/main/install.sh | bash
```

It picks the right static binary for your CPU, drops it in `$PREFIX/bin`, and
tells you if that is not on your `PATH` yet. Nothing else to install: the
binary carries its own libc and does not link OpenSSL.

```
>> downloading harness (aarch64, main)
>> installed /data/data/com.termux/files/usr/bin/harness (713K)
>> run it with: harness
```

Pin a specific version instead of `main` with
`curl -fsSL .../install.sh | bash -s -- --ref v0.1.0`.

### Build it yourself

#### Option A: on the phone

```bash
pkg install rust
git clone https://github.com/Sanuu7/AndroidHarnessCLI
cd AndroidHarnessCLI
./scripts/build-termux.sh
```

The first build takes a few minutes on a phone. After that, incremental
builds are quick. The script prints the binary path and how to put it on
`PATH`.

#### Option B: cross-compile on a desktop (faster)

A static `aarch64-unknown-linux-musl` binary needs no Termux packages at all,
since it carries its own libc and links no OpenSSL.

```bash
rustup target add aarch64-unknown-linux-musl
./scripts/build-cross.sh --push          # pushes to /sdcard/Download/harness
```

Then in Termux:

```bash
mkdir -p ~/.local/bin
cp /sdcard/Download/harness ~/.local/bin/
chmod +x ~/.local/bin/harness
export PATH=$HOME/.local/bin:$PATH       # add to ~/.bashrc to keep it
```

## Keys

Termux has no function keys, so everything sits on Ctrl chords and the
composer itself. `/help` shows the same list.

| Key | Action |
| --- | --- |
| `enter` | send |
| `ctrl+j` | new line |
| `/` | command palette |
| `tab` | complete the highlighted command |
| `up` `down` | history (or move the cursor inside multi-line input) |
| `pgup` `pgdn` | scroll the transcript |
| `ctrl+e` | expand or collapse the last tool card |
| `ctrl+w` | delete the word before the cursor |
| `ctrl+u` | clear the composer |
| `ctrl+c` | cancel the run, twice when idle to quit |
| `ctrl+l` | redraw |
| `esc` | close an overlay |

## Commands

`/clear`, `/compact`, `/cost`, `/doctor`, `/help`, `/init`, `/model`, `/plan`,
`/skills`, `/theme`. The ones that need an engine say so when you run them.

## Environment

| Variable | Meaning |
| --- | --- |
| `HARNESS_COLOR` | `true`, `256`, or `16` to force a color depth |
| `HARNESS_BG` | terminal background as `rrggbb`, used by fade-ins |

Termux reports truecolor already, so neither is usually needed. They matter
under tmux or a light theme, where fading toward a guessed background would
otherwise look wrong.

## Reviewing the UI without a terminal

The binary can render a single frame at any moment into ANSI text:

```bash
harness --dump chat 46 34 4200 > frame.ansi        # state, width, height, ms
harness --dump splash 46 34 900
harness --dump tools 80 40 3000 --256              # 256-color path
```

States: `splash`, `chat`, `stream`, `thinking`, `tools`, `popup`, `help`,
`idle`. Sizes are meant to be a phone: 46x34 is a typical Termux portrait
window.

`tools/ansi2html.py` turns a dump into an HTML page, which is how the colors
get checked in a browser:

```bash
python3 tools/ansi2html.py < frame.ansi > frame.html
```

## How it is put together

```
install.sh       what the one-line install downloads and runs
scripts/         build-termux.sh, build-cross.sh, release.sh (refreshes dist/)
dist/            committed static binaries, aarch64 and x86_64
src/
  main.rs        terminal setup, event loop, frame pacing
  app.rs         state, key handling, agent events, layout decisions
  theme.rs       palette in plain RGB, converted at the last moment
  anim.rs        clock, easing, tweens, shimmer and swept gradients
  input.rs       composer: char-indexed buffer, wrapping, history
  markdown.rs    streaming-friendly markdown to styled lines
  textutil.rs    width-aware wrap, truncate, pad, token formatting
  ui/            splash, transcript, chrome, palette, help
  demo.rs        scripted agent, same event shape the engine will use
  dump.rs        headless frame rendering
```

Two rules shape most of the code:

**Animation is a pure function of time.** Nothing owns a mutable animation
state machine. Views rebuild their lines from `app.now` every frame, so a
frame dump at any timestamp is exactly what the terminal shows at that
timestamp. That is what makes the UI reviewable in CI and in a screenshot.

**Frame pacing follows the work.** 30fps while something moves, 8fps when the
screen is settled and only the input cursor blinks. The event loop decides
from `App::is_animating`, so an idle session on a phone battery costs almost
nothing.

## Tests

```bash
cargo test
```

37 tests covering width math (CJK and emoji sit in two cells, so nothing uses
`len()`), wrapping and prefix-stable cursor math, history, markdown rendering
while a fence is still open, tool card state, and the splash timeline.

## Next

The engine: providers (OpenAI, Anthropic, Gemini, Responses, the keyless
relay), the tool registry, sessions, compaction, skills, and MCP. The port
plan starts from the Android app, which already has all of it in Kotlin.

## License

MIT, same as AndroidHarness.
