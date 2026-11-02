# Beale v2 App Server

These instructions apply to `beale-v2/apps/beale-app-server/`. Follow the repository security rules in the top-level `AGENTS.md`. Its TypeScript layout and pnpm commands describe v1; use Rust and the v2 Cargo workspace here.

## Scope and ownership

- Build v2 incrementally from the behavior requested for each change. Treat v1 as a source of tested product behavior, not a crate-by-crate porting plan.
- The app-server is a trusted host process for desktop operating systems. It owns agent execution, host operations, and durable state. Desktop and iOS are clients.
- Keep versioned client and app-server peer contracts in `beale-contracts`. Keep host-only paths, credentials, database handles, and implementation types out of those contracts and model-visible results. Do not choose a wire format by implication from an internal Rust type.
- Keep crate dependencies acyclic. Put a shared type at the lowest boundary that owns its meaning; avoid making `beale-contracts` depend on host, provider, tool, or storage crates.
- Native optional features may use narrow, typed harness hooks when deep integration is necessary. Keep feature-specific behavior in its owning project and verify that the feature can be disabled. Plugins use the less integrated Agent Plugins path; do not force both extension kinds through one lifecycle.

## Rust changes

- When adding the first crate manifests, choose an edition and minimum Rust version deliberately and set the resolver explicitly in the virtual workspace root.
- Define explicit types at protocol, storage, provider, tool, and host boundaries. Validate untrusted inputs before they reach durable operations or target execution.
- Return errors with useful context inside the host and bounded, sanitized errors across client and model boundaries. Avoid panics and unchecked `unwrap` or `expect` on runtime inputs.
- Prefer safe Rust. When FFI or another unsafe operation is necessary, keep the unsafe block small, document its safety invariant, and expose a safe interface.
- Keep Cargo features additive. Do not assume `--all-features` proves optional-feature isolation: Cargo unifies features across a build.
- Use synthetic research data in tests, fixtures, logs, and docs. Never copy live workspace content into this tree. Preserve the recorded authorization boundary for live-target operations; the app-server does not provide OS isolation.

## Verification

- Inspect the affected crate, contract, and tests before changing behavior. Add focused tests for boundary or protocol changes; test both sides of a versioned contract together.
- Once member crates have manifests, run `cargo fmt --all -- --check` from `beale-v2/`, then `cargo check -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`, and `cargo test -p <crate>` for affected crates. Run broader workspace checks when shared contracts or dependencies change.
- For an optional feature, check its enabled and disabled configurations separately. Keep live provider and live-target tests opt-in with user-provided credentials and authorization.
- Update the relevant README and changelog when behavior or a public contract changes. Keep this file aligned with the crates and commands that actually exist.
