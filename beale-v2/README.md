# v2

A complete re-implementation of Beale using lessons learned from the first. This directory will be moved to the root and replace its content when it is ready.

The Rust workspace now includes `beale-contracts`, `beale-core`, `beale-extender`, `beale-providers`, `beale-safety`, `beale-sessions`, `beale-storage`, `beale-tools`, and `beale-workspaces` under `apps/beale-app-server/beale-harness/`, plus the first Rune desktop client in `apps/beale-desktop-app/`. `beale-core` assembles the host; the session, workspace, and extension crates own their respective operations, using the shared host database for global records. Other app and extension directories remain reserved until their crate manifests exist. The workspace uses Rust edition 2024 and requires Rust 1.89 or newer.

## Run the local desktop and app-server

Open two terminals in `beale-v2/`. Start the app-server in the first terminal:

```text
cargo run -p beale-core -- serve
```

Start the desktop in the second terminal:

```text
cargo run -p beale-desktop-app
```

The Agent Settings sidenav shows the negotiated protocol version when the desktop validates the app-server's protobuf health response. To check the connection without opening the window, run `cargo run -p beale-desktop-app -- --check-connection`; it exits unsuccessfully if the host is offline or has an incompatible contract. Start order does not matter: the desktop retries every two seconds.

In Agent Settings → Providers, OpenAI, xAI, Anthropic, and Z.ai offer same-device subscription sign-in or an API key; OpenRouter uses API keys. Anthropic sign-in opens the official Claude CLI, and Z.ai sign-in opens ZCode Model Settings. Beale checks their official sign-in state without copying their subscription credentials. **Configure** sends a masked key to the local app-server; the desktop receives only whether the key is configured. **Forget** removes a saved credential. The **Custom** tab registers multiple OpenAI-compatible chat completions APIs by name, base URL, and model IDs, which can be discovered from `/models` or entered manually; each has an optional host-saved API key, lead/small defaults, and an opt-in OpenAI-compatible reasoning-effort control. Forget a saved key before changing that provider's API URL. Use `cargo run -p beale-desktop-app -- --provider-status` for built-in credential status without opening the window.

The Providers view loads available text models for configured built-in providers. OpenAI, xAI, Anthropic, and Z.ai maintain separate subscription and API-key catalogs and lead/small choices. Anthropic and Z.ai subscription catalogs currently contain selected models supported by their official CLIs; OpenRouter loads an API-key catalog. OpenRouter includes its automatic router and models that advertise tool and text support. Custom providers use the model IDs entered in their configuration, qualified by provider identity in sessions. The **Default Provider** control selects any configured built-in or custom provider; the first configured provider becomes lead automatically. Removing its last credential selects another configured provider. Catalog, default, and custom-provider operations require the same local control proof as credential operations.

For an Anthropic API key that is not scoped to one workspace, set `ANTHROPIC_WORKSPACE_ID` in the app-server environment before launch.

Subscription usage circles appear at the bottom of the icon rail when a provider reports a limit window. Hover for remaining percentages and reset countdowns. xAI uses the Beale subscription sign-in. OpenAI usage currently requires a separate Codex sign-in on the same host; its tooltip names Codex as the source. The desktop refreshes usage every 20 minutes and after subscription status changes.

Open **New Research**, select a workspace and an available model, enter a message, then press the send arrow. The app-server runs the selected provider's tool calls and saves turn activity locally. The available base tools are `read`, `write`, `edit`, and `bash`; `tool_result_page` lets the model inspect omitted portions of large tool output. The first response turns the draft into a session in the same view; its transcript appears above the composer. The draft workspace and repeat ribbon slides away when a session starts and stays absent for reopened sessions. Saved sessions appear beneath their workspace in the Home sidebar and can be reopened after a restart. The composer’s up and down buttons revisit sent messages from the current session and restore any unsent draft when moving past the newest message. Tool rows initially show a short description; expand one to fetch its arguments and a bounded output page. Bash runs with the current user's host privileges, so use an externally managed VM or container when target code needs isolation. Repeat scheduling is not connected yet.

Agent Settings opens on Safety, with Auto-Review and Guardian tabs. The configured small model reviews each `read`, `write`, `edit`, and `bash` call against three built-in rules: broad destructive changes, unrelated credential access, and recorded scope. After a denial, the session lead model can review the call once against two built-in Guardian rules. The plus button beside either rule-set title adds a rule to that stage. Each rule's name and description can be edited in a dialog, and each rule can be turned off; changes live in the host database and apply to subsequent tool calls. The catalog allows 16 rules in total. Review failures deny the call. With all Auto-Review rules off, base tool calls run without model review. Reviews do not restrict host privileges or provide OS isolation.

