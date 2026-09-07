# @beale/app-server

A standalone app-server execution host and client-neutral control plane. It runs each app-server session in an app-server worker and carries events, controls, and private synchronous storage requests directly over worker messages. Workers never open SQLite themselves: the resident app-server owns every database connection and restricts each worker to its session's registered database. There is no child app-server CLI process or private loopback WebSocket. Beale Desktop and iOS use the same authenticated HTTP and WebSocket surface.

The app-server is the single host adapter for Desktop, iOS, and future clients. Clients submit typed session intent; the app-server resolves workspace identity, paths, provider policy, plugins, storage, capture and continuation state, hosts the engine, and executes canonical operations in-process. Its bundled agent-plugin resources live under `app-server/resources/agent-plugins`.

## Managed tool plugins

Seven host-backed plugins are enabled by default and appear in Agent Plugins settings. Every session advertises each plugin with a short usage description, including disabled plugins and plugins whose tools are unavailable under the current profile or configuration.

| Plugin | When to use |
| --- | --- |
| Source (`beale-source`) | Source search, code navigation, and structured text analysis. |
| Provenance (`beale-provenance`) | Repository revision history, public advisory references, and source provenance. |
| Knowledge (`beale-knowledge`) | Reusable memory, workspace history, artifacts, local inspection, and resource inventory. |
| Claims (`beale-claims`) | Leads, findings, their evidence, and canonical claim revisions. |
| Investigations (`beale-investigations`) | Investigation records, questions, observations, and configured experiments. |
| Runbooks (`beale-runbooks`) | Reusable procedure documents, revisions, and recorded executions. |
| Reporting (`beale-reporting`) | Report documents, revisions, and structured summaries of supported results. |

`file.read`, `file.write`, `file.edit`, and `shell.run` remain core tools, alongside session and collaboration controls. Existing profile, configuration, and governance limits still apply. File writes create candidate files; replacing an existing file requires the SHA-256 `contentHash` returned by `file.read` as `expectedHash`. File edits require one exact literal match, preserve UTF-8 bytes outside that match, and accept an optional hash check. Both mutations have a 1 MiB ceiling and honor lower host byte budgets.

Enabled plugins are available for discovery. Pi agents initially receive core tools and `plugins.load`; requesting plugin IDs adds only those tool schemas on the next turn. Loaded IDs are isolated per agent and retained in compatible continuation captures. Claude uses its SDK's native `ToolSearch` with deferred plugin tools and always-loaded core tools. ZCode currently uses a fixed MCP tool list, so its enabled tool schemas remain eager. External MCP plugins retain their existing loading behavior.

Plugin toggles take effect on subsequent session launches and continuations, including Quick Chat. The app-server projects the current selection through `--managed-plugins <comma-separated IDs>`; `none` explicitly disables all seven. A settings-read failure stops the launch instead of silently restoring defaults. Loading a schema does not enable a disabled plugin or change host policy, canonical storage, or execution privileges.

New native tools must be assigned in `packages/research-agent/src/managed-tool-plugins.ts` to a plugin or the explicit core list. Runtime assembly rejects unassigned host tools. Bundled manifests and the compact discovery catalog are checked together by boundary tests.

The Claims plugin provides `claim.get` for either a lead or finding ID. Its default response includes the stored overview and the first 10 evidence, transition, and duplicate rows; focused `section` reads omit unrelated sections. Follow each collection's `nextOffset`, passing the returned `readRevision` as `expectedReadRevision`; restart if the claim or its duplicate relationships changed. The numeric `revision` remains the version used for edits. Evidence retains its durable reference, hash, source session, actor, and metadata; transitions retain reasons and evidence links. Retained duplicate IDs expose their canonical parent. Reads stay within the current workspace and do not change claim state. `lead.list` and `finding.list` are summary catalogs with continuation offsets; `history.search` remains the search entry point.

## Public history reads

The Provenance plugin includes `prior_art.search`, `prior_art.fetch`, `repository.history`, and `repository.fetch_history`.

