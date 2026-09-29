# Managed Agent Plugins

This directory contains independent Agent Plugin packages maintained with Beale. Each immediate child follows [Agent Plugins 1.0.0](https://agent-plugins.org/specification).

Beale registers Introspection and Terminator from this directory as built-in plugins. Introspection is enabled by default; Terminator is disabled by default. The remaining managed plugins are opt-in: in Beale, open Plugins, choose **Add Agent Plugin**, and select an individual plugin directory such as `managed-plugins/apple-security-devices`, not this collection directory.

- **Beale Introspection** (`beale-introspection`): Workspace and session tools used by Quick Chat.
- **Beale Terminator** (`beale-terminator`): Optional Windows computer-use tools.
- [Apple Security Devices](apple-security-devices/README.md): Select and inspect Apple research environments, including Tart, physical iPhones, and Darwin VM.
- [Apple Target Flags](apple-target-flags/README.md): Interpret Apple Target Flag evidence and its limits.
- [Microsoft Security Devices](microsoft-security-devices/README.md): Prefer a Canary Hyper-V guest for Windows validation and check guest build freshness on the first VM touch in each session.

Every managed plugin must:

- keep its portable manifest at `plugin.json` and, when it provides MCP servers, configuration at `mcp.json`;
- keep each Agent Skill in an immediate child of `skills/`;
- resolve package-supplied files within its own plugin root;
- avoid credentials, device identifiers, machine-specific paths, and research-target data;
- treat host mutation and target execution as explicit, confirmable operations.
