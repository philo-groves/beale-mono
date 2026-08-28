# Beale Monorepo Development Rules

## Project Scope

This repository contains:

- **Beale Desktop** (`apps/desktop`): an authorized vulnerability research workbench and trusted Electron host harness.
- **Beale iOS** (`apps/ios`): developed in its dedicated repository; this directory reserves the workspace slot.
- **Honeycrisp** (`packages/research-agent`, `packages/honeycrisp-host`): the general research engine plus its shared app-server runtime and versioned client protocol.
- **App Server** (`app-server`): the tray-resident Honeycrisp execution host and client-neutral HTTP/WebSocket control plane.

The implementation, tests, READMEs, and changelog define the current product state.

Do not use legacy branding in new docs or code. Use `Beale`.

## Repository Layout

- `apps/desktop` — `@beale/desktop`. Electron app. Trusted main process (`src/main`), preload bridge (`src/preload`), React renderer (`src/renderer`), shared contracts (`src/shared`), vitest suites (`tests`).
- `apps/ios` — reserved for the Beale-iOS application, which lives in its dedicated repository.
- `packages/research-agent` — `@honeycrisp/research-agent`: workspace context, durable memory, tools, research profiles, and the Pi-backed agent runtime.
- `packages/honeycrisp-host` — shared `honeycrisp/protocol`, app-server worker runtime, and optional app-server client.
- `app-server` — `@beale/app-server`: the standalone tray-resident process (Electron host on Windows/macOS, headless Node otherwise) that hosts Honeycrisp workers, executes canonical operations, and publishes `~/.beale/app-server.json`.
- `tests` — Honeycrisp `node:test` suite; imports built output from `../packages/honeycrisp-host/dist/` and `../packages/research-agent/dist/`.
- `examples` — example research profiles. `patches` — pnpm patches. `planning` — architecture notes.
- Package manager is pnpm (`packageManager` in root `package.json`). Node >= 22.19.0.
- Dependency build scripts are denied by default; allowed builds are listed in `pnpm-workspace.yaml`. Update that list deliberately when adding native dependencies.

## Communication Style

- Keep responses concise and technical.
- Avoid fluff.
- No emojis in commits, issues, docs, comments, or code.
- Prefer direct implementation notes over broad speculation.

## Documentation Rules

- Keep documentation aligned with implemented behavior and tested boundaries.
- Do not add speculative planning, roadmap, or architecture documents unless explicitly requested.
- Keep terminology consistent:
  - Product name: `Beale`
  - Workspace metadata directory: `.beale/`
  - Global database: `~/.honeycrisp/memory.sqlite` (Honeycrisp-owned and shared with Beale; records retain workspace ownership)
  - Workspace registry: user-global metadata for known Beale workspaces
  - Authorized scope: the recorded authorization boundary within a workspace
  - First release focus: authorized open-ended vulnerability discovery
  - Execution posture: Beale and Honeycrisp run with the current user's host privileges; users should launch them inside their own VM/container when OS isolation is required

## CHANGELOG.md Management

- Maintain the root `CHANGELOG.md` for product, architecture, persistence, security model, and notable UX changes. Entries are grouped under `### Beale` and `### Honeycrisp` inside `## Unreleased`.
- Do not invent release versions or dates unless explicitly asked.
- Group entries under concise headings such as `Added`, `Changed`, `Fixed`, `Removed`, `Security`, and `Documentation` beneath the owning component.
- Add entries for user-visible behavior, beta-relevant fixes, schema or migration changes, model/tool contract changes, sandbox or networking changes, major refactors, and project structure changes.
- Do not add entries for trivial formatting, typo-only edits, test-only updates with no behavior change, or purely internal cleanup that does not affect future development.
- Keep entries short and factual. Mention migrations, compatibility notes, or manual setup steps when relevant.

## Security Model

Preserve these invariants in docs and implementation:

