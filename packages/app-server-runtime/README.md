# app-server host runtime

This package contains the shared app-server protocol and the session runtime hosted by `@beale/app-server`. It is a library boundary, not an execution host.

The app-server imports the runtime into isolated workers and connects controls and events with direct worker messages. Worker research stores use a private synchronous mediator instead of opening SQLite; the resident app-server owns the connections and validates the session's database path. Canonical session, memory, artifact, provider, plugin, source, and maintenance operations execute inside the app-server process against app-server-owned services.

The optional `appServer` executable is only a compatibility client for the authenticated app-server operation endpoint. It does not launch research sessions or open app-server storage itself. New Desktop and iOS functionality should use the app-server HTTP/WebSocket contracts directly.

The research engine, durable memory, tools, provider adapters, and orchestration remain in `@beale/research-agent`.

## Development

```sh
pnpm build
pnpm --filter @beale/app-server start
pnpm test:app-server
```
