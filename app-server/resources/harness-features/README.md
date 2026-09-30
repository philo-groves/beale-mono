# Harness features

These packages are built-in capabilities controlled by Agent Settings > Optional Features. The six native research tool groups retain Agent Plugin manifests as registry compatibility adapters; their tool ownership and schemas live in `packages/research-agent/src/managed-tool-plugins.ts`. Introspection includes an MCP server for workspace and session operations. Enabled features appear to agents in `{{features}}` under "Internal features" and load through `features.preview` and `features.load`.

The registry IDs remain stable so existing saved enabled states and session captures continue to work.