- `prior_art.search` supports NVD keyword queries, OSV package/version or commit lookups, public GitHub issue/PR search and releases for an explicit `owner/repository`, and literal searches of supplied document URLs. Supplying a repository or document URLs includes those sources by default; an explicit `sources` list narrows coverage. Document search covers only the supplied pages, not an entire website or the open web.
- `nextCursor` continues unread records and provider pages with the same query inputs. Context pages contain at most 20 cards even if a larger limit is requested. Each source reports complete, partial, failed, or not-yet-searched coverage; rate limits, GitHub's search ceiling, and empty non-terminal pages are not no-match evidence. A changed buffered source page requires restarting the query. Earlier matches remain recorded in continuation disposition.
- Normalized advisory output retains OSV range events, versions, withdrawal/severity data and typed references, and NVD applicability trees with version bounds and metrics. Cross-source records retain separate provenance even when their IDs are aliases. Model search cards defer large details to `detailUrl`; full normalized data remains in the canonical tool result.
- `prior_art.fetch` reads HTTP(S) HTML, text, XML/feed, or JSON source pages without attaching host credentials or running scripts. Text and extracted HTML links have continuation offsets checked against the response content hash. Results include requested/final URLs, retrieval time, content type, ETag and Last-Modified when supplied. Reads are capped at 2 MiB and redirects are bounded; PDF/binary documents and JavaScript-only content need another reader. GitHub and NVD search API requests are unauthenticated, so their public rate limits apply.
- `repository.history` remains local and read-only. `repository.fetch_history` is an explicit network operation against an existing named remote, with optional `ref`, `deepen`, or `unshallow`. It fetches remote branches into remote-tracking refs or a selected ref into a dedicated history ref, ignores configured destination mappings, and performs no checkout, merge, reset, submodule recursion, or automatic maintenance. It does not fetch every tag or upstream automatically; record unfetched sources as coverage limits. Network isolation remains operator-managed.

