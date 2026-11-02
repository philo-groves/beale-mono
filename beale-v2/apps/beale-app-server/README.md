# app-server

The app-server is the primary processing component of Beale. Each app client connects to one or more app-servers, and app-servers may also communicate with each other. The app-server runtime contains the full agent functionality to be shared across app clients.

## First Rust slice

`beale-core` is a runnable host. `cargo run -p beale-core -- serve` binds `127.0.0.1:43831` by default and serves `GET /v1/health` as `application/x-protobuf`. Set `BEALE_V2_APP_SERVER_ADDR` to another loopback IP and nonzero port when needed; non-loopback bindings are rejected. The desktop uses the same address and validates the service name and protocol version. The v1.1 protobuf contract adds authenticated local provider status, subscription sign-in start/forget, and API key save/remove operations. Its schema is `beale-contracts/proto/beale/v1/host.proto`; the matching Rust `prost` type is kept in source because the bundled Windows `protoc` cannot run in this development environment. The app-server creates a random control token in its private host directory, which the same-device desktop reads for these operations. No remote listener or client inference operation is exposed yet. Client-to-host WebSockets are not part of this transport. See the [workspace run instructions](../../README.md) for the two-process flow.

`beale-providers` owns host-only OpenAI and xAI credentials and text inference. API-key requests use a saved host key when configured, falling back to `OPENAI_API_KEY` or `XAI_API_KEY`, and call the providers' Responses endpoints. Subscription requests use OpenAI's Sign in with ChatGPT flow on the public Responses API or xAI device authorization and the Grok Build chat proxy. The xAI subscription device client ID currently matches the one used by v1; it is isolated in the OAuth module for replacement with a Beale-specific registration. Provider routes centralize endpoint and wire-shape choices so additional adapters can be added without changing the host contract. All routes stream upstream responses and require a terminal success event. Model IDs are supplied by the caller instead of pinned in code.

From `beale-v2/`:

```text
cargo run -p beale-core -- serve
cargo run -p beale-core -- login-openai
cargo run -p beale-core -- login-xai
cargo run -p beale-core -- infer openai subscription <model> < prompt.txt
cargo run -p beale-core -- infer xai api-key <model> < prompt.txt
```

Sign-in prints a provider URL or device code and saves OAuth credentials under the current user's `.beale/v2` directory (or `BEALE_V2_HOME`). On Unix files are created with mode `0600`; on Windows the host sets a user-only file ACL before replacing a saved record. API keys are read from the host environment and are never written by these commands. `infer` reads the prompt from standard input. Live provider calls require user-provided credentials and have not been exercised by the automated tests.

The CLI is a local provider verification path. This slice stores one OAuth registration per provider; account selection is not implemented. The app-server currently has no client-facing inference endpoint, agent runtime, durable research storage, client authentication, or Tailscale listener. These boundaries need their own contracts before they are connected.
