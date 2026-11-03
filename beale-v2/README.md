# v2

A complete re-implementation of Beale using lessons learned from the first. This directory will be moved to the root and replace its content when it is ready.

The Rust workspace now includes `beale-contracts`, `beale-core`, `beale-providers`, `beale-sessions`, `beale-storage`, `beale-tools`, and `beale-workspaces` under `apps/beale-app-server/beale-harness/`, plus the first Rune desktop client in `apps/beale-desktop-app/`. `beale-core` assembles the host; the session and workspace crates own their respective operations, using the shared host database for global records. Other app and extension directories remain reserved until their crate manifests exist. The workspace uses Rust edition 2024 and requires Rust 1.89 or newer.

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

In Agent Settings → Providers, select OpenAI or xAI and choose **Sign in** or **Configure**. Sign in opens the provider's authorization page in the default browser on the same device. For xAI, enter the device code shown in Beale. The app-server completes the flow and saves the subscription credential; the desktop polls for completion. Configure accepts an API key in a masked field and sends it to the local app-server. The desktop only receives whether each method is configured. **Forget** removes the saved credential for that method. Use `cargo run -p beale-desktop-app -- --provider-status` to check both providers without opening the window.

The Providers view loads the available text models for each configured sign-in method. OpenAI subscription and API key access, and xAI subscription and API key access, each have their own catalog and saved lead/small choices. Select a method in Default Models, then use **Choose** to save a model for either role. **Refresh** reloads the provider catalog; if a saved model is no longer offered, the app-server selects an available default. The **Default Provider** control beside the provider tabs selects which configured provider will be primary. The first provider configured becomes lead automatically; removing its last credential selects another configured provider. Catalog and default operations require the same local control proof as credential operations.

Open **New Research**, select a workspace and an available model, enter a message, then press the send arrow. The app-server runs the selected provider's tool calls and saves turn activity locally. The available base tools are `read`, `write`, `edit`, and `bash`; `tool_result_page` lets the model inspect omitted portions of large tool output. The first response turns the draft into a session in the same view; its transcript appears above the composer. Saved sessions appear beneath their workspace in the Home sidebar and can be reopened after a restart. Tool rows initially show a short description; expand one to fetch its arguments and a bounded output page. Bash runs with the current user's host privileges, so use an externally managed VM or container when target code needs isolation. Permissions and repeat scheduling are not connected yet.

Both processes use `127.0.0.1:43831` by default. If that port is occupied, set `BEALE_V2_APP_SERVER_ADDR` to the same loopback IP and port in both terminals before starting them. For example, in each PowerShell terminal:

```powershell
$env:BEALE_V2_APP_SERVER_ADDR = "127.0.0.1:43832"
```

The server rejects non-loopback addresses. Local credential and session operations require a random control token stored in the app-server's private host directory (`~/.beale/v2` by default, or `BEALE_V2_HOME` in both processes). Host-wide workspace registration, sessions, and provider choices are stored in `state.sqlite` there. Earlier v2 workspace, session, and provider setting files are not read; recreate any needed workspace and session and reselect provider choices after switching to the database. Before sending credentials, the desktop challenges the process on the loopback port to prove it knows that token. OAuth tokens and API keys remain in protected host files; on Windows, API keys are encrypted for the current user with DPAPI. Desktop appearance and window placement remain client-local files. Remote Tailscale access is not enabled yet.

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
