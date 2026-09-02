# Beale

**Not a coding agent; a decoding agent.**

![Screenshot](https://i.ibb.co/qL9PDnGt/beale-sc.jpg)

This monorepo contains:

- **Beale Desktop** (`apps/desktop`) — an Electron-based desktop workbench for authorized vulnerability research.
- **Beale iOS** (`apps/ios`) — a basic native SwiftUI client for connecting to the app-server through tailnet-only Tailscale Serve HTTPS.
- **Beale research runtime** (`packages/research-agent`, `packages/app-server-runtime`) — the research engine plus its shared protocol and app-server-hosted worker runtime.
- **App Server** (`app-server`) — the standalone tray-resident app-server host and common control plane for Desktop and iOS.

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
- **The Beale research runtime is the research engine.** It owns context compilation, tool execution, durable knowledge, storage, orchestration, live events, flow captures, and the user-global SQLite database at `~/.beale/memory.sqlite`. Records retain workspace, subject, and research-profile ownership.
- **Integration is server-mediated.** Desktop and iOS request sessions and canonical operations from the Beale app-server. The server hosts app-server workers directly and owns canonical storage access; no app-server CLI process or private loopback WebSocket sits between the server and engine.
- **The App Server is the client-neutral host.** It fans worker events out over per-session authenticated WebSockets and accepts correlated controls from multiple attached clients.
- **No managed sandbox.** All of these programs run with the current user's host privileges. Launch them inside your own VM or container when OS isolation is required.

Workspace memory is independently selectable in Desktop's Workspace Overview:

- **Enabled** uses one canonical research system: concise knowledge memory, a stable claim ledger projected as Leads and Findings, reversible duplicate coalescing, and durable campaign tracks.
- **Disabled** removes memory behavior from new sessions while retaining stored data.

Profiles define domain classifications such as `security.primitive`, `security.chain`, `mathematics.theorem`, and `mathematics.counterexample`; they do not create competing memory backends. Legacy v1/v2/shadow selections migrate to Enabled, and legacy claim-shaped memory rows migrate non-destructively into the claim ledger.

During development, Desktop discovers and launches the workspace app-server. `BEALE_APP_SERVER_COMMAND` and related environment variables override this for packaged builds and custom setups.

---

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
