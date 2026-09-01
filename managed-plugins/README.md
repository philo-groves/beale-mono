# Managed Agent Plugins

This directory contains Agent Plugins maintained with Beale but kept separate from the built-in plugin set shipped by the app-server. Each immediate child is an independent, portable package that follows [Agent Plugins 1.0.0](https://agent-plugins.org/specification).

Managed plugins are not enabled automatically. In Beale, open Plugins, choose **Add Agent Plugin**, and select the individual plugin directory. For example, select `managed-plugins/apple-security-devices`, not this collection directory.

Every managed plugin must:

- keep its portable manifest at `plugin.json` and MCP configuration at `mcp.json`;
- keep each Agent Skill in an immediate child of `skills/`;
- resolve package-supplied files within its own plugin root;
- avoid credentials, device identifiers, machine-specific paths, and research-target data;
- treat host mutation and target execution as explicit, confirmable operations.

