# Beale

**Not a coding agent; a decoding agent.**

![Screenshot](https://i.ibb.co/qL9PDnGt/beale-sc.jpg)

This monorepo contains:

- **Beale Desktop** (`apps/desktop`) — an Electron-based desktop workbench for authorized vulnerability research.
- **Beale iOS** (`apps/ios`) — a basic native SwiftUI client for connecting to the app-server through tailnet-only Tailscale Serve HTTPS.
- **Beale research runtime** (`packages/research-agent`, `packages/app-server-runtime`) — the research engine plus its shared protocol and app-server-hosted worker runtime.
- **App Server** (`app-server`) — the standalone tray-resident app-server host and common control plane for Desktop and iOS.
- **Integrations** (`integrations`) — first-party client integrations, including the local `beale-codex` Codex plugin.

---

## Status

**Very early stage / pre-alpha.**

The project is under heavy development. The agent is not ready for real use; expect incomplete, unstable behavior and frequent change. Feedback and ideas are welcome, but large changes are best discussed in an issue first.

---

## Repository Layout

| Path | Contents |
| --- | --- |
| `apps/desktop` | `@beale/desktop` — Beale Electron workbench (React + TypeScript, electron-vite) |
| `apps/ios` | Native SwiftUI app-server client and Xcode project |
| `packages/app-server-runtime` | Shared protocol, app-server session runtime, and optional app-server client |
| `app-server` | `@beale/app-server` — standalone tray-resident app-server execution host and control plane |
| `app-server/resources/harness-features` | Built-in harness feature manifests and Introspection runtime |
| `managed-plugins` | Portable Agent Plugin packages, including Meta Skills |
| `integrations/beale-codex` | Codex plugin with a Beale research skill and local stdio MCP server |
| `packages/research-agent` | `@beale/research-agent` — workspace context, durable memory, tools, and the Pi-backed agent runtime |
| `tests` | app-server test suite (`node:test`, runs against built packages) |
| `patches` | pnpm patches for Pi dependencies |
| `planning` | app-server architecture notes |

Component documentation:

- [`apps/desktop/README.md`](apps/desktop/README.md) — Beale product overview, architecture, and execution notes.
- [`apps/ios/README.md`](apps/ios/README.md) — iOS build, Tailscale Serve, authentication, and security notes.
- [`packages/app-server-runtime/README.md`](packages/app-server-runtime/README.md) — shared app-server host-runtime and protocol boundary.
- [`app-server/README.md`](app-server/README.md) — App Server control surface and session transport.
- [`planning/ARCHITECTURE.md`](planning/ARCHITECTURE.md) — app-server runtime architecture and ownership boundaries.

---

## How the Pieces Fit Together

- **Beale is the trusted host harness.** It owns workspace setup, authorization recording, provider credentials, tool-family and side-effect grants, shell policy, and presentation.
- **The Beale research runtime is the research engine.** It owns context compilation, tool execution, durable-knowledge semantics, orchestration, live events, and flow captures. Records retain workspace, subject, and research-profile ownership.
- **Integration and runtime storage are server-mediated.** Desktop, iOS, and hosted research workers use the Beale app-server. Only the resident app-server process opens or writes the user-global SQLite database at `~/.beale/memory.sqlite`; schema-v2 workspace research is authoritative in workspace files, while SQLite supplies session state and a synchronized query index. No app-server CLI process or private loopback WebSocket sits between the server and engine.
- **The App Server is the client-neutral host.** It fans worker events out over per-session authenticated WebSockets and accepts correlated controls from multiple attached clients.
- **Fleet VM execution.** A primary app-server can clone operator-prepared Tart or Hyper-V base VMs and run research in a guest Beale instance over SSH. Fleet can also select VMs on another primary app server in the same Tailscale network while returning the session to its launching workspace. Interrupted sessions reconnect to their recorded clone and preserve its guest workspace; completed clones remain available for later continuation. Local sessions still use the current user's host privileges; Fleet depends on the VM's isolation rather than an application sandbox.
- **Fleet browser relay.** Fleet sessions keep their browser page, profile, and page requests in the VM. Desktop displays compressed frames in the session browser tab and relays researcher input to that page. The guest uses an installed Chrome or Edge browser, or downloads Chrome for Testing on first browser use.

Workspace memory is independently selectable in Desktop's Workspace Overview:

- **Enabled** uses one canonical research system: concise knowledge memory, a stable claim ledger projected as Leads and Findings, reversible duplicate coalescing, durable runbooks, and one workspace-history search across those records.
- **Disabled** removes memory behavior from new sessions while retaining stored data.

Beale runs authorized security research. Security claims distinguish leads, isolated findings (`security.primitive`), and demonstrated exploit chains (`security.chain`). Legacy v1/v2/shadow memory selections migrate to Enabled, and legacy claim-shaped memory rows migrate non-destructively into the claim ledger.

During development, Desktop discovers and launches the workspace app-server. `BEALE_APP_SERVER_COMMAND` and related environment variables override this for packaged builds and custom setups.

### Codex integration

Install `integrations/beale-codex` as a local Codex plugin and start Beale or the app-server before using it. The plugin reads the private `~/.beale/app-server.json` discovery record internally, uses its loopback endpoint by default, and exposes path-free workspace/session controls plus the active research profile's durable tools. Codex-native filesystem, terminal, browser, computer-use, and approval behavior remains the execution boundary; research results written through the plugin immediately enter Beale's canonical app-server storage.

---

## Research workspaces

New Beale workspaces use one dedicated research directory. Creation does not initialize Git or require Git on the app-server host. Existing workspace Git history remains intact and can be managed by the operator. Existing workspaces can also be opened as reference material without automatic layout conversion.

The root holds `AGENTS.md`, optional `AGENTS.override.md`, `README.md`, and `workspace.json`. Research lives in `investigations/`, `runbooks/`, `reports/`, `evidence/`, `references/`, `memories/`, `claims/`, and `traces/`. Keep candidate code and fixtures with their investigation, and reusable procedures with their runbook. Source repositories stay in the user-global repository store. `scratch/` holds disposable session work and `cache/` holds rebuildable resources. A direct filesystem guard reactivates the root agent every turn until unexpected top-level entries are classified into an approved directory.

New schema-v2 workspaces declare `researchAuthority: "files"`. Claims, memory Markdown, notebooks, reports, scope records, evidence manifests, and session summaries are primary workspace records. App-server keeps a derived SQLite query index synchronized through the same typed validators used by research tools. Publication metadata and recovery copies live under `.beale/publication/`; existing workspaces retain their prior metadata location. Schema-v1 workspaces remain database-authoritative and can still create an explicit compatibility export.

Research sessions no longer require or create Git checkpoints. Typed research mutations refresh the canonical file publication and derived query index without a Git commit. `workspace.project sync` explicitly reconciles supported direct edits. Direct imports support claim prose, memory prose, report content, and existing runbook cell sources; evidence, ownership, execution results, and claim status cannot be forged through file edits. Runbook source imports create a new content revision. Raw evidence is retained under `evidence/raw/` with verified hashes and full event exports are paged JSONL under `traces/<session-id>/`.

Workspace Dejunk quarantines disposable scratch/cache content under `.beale/quarantine/` with a move journal. It does not delete research by filename heuristics. Quarantine retains disk space until the operator removes it.

`workspace.search` defaults to the active workspace. Its input schema advertises the exact IDs and names of registered workspaces that app-server has verified share the active research Subject, even when a schema-v2 reference has released its derived SQLite rows. Selecting one searches it read-only and omits its host path from results. `history.search scope=subject` separately discovers compact prior canonical records currently loaded in the query index.

Operators can use `workspace.project release-index` to remove rebuildable research rows from SQLite while retaining sessions, authorization scope, workspace/session bindings, and runtime coordination. `workspace.project rebuild-index` restores the query index from hash-validated canonical files, and ordinary operations do the same automatically when the index is released. Releasing rows makes SQLite pages reusable but does not run `VACUUM` or promise an immediate reduction in the database file's physical size.

## Development

Requirements: Node.js >= 22.19.0 and pnpm 11 (see `packageManager` in `package.json`).

```sh
pnpm install
```

Dependency build scripts are denied by default; `electron` and `node-pty` are explicitly allowed because they need native install steps. See `pnpm-workspace.yaml`.

### Research runtime and app-server

```sh
pnpm build            # tsc -b across runtime packages and app-server
pnpm check            # typecheck only
pnpm start            # start the app-server tray host
```

The app-server is the research runtime's execution host. Any retained command-line adapter is only a client of its authenticated `/v1/operations` surface.

### Test tiers

```sh
pnpm test                         # default fast monorepo gate
pnpm test:integration             # hosted-worker and real-session Desktop coverage
pnpm test:all                     # both tiers
pnpm test:runtime:fast            # isolated research-runtime tests
pnpm test:runtime:integration     # hosted-runtime integration cases
```

Keep isolated logic, protocol, persistence, and host-policy checks in the fast tier. Use the integration tier when a hosted worker or Desktop/app-server session boundary is essential.

The tier split and integration consolidation produced the following same-machine Windows timings on 2026-08-22:

| Suite | Before | After | Change |
| --- | ---: | ---: | ---: |
| Desktop integration | 436.09 s | 125.53 s | 71% faster |
| Desktop research-profile integration | 259.58 s | 65.38 s | 75% faster |
| Profile snapshot replacement case | 82.50 s | 29.16 s | 65% faster |

### App Server

```sh
pnpm build            # built together with the packages by tsc -b
pnpm --filter @beale/app-server start            # tray host (Windows, macOS)
pnpm --filter @beale/app-server start:headless   # plain Node process
pnpm test:app-server  # node:test suite against the hosted runtime
```

See `app-server/README.md` for configuration, the discovery record at `~/.beale/app-server.json`, and the operator/per-session token model.

### Beale Desktop (apps/desktop)

```sh
pnpm --filter @beale/desktop dev          # Electron dev mode
pnpm --filter @beale/desktop build        # typecheck + production bundle
pnpm --filter @beale/desktop start        # run the built app
pnpm --filter @beale/desktop typecheck
pnpm --filter @beale/desktop test         # unit tests (vitest)
```

The desktop integration tests (`pnpm --filter @beale/desktop test:integration`) exercise real app-server sessions and require the packages to be built first. `pnpm --filter @beale/desktop test:fast` is an explicit alias for the unit tier.

Live provider tests remain opt-in because they require local credentials.

---

## Safety

This tool is intended **only** for authorized vulnerability research and testing. Always respect scope, legal boundaries, and responsible disclosure practices. Because the project is pre-alpha, policy and isolation safeguards are incomplete; operator-managed VMs, containers, firewalls, and proxies are the isolation boundary.

Development rules, terminology, and security-model invariants live in [`AGENTS.md`](AGENTS.md). Product changes are recorded in [`CHANGELOG.md`](CHANGELOG.md).

---

## License

MIT. See [`LICENSE`](LICENSE).
