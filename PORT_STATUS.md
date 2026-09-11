# Android app to Termux CLI

Compared with the local AndroidHarness source on 2026-09-11, using CodeGraph
and the app tool registry, run manager, memory and skill implementations.
This is a practical feature map, not a claim of complete parity.

| Area | CLI status |
| --- | --- |
| Providers | Existing Harness, OpenAI-compatible, Responses, Anthropic and Gemini wires; provider/model/thinking pickers. |
| File, Git, shell, web tools | Existing native CLI equivalents, including background shell work. |
| Conversation compaction and cost | Existing; manual compaction now saves immediately and `/context` makes limits accessible. |
| Planning | Added `/plan`: restricted inspection tools, execution guard and visible phone-footer mode. |
| Recovery | Added atomic checkpoints, missing-result repair, unique session IDs and workspace validation. |
| Resume | Added transcript restoration at startup and saved-provider restoration; fixed `--continue` swallowing the next option. |
| Memory | Existing core/topics/search; added topic discovery in reads and prompts, plus `/memory`. |
| Task checklist | Existing tool/storage; added `/todos` display. |
| Skills | Existing discovery/view; added supporting-file reads, workspace precedence, create/update tool and `/skills name`. |
| CodeGraph | Added conditional prompt guidance for already-indexed projects. |
| Phone interface | Two-row compact footer, context visibility, contextual composer text, useful welcome commands and bounded help overlay. |
| Prompt submission during work | Kept as a draft; no accidental hidden worker queue. |

## Remaining work

- MCP server connections and dynamic tool discovery.
- Subagent/task delegation and interactive agent questions.
- Persisted prompt queues, editable plans and execution approval flows.
- Automation schedules/history, background keepalive and completion notifications.
- Embedded browser DOM control, screenshots and image attachments.
- Full skill settings, enable/disable, user-skill patching, bundled-skill migration.
- Session branching, exports, search/archive controls and persisted usage totals.
- Build/test dashboard and richer structured tool-result cards.
- Android-specific integrations: Shizuku, logcat permissions, embedded Linux setup,
  SAF workspaces, GitHub OAuth UI and app-native browser services.

Those need separate terminal workflows or runtime integrations. They are not
represented by placeholder commands in this CLI. Planning here is a restricted
tool workflow, not an operating-system sandbox.