The **Extensions** icon opens a separate sidebar with Plugins, Features, and Research Kits. Plugins opens by default with Available and Installed tabs; Installed filters the same plugin catalog. Four bundled Agent Plugins packages appear under Available as content-sized cards in the wide content area, and none are installed by default. Each card shows a three-line description and separate Skills, Tools, Resources, and Prompts stats. Bundled skill counts come from their package contents. MCP counts show an unknown marker until the plugin server can be started and queried; packages without an MCP server show zero. **Install** validates a bundled package and saves its installed state through the app-server; **Uninstall** clears that state and leaves the package available for reinstall. Installed reflects host-owned state across restarts. Installation does not yet load skills or MCP components into sessions. The plus button opens an external-source form, but source registration is not connected yet, so the form cannot submit.

Click **Quick Chat** beneath New Research to open a compact chat at the bottom right. Before sending, choose a registered workspace in the title ribbon or leave **No Workspace** selected for general chat. A selected workspace supplies its saved guidance, rules, and scope as context; Quick Chat has no file tools. Enter a message in the single-line composer and press Enter or the send arrow. The up and down buttons revisit messages sent in that chat and restore the unsent draft. The ribbon becomes the chat title when the first message is sent, and a bounded, scrollable transcript appears beneath it. The host uses the Default Provider, its configured source priority, and its lead model. Closing the panel ends the current chat; opening Quick Chat again starts a new one. Saved Quick Chats appear under Agent Settings → Archive → Quick Chats, where Restore reopens the conversation in the dock. Archived workspace sessions appear in the Sessions tab and can be restored to the Home sidebar.

Both processes use `127.0.0.1:43831` by default. If that port is occupied, set `BEALE_V2_APP_SERVER_ADDR` to the same loopback IP and port in both terminals before starting them. For example, in each PowerShell terminal:

```powershell
$env:BEALE_V2_APP_SERVER_ADDR = "127.0.0.1:43832"
```

The server rejects non-loopback addresses. Local credential and session operations require a random control token stored in the app-server's private host directory (`~/.beale/v2` by default, or `BEALE_V2_HOME` in both processes). Host-wide workspace registration, sessions, and provider choices are stored in `state.sqlite` there. Earlier v2 workspace, session, and provider setting files are not read; recreate any needed workspace and session and reselect provider choices after switching to the database. Before sending credentials, the desktop challenges the process on the loopback port to prove it knows that token. OAuth tokens and API keys remain in protected host files; on Windows, API keys are encrypted for the current user with DPAPI. Desktop appearance and window placement remain client-local files. Remote Tailscale access is not enabled yet.

The app-server creates `SYSTEM.md` in that private host directory on startup. Edit its Markdown to change the base session prompt; the next turn reads the updated file. Future optional tool and plugin details can be appended at runtime without placeholder substitution.

Tool calls use compact icon and label rows with a trailing disclosure chevron. Provider supplied reasoning summaries appear as muted lines in the transcript when available.

During a session turn, the desktop receives assistant text, available reasoning summaries, and each tool's start and finish over a framed HTTP response. The live activity sits below a “Working for HH:MM:SS” header. When the turn finishes, the header becomes “Worked for HH:MM:SS” with a chevron; the activity collapses while the final answer stays visible. Expanding the header reveals saved tool, reasoning, and intermediate text activity after a restart.

## Core Directories

### Apps

- `apps/beale-app-server/`: A general agent runtime server shared across app clients
- `apps/beale-desktop-app/`: An installable app client for Windows, macOS, and Linux
- `apps/beale-ios-app/`: An app client for iOS, not yet available on the App Store

### Extensions

- `extensions/features/`: Optional native features with deep app integrations
- `extensions/plugins/`: Optional skills and MCP servers using Agent Plugins 1.0
- `extensions/research-kits/`: Optional helpers to streamline certain programs

### Integrations

- `integrations/beale-for-codex/`: A third-party integration plugin for Codex
- `integrations/beale-for-claude-code/`: A third-party integration plugin for Claude Code
- `integrations/beale-for-cursor/`: A third-party integration plugin for Cursor

### Utilities

- `utilities/scripts/`: Scripts to assist with release, build, and other workflows
- `utilities/site/`: Basic utilities and assets for the beale.ai website