API contracts: [NVD CVE API](https://nvd.nist.gov/developers/vulnerabilities), [OSV queries and pagination](https://google.github.io/osv.dev/post-v1-query/), [GitHub search](https://docs.github.com/en/rest/search/search), and [GitHub releases](https://docs.github.com/en/rest/releases/releases).

## Running

The package ships two entry points over one server core:

```sh
pnpm --filter @beale/app-server start            # tray-resident Electron host (Windows, macOS)
pnpm --filter @beale/app-server start:headless   # plain Node process
```

The tray host lives in the notification area (Windows) or menu bar (macOS). Its menu shows the endpoint and active sessions, can copy the URL and operator token, shows an iOS provisioning QR code, and quits the server, stopping its worker processes. Canonically active sessions are continued automatically the next time the app-server starts; sessions explicitly paused or stopped by a client are not. The tray host enforces a single instance per machine. The QR code carries a versioned `beale://connect` payload with the advertised origin and operator token, so it must only be shown to trusted devices on the operator's tailnet.

Both entry points write a discovery record so clients can find a running instance:

```
~/.beale/app-server.json
```

```json
{
  "version": 1,
  "contractTimestamp": "2026-08-24T18:30:00.000Z",
  "hostMode": "tray",
  "pid": 12345,
  "host": "127.0.0.1",
  "port": 54321,
  "url": "http://127.0.0.1:54321",
  "operatorToken": "...",
  "startedAt": "2026-08-21T00:00:00.000Z"
}
```

The headless entry refuses to start while a live instance is already recorded. `--check` starts the server, confirms the listener came up, then exits; use it to validate a machine's setup without leaving a process behind.

## Configuration

Options may be supplied as CLI flags (`--host`, `--port`, `--state-file`) or environment variables:

| Environment variable | Meaning |
| --- | --- |
| `BEALE_APP_SERVER_HOST` | Bind address (default `127.0.0.1`; bind the host's Tailscale IPv4 address for tailnet clients) |
| `BEALE_APP_SERVER_PORT` | TCP port (default ephemeral) |
| `BEALE_APP_SERVER_PUBLIC_URL` | Optional public HTTP(S) origin advertised to clients, such as a Tailscale Serve HTTPS origin |
| `BEALE_APP_SERVER_TOKEN` | Explicit operator-token override |
| `BEALE_APP_SERVER_STATE_FILE` | Discovery record path (default `~/.beale/app-server.json`) |
| `BEALE_APP_SERVER_ICON` | Optional tray-icon override (transparent SVG on macOS; PNG elsewhere) |

Beale Desktop persists its managed remote-access choice in `~/.beale/app-server-remote-access.json`. When enabled, Desktop launches the app-server on the stable loopback port `47173`, advertises the configured MagicDNS origin on the dedicated Tailscale Serve HTTPS port `47174`, and continues using the discovery record's `localUrl` for host-local control traffic. Explicit `BEALE_APP_SERVER_*` environment variables retain precedence for custom deployments.

## Authentication model

Two token scopes exist:

- **Operator token** guards the control plane: workspace and canonical reads plus session create, list, stop, and shutdown. Local clients read it from the discovery record; remote clients need an operator-controlled provisioning channel.
- **Per-attachment session tokens** guard each session's WebSocket transport. The launch response receives the first token, and operator-authenticated clients mint independent tokens when joining an active session.

Unless `BEALE_APP_SERVER_TOKEN` is set, the operator token is generated once in the private sibling file `~/.beale/app-server.token` and reused across graceful app-server restarts so provisioned mobile clients remain valid. The discovery record is also created with private file permissions because it contains the same token. Delete the token file while the app-server is stopped to rotate it. `GET /health` is unauthenticated. Binding beyond loopback changes who can reach these endpoints: network authorization belongs to the operator-managed boundary (for example Tailscale ACLs).

For iOS, the recommended deployment is a loopback listener behind Tailscale Serve. Use Desktop **Settings > Remote** to detect the machine's MagicDNS name and manage a dedicated HTTPS listener without replacing other Serve routes. The launch response contains a relative WebSocket path, so clients resolve it as `wss:` from that configured HTTPS origin; app-server code does not terminate TLS or manage Tailscale identity.

## HTTP control surface

- `GET /health` — liveness and compatibility probe; returns `ok`, the UTC control-contract timestamp, and the capability list. An older Desktop prompts for restart; a newer Desktop replaces an older app-server automatically.
- `GET /v1/server` — authenticated, typed server descriptor with control/protocol versions, endpoints, capabilities, and payload/replay limits.
- `GET /v1/providers` — authenticated, path-free model catalogs for providers connected in Desktop, including host Lead/subagent/reasoning defaults but no credentials or authentication metadata.
- `POST /v1/operations` — execute an allowlisted canonical app-server operation inside the app-server host. Research clients can use `suggestion.generate` for profile-default workspace suggestions and `prompt.expand` for bounded model-assisted context expansion; the host supplies workspace storage, provider policy, and credentials. Campaign-track clients can use `investigation.list`, `investigation.get`, and `investigation.replay`; disabled-memory workspaces reject investigation and Dreaming operations. Codex integrations use `research.tools.list`, `research.tools.read`, and `research.tools.mutate` to discover and invoke the same profile-scoped durable research tools as hosted agents; the host replaces client-supplied paths and profile policy with registered workspace values and enforces read-versus-mutating routing.
- `POST /v1/sessions` — launch a session. Body:

  ```json
  {
    "launchVersion": 2,
    "sessionId": "session-1",
    "launch": {
      "workspaceId": "workspace_abc123",
      "attemptId": "attempt_abc123",
      "promptMarkdown": "Research request",
      "provider": {
        "id": "openai-codex",
        "model": "gpt-5.6-sol",
        "reasoningEffort": "high"
      },
      "shellSafetyMode": "auto_review",
      "researchProfileId": "security-research",
      "researchProfileHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "workflowId": "discovery",
      "generateTitle": true
    }
  }
  ```

  The shared request and response DTOs and decoders live in `@beale/app-server-runtime/protocol`. Optional typed sections cover goals, logical continuation intent and collaboration. Filesystem paths, storage locations, CLI arguments, plugins, provider policy, arbitrary environment variables, and credentials are not accepted from clients. `sessionId` is optional and must match `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`. A completed session id may be reused; an active one rejects duplicates with `409`. The `201` response contains `controlVersion`, the session catalog entry, `attemptId`, and a transport descriptor with a relative `path`, protocol/authentication metadata, replay semantics, and the per-session token.
- `GET /v1/sessions` — typed catalog of known sessions with state (`starting`, `running`, `completed`, `failed`, `stopped`), timestamps, exit codes, client attachment, replay-buffer counts, and a bounded diagnostic for failed app-server exits. Terminal sessions are retained up to 50 entries; `DELETE` removes them.
- `GET /v1/sessions/<id>` — one typed live-process catalog entry, avoiding a full catalog poll.
- `POST /v1/sessions/<id>/attachments` — mint an independent transport token for another Desktop or mobile client to join an active session. Terminal sessions return `410`.
- `POST /v1/sessions/<id>/continuations` — continue a canonical blocked, completed, failed, or stopped session. The authenticated body supplies `workspaceId` and the new `instruction`; app-server appends a child attempt to the same session, preserves its original request and transcript, and returns a fresh session transport.
- `POST /v1/sessions/<id>/control` — send an authenticated `steer`, `pause`, `resume`, or `stop` control without opening a WebSocket attachment. The app-server assigns the correlation id and routes the validated control through the same hosted worker boundary.
- `DELETE /v1/sessions/<id>` — stop a running session (`202`) or remove a retained terminal record (`200`). Unknown ids return `404`.
- `POST /v1/server/shutdown` — authenticated graceful process-host shutdown used by Desktop when replacing an older app-server.

Non-success responses use one bounded shape: `{"controlVersion":1,"error":{"code","message","retryable"}}`.

Canonical, path-free host reads use control contract v1:

- `GET /v1/workspaces`
- `GET /v1/workspaces/<workspace>/channels` — list durable workspace research channels with member/message counts and the latest bounded preview
- `POST /v1/workspaces/<workspace>/channels` — create a workspace research channel
- `GET /v1/workspaces/<workspace>/channels/<channel>` — read channel metadata, members, and a bounded transcript
- `POST /v1/workspaces/<workspace>/channels/<channel>` — append a human-authored channel message
- `DELETE /v1/workspaces/<workspace>/channels/<channel>` — explicitly delete the channel and its transcript
- `GET /v1/workspaces/<workspace>/memory` — path-free workspace memory catalog resolved through the workspace's durable research subject and active profile, with identity, type, status, confidence, tags, title/summary, timestamps, revisions, and session links; no bodies, evidence, attributes, or host paths
- `GET /v1/workspaces/<workspace>/memory-notifications` — up to 500 newest heat-bearing memory identities, types, statuses, heat levels, title/summaries, timestamps, revisions, and session links only; no bodies, evidence, attributes, or host paths
- `GET /v1/workspaces/<workspace>/sessions`
- `GET /v1/workspaces/<workspace>/sessions/<session>/update`
- `GET /v1/workspaces/<workspace>/sessions/<session>/events` — bounded `all`, `transcript`, `trace`, or `commentary` streams; `commentary` combines transcripts with path-safe tool labels and omits raw tool inputs/results
- `POST /v1/workspaces/<workspace>/sessions/<session>/event-details`
- `GET /v1/workspaces/<workspace>/sessions/<session>/collaboration`
- `GET /v1/workspaces/<workspace>/sessions/<session>/captures`

The app-server resolves `<workspace>` through the shared Beale registry. Session reads verify that the canonical session belongs to that workspace. Responses identify the workspace without exposing its path and wrap the app-server result with `controlVersion: 1`.

## Session transport

Clients resolve the returned `/v1/sessions/<sessionId>/transport` path against their app-server origin, replace `http` with `ws` or `https` with `wss`, authenticate with `Authorization: Bearer <per-attachment-token>`, and speak the standard app-server protocol v1 envelope stream. Every connection sends `client.hello` first; the app-server then routes controls directly to the hosted worker and broadcasts its events.

Desktop and mobile clients may remain attached concurrently, send independently correlated controls, and receive the same live event stream. Disconnecting one client does not stop app-server or interrupt the others. The app-server owns `server.hello` and buffers up to 256 hosted event frames or 4 MiB only while no handshaken client is attached, dropping the oldest frames on overflow and reporting counts in the session catalog. Reconnecting clients complete the handshake, receive buffered events, then use canonical cursor reads to reconcile if `droppedFrames` is nonzero.

At startup, the app-server marks only canonically active attempts from its prior process as interrupted and relaunches eligible sessions as child attempts before publishing its discovery record. The restart launch descriptor contains typed, host-resolved session intent but no credentials, host paths, attachment tokens, or introspection secrets. Accepted pause and stop controls transition canonical state immediately, so those sessions are excluded from startup recovery. Older sessions without a restart descriptor and secret-bearing introspection sessions remain safely paused for manual handling.

Missing or invalid attachment tokens are rejected with `401`; ended sessions are rejected with `410`. Operator authentication and the operator-managed network boundary control who may mint additional attachment tokens.

On Windows and macOS, Desktop launches the Electron tray host for normal use. Any compatible non-tray instance started implicitly is replaced with the tray host, so an early operation-client or older invisible headless process cannot suppress the menu-bar or notification-area icon. Explicit custom launchers are preserved. Windows center-crops the bitmap source before tray scaling. macOS rasterizes Desktop's transparent monochrome SVG into 1x and 2x template-image representations so the menu-bar icon keeps its silhouette and remains visible in both appearances. Explicit headless Node callers and tests continue to launch `headlessMain.js`.

On macOS, the shared Beale Electron bundle declares why it needs Local Network access. Allow that system permission so app-server-hosted research sessions can reach operator-authorized local VMs and research targets; child tools such as `ssh` inherit the app-server's local-network policy.

## Development

```sh
pnpm build            # tsc -b from the repository root builds this package
pnpm test:app-server  # node:test suite; requires built packages
```
