# Managed Agent Plugins

This directory contains Agent Plugins maintained with Beale but kept separate from the built-in plugin set shipped by the app-server. Each immediate child is an independent, portable package that follows [Agent Plugins 1.0.0](https://agent-plugins.org/specification).

Managed plugins are not enabled automatically. In Beale, open Plugins, choose **Add Agent Plugin**, and select the individual plugin directory. For example, select `managed-plugins/apple-security-devices`, not this collection directory.

- [Apple Security Devices](apple-security-devices/README.md): Select and inspect Apple research environments, including Tart, physical iPhones, and Darwin VM.
- [Apple Target Flags](apple-target-flags/README.md): Interpret Apple Target Flag evidence and its limits.
- [Microsoft Security Devices](microsoft-security-devices/README.md): Prefer a Canary Hyper-V guest for Windows validation and check guest build freshness on the first VM touch in each session.

Every managed plugin must:

- keep its portable manifest at `plugin.json` and MCP configuration at `mcp.json`;
- keep each Agent Skill in an immediate child of `skills/`;
- resolve package-supplied files within its own plugin root;
- avoid credentials, device identifiers, machine-specific paths, and research-target data;
- treat host mutation and target execution as explicit, confirmable operations.

