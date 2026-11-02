# v2

A complete re-implementation of Beale using lessons learned from the first. This directory will be moved to the root and replace its content when it is ready.

## Core Directories

### Apps

- `apps/beale-app-server/`: A general agent runtime server shared across app clients
- `apps/beale-desktop-app/`: An installable app client for Windows, macOS, and Linux
- `apps/beale-ios-app/`: An app client for iOS, not yet available on the App Store

### Extensions

- `extensions/features/`: Optional native features with deep app integrations
- `extensions/plugins/`: Optional skills and MCP servers using Agent Plugins 1.0
- `extensions/research-kits/`: Optional helpers to streamline certain programs

### Integrations

- `integrations/beale-for-codex/`: A third-party integration plugin for Codex
- `integrations/beale-for-claude-code/`: A third-party integration plugin for Claude Code
- `integrations/beale-for-cursor/`: A third-party integration plugin for Cursor

### Utilities

- `utilities/scripts/`: Scripts to assist with release, build, and other workflows
- `utilities/site/`: Basic utilities and assets for the beale.ai website