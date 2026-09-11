# Android Harness CLI

A Rust coding agent for Termux, with a phone-width terminal interface.

## Run on Termux

```sh
pkg install rust clang make curl git
cargo build --release
./target/release/harness
```

From this checkout on Linux, `bash scripts/build-cross.sh` builds a static ARM64
binary in `target/aarch64-unknown-linux-musl/release/harness`.
Copy it into Termux's private executable directory, not shared storage:

```sh
mkdir -p ~/.local/bin
cp /sdcard/Download/harness ~/.local/bin/harness
chmod +x ~/.local/bin/harness
export PATH="$HOME/.local/bin:$PATH"
harness -C ~/my-project
```

The built-in Harness provider starts without a key. Custom provider configuration
lives in `~/.config/harness/config.json`. Network access requires curl.

## Phone controls

Type `/` for a searchable command menu. Enter sends, Ctrl+J adds a newline,
Ctrl+C stops, Ctrl+O expands tool output, and Ctrl+T shows reasoning.
While a task runs, a submitted follow-up stays in the composer as a draft.
Send it after completion. This is not a persistent prompt queue.

| Command | Behavior |
| --- | --- |
| `/plan on`, `/plan off` | Switch between inspection/planning and execution. Plan mode filters tools and rejects mutating calls, including shell commands. |
| `/context`, `/cost` | Inspect token usage, context limits and reported cost. |
| `/compact` | Summarize older conversation and save the result. |
| `/memory [topic]` | Read core notes and topic index, or one topic. |
| `/todos` | Show the workspace task checklist. |
| `/skills [name]` | List skills or load one. |
| `/sessions [id]` | Select a saved conversation, restoring its provider when available. |
| `/model`, `/provider`, `/thinking` | Choose a model, provider or thinking level. |
| `/stop` | Stop the active task without closing the interface. |
| `/doctor`, `/help` | Check tools or view keyboard help. |

`harness --continue` opens the latest saved session. Run it from that session's
workspace. `--continue ID` selects a session, and explicit model/provider options
override saved defaults. Older sessions without workspace metadata remain readable.
A resumed conversation waits for your next message; it does not automatically run.

## Memory and skills

Core notes: `.harness/memory.md`. Topics: `.harness/memory/*.md`.
Skills: `.harness/skills/<name>/SKILL.md`, then `~/.config/harness/skills/`.
A workspace skill takes precedence over a user skill with the same name.
The agent can read supporting files using `skill_view(name, file_path)` and
create/update workspace skills using `skill_manage`. Replacing a skill requires
`overwrite=true`. Supporting files must resolve inside the skill folder.

When `.codegraph/` exists, the agent prompt directs exploration through
`codegraph explore` first. The CLI never creates an index automatically.

## Saved progress

Sessions use atomic private writes. The CLI checkpoints the user prompt, each
completed model response, and each completed tool batch. When loading an
interrupted exchange, missing tool results are marked as unknown so the agent
can inspect the outcome before retrying. A crash during a tool or streaming
response can still lose that in-flight result. This is not exactly-once execution.

## Port status

[PORT_STATUS.md](PORT_STATUS.md) tracks what carries over from the Android app,
what this pass added, and the remaining platform work.

## Verification

```sh
cargo test
cargo build --release --target aarch64-unknown-linux-musl
harness --dump empty 36 24 5000
```
