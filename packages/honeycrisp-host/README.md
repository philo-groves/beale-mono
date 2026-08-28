# Honeycrisp host runtime

This package contains the shared Honeycrisp protocol and the session runtime hosted by `@beale/app-server`. It is a library boundary, not an execution host.

The app-server imports the runtime into isolated workers and connects controls and events with direct worker messages. Canonical session, memory, artifact, provider, plugin, source, and maintenance operations execute inside the app-server process against Honeycrisp-owned services.

The optional `honeycrisp` executable is only a compatibility client for the authenticated app-server operation endpoint. It does not launch research sessions or open Honeycrisp storage itself. New Desktop and iOS functionality should use the app-server HTTP/WebSocket contracts directly.

The research engine, durable memory, tools, provider adapters, and orchestration remain in `@honeycrisp/research-agent`.

## Development

```sh
pnpm build
pnpm --filter @beale/app-server start
pnpm test:app-server
```
