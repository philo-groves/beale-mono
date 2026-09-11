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
| `integrations/beale-codex` | Codex plugin with a Beale research skill and local stdio MCP server |
| `packages/research-agent` | `@beale/research-agent` — workspace context, durable memory, tools, and the Pi-backed agent runtime |
| `tests` | app-server test suite (`node:test`, runs against built packages) |
| `examples` | Example research profiles |
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
- **No managed sandbox.** All of these programs run with the current user's host privileges. Launch them inside your own VM or container when OS isolation is required.

Workspace memory is independently selectable in Desktop's Workspace Overview:

- **Enabled** uses one canonical research system: concise knowledge memory, a stable claim ledger projected as Leads and Findings, reversible duplicate coalescing, durable runbooks and campaign tracks, and one workspace-history search across those records.
- **Disabled** removes memory behavior from new sessions while retaining stored data.

Profiles define domain classifications such as `security.primitive`, `security.chain`, `mathematics.theorem`, and `mathematics.counterexample`; they do not create competing memory backends. Legacy v1/v2/shadow selections migrate to Enabled, and legacy claim-shaped memory rows migrate non-destructively into the claim ledger.

During development, Desktop discovers and launches the workspace app-server. `BEALE_APP_SERVER_COMMAND` and related environment variables override this for packaged builds and custom setups.

### Codex integration

Install `integrations/beale-codex` as a local Codex plugin and start Beale or the app-server before using it. The plugin reads the private `~/.beale/app-server.json` discovery record internally, uses its loopback endpoint by default, and exposes path-free workspace/session controls plus the active research profile's durable tools. Codex-native filesystem, terminal, browser, computer-use, and approval behavior remains the execution boundary; research results written through the plugin immediately enter Beale's canonical app-server storage.

---

## Research workspaces

New Beale workspaces use one dedicated research directory. Git must be available on the app-server host. Creation initializes local history with no remote; remote configuration and synchronization remain operator-controlled. Existing workspaces can be opened as reference material without automatic layout conversion.

The root holds `AGENTS.md`, optional `AGENTS.override.md`, `README.md`, `.gitignore`, and `workspace.json`. Research lives in `investigations/`, `runbooks/`, `reports/`, `evidence/`, `references/`, `memories/`, `claims/`, and `traces/`. Keep candidate code and fixtures with their investigation, and reusable procedures with their runbook. Source repositories stay in the user-global repository store. `scratch/` holds disposable session work and `cache/` holds rebuildable resources; both are excluded from Git. Compatibility/runtime files under `.beale/` also remain untracked. A managed root allowlist ignores misplaced top-level entries, while a direct filesystem guard—not Git status—reactivates the root agent every turn until it classifies them into an approved directory.

New schema-v2 workspaces declare `researchAuthority: "files"`. Claims, memory Markdown, notebooks, reports, investigation state, scope records, evidence manifests, and session summaries are maintained as the primary workspace record and versioned in local Git. App-server keeps a derived SQLite query index synchronized through the same typed validators used by research tools. Schema-v1 workspaces remain database-authoritative and can still create an explicit compatibility export; they are not converted automatically.

App-server reconciles and checkpoints eligible changes before session launch, at research milestones, every ten minutes during a session, after worker exit, and immediately after typed research mutations. In file-authority workspaces, a checkpoint validates supported direct edits, updates the derived index, republishes the complete file snapshot, and commits it. Git and synchronization run off the control-plane event loop. A pre-session checkpoint failure prevents launch; later failures preserve research and appear in checkpoint status and session diagnostics. Manual staged changes are preserved for the operator to commit or unstage. Checkpoints never push, stash, reset the working tree, or rewrite history.

Commit messages end with `Investigation-ID: <id>` and `Session-ID: <id>` trailers. Session checkpoints use the requested investigation or resolve the session's latest canonical investigation link. Unbound IDs, workspace creation, and operator-only checkpoints use `none`. The managed commit-message hook supplies missing trailers for manual commits and preserves explicitly supplied pairs; supply actual IDs when manually committing research. Filter history with `git log --grep='^Session-ID: session-example$'` or the corresponding investigation trailer. Existing history is not rewritten.

The managed pre-commit hook validates staged layout, size limits, credential/database exclusions, canonical publication hashes, and retained evidence. These checks are product integrity controls, not OS isolation. Managed file overwrites retain recovery copies. New-workspace Dejunk checkpoints first and quarantines scratch/cache content under `.git/beale/quarantine/`, with a move journal; it does not delete research by filename heuristics. Terminal session cleanup quarantines that session's scratch after a successful checkpoint. Quarantine retains disk space until the operator removes it.

Use typed research tools for normal changes; successful durable mutations trigger immediate background synchronization of the file-authority snapshot. `workspace.project sync` explicitly reconciles supported direct edits, while routine checkpoints do the same automatically. Direct imports support claim prose, memory prose, report content, and existing runbook cell sources; evidence, ownership, execution results, and claim status cannot be forged through file edits. Runbook source imports create a new content revision. Raw evidence is retained under `evidence/raw/` with verified hashes and full event exports are paged JSONL under `traces/<session-id>/`; both stay outside Git, so Git history alone is not a raw-evidence or live-session backup.

`workspace.search` defaults to the active workspace. Its input schema advertises the exact IDs and names of registered workspaces that app-server has verified share the active research Subject, even when a schema-v2 reference has released its derived SQLite rows. Selecting one searches it read-only and omits its host path from results. `history.search scope=subject` separately discovers compact prior canonical records currently loaded in the query index.

After a clean terminal schema-v2 checkpoint, app-server removes the workspace's rebuildable research rows from SQLite when no other session is active there. Sessions, authorization scope, workspace/session bindings, and runtime coordination remain in the app-server database. Operators can also use `workspace.project release-index`; `workspace.project rebuild-index` restores the query index from hash-validated canonical files, and ordinary operations do the same automatically when the index is released. Releasing rows makes SQLite pages reusable but does not run `VACUUM` or promise an immediate reduction in the database file's physical size.

## Development

Requirements: Node.js >= 22.19.0 and pnpm 11 (see `packageManager` in `package.json`).

```sh
pnpm install
```

Dependency build scripts are denied by default; `electron`, `node-pty`, and `@mediar-ai/terminator` are explicitly allowed because they need native install steps. See `pnpm-workspace.yaml`.

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