- Do not commit personal identifiers, real machine-local paths, device or account metadata, credentials, private endpoints, or real research targets and findings in code, tests, fixtures, examples, snapshots, or documentation.
- Use clearly synthetic identities and scenarios, `example` domains, and RFC 5737 address ranges whenever representative data is required.
- Beale is the trusted host harness.
- Target code, build scripts, generated PoCs, tests, fuzzing, debugging, and closed-source executables run with the user's chosen host privileges. Beale must not pretend to provide isolation it does not manage.
- OpenAI OAuth credentials stay on the host.
- The global database and credential material must not be exposed through model-visible tool results.
- Generated files and verifier outputs are candidate artifacts until accepted into durable Honeycrisp/Beale storage.
- Confirmed vulnerability conclusions require tool, artifact, or verifier-backed evidence references.
- User-provided vulnerability claims may seed Honeycrisp hypotheses; they are not target observations by themselves.
- Live-target testing is allowed only within the recorded authorized scope. Network isolation and destination controls belong to operator-managed system boundaries, not Beale application policy.

## Implementation Rules

- Inspect the current source, shared contracts, and relevant tests before changing a subsystem.
- Preserve the Honeycrisp-owned research engine and global database as the canonical runtime and persistence boundaries.
- Keep product behavior, shared types, IPC contracts, protocol DTOs, and tests synchronized across app/package boundaries.
- Beale locates Honeycrisp through workspace-root discovery (`resolveHoneycrispWorkspaceRoot`) or the `BEALE_HONEYCRISP_*` environment overrides. Do not reintroduce assumptions about a sibling checkout layout.
- Run transport and canonical operations go through the Beale app-server. Desktop and iOS are clients; they do not host the research runtime or open Honeycrisp storage.
- Protocol DTOs, constants, and decoders come from the `honeycrisp/protocol` package; `apps/desktop/src/main/honeycrispProtocol.ts` is only a re-export adapter plus Beale-side policy (required capabilities, bootstrap hardening, pinned client identity). Do not restate shared DTOs locally.
- Keep model-facing tools and durable semantics in `@honeycrisp/research-agent`; app-server owns host lifecycle, storage routing, provider policy, canonical operation dispatch, and client transport. Remote-client authorization belongs to the operator-managed network boundary.
- Do not introduce remote persistence, cloud sync, or cross-workspace global search unless explicitly requested.
- Do not add model-facing tools without updating their typed contracts and boundary tests.

## Code Quality

- Use TypeScript when implementation begins.
- Avoid `any` unless there is no reasonable alternative.
- Prefer typed boundaries between renderer, host service, model adapter, persistence, and executor layers.
- Use structured parsers/APIs instead of ad hoc string parsing when practical.
- Keep host-safe setup as narrow workspace/import operations, not general host shell execution.
- Keep target execution posture explicit. Recommend an externally launched VM/container for risky target code, but do not add Beale-managed permission gates or sandbox locks.

## Commands

Run from the repository root:

- `pnpm install` — install all workspaces.
- `pnpm build` / `pnpm check` / `pnpm clean` — Honeycrisp and app-server TypeScript project references.
- `pnpm test` / `pnpm test:fast` — default monorepo gate: fast Honeycrisp, app-server, and Desktop unit suites.
- `pnpm test:integration` — hosted Honeycrisp runtime and real-session Desktop suites.
- `pnpm test:all` — run both test tiers.
- `pnpm test:honeycrisp:fast` / `pnpm test:honeycrisp:integration` / `pnpm test:honeycrisp:all` — component-specific Honeycrisp tiers.
- `pnpm test:app-server` — app-server suite against the hosted runtime; requires built packages first.
- `pnpm --filter @beale/desktop typecheck` after Beale code changes.
- `pnpm --filter @beale/desktop test` / `test:fast` after behavior, boundary, persistence, or test changes.
- `pnpm --filter @beale/desktop test:integration` exercises real Honeycrisp sessions with bounded file concurrency and requires built packages first.
- Keep isolated logic, protocol, persistence, and host-policy coverage in the fast tier. Add an integration case only when a hosted worker or real session boundary is essential, and avoid repeating scenarios already covered below that boundary.
- Live provider tests remain opt-in and require user-provided credentials.
