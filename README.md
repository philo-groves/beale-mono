<p align="center">
    <img align="center" width="160" height="160" src="https://github.com/philo-groves/beale-mono/blob/main/apps/desktop/resources/app-icon.png?raw=true">
</p>

<p align="center">
    <a href="https://github.com/philo-groves/beale-mono/blob/main/CHANGELOG.md" target="_blank">Changelog</a>
     • <a href="https://beale.ai/blog/" target="_blank">Blog</a>
     • <a href="https://beale.ai/docs/" target="_blank">Docs</a>
     • <a href="https://beale.ai/downloads/" target="_blank">Downloads</a>
</p>

# Beale

A decoding agent, now in beta!

## Projects

### App Server

The app server (`app-server/`) is the main processing unit of the Beale ecosystem. It contains all business logic of the agent, workspaces, research, permissions, and providers. The app server shares APIs that are consumed by various clients for security research.

### Desktop App

The desktop app (`apps/desktop/`) is the primary client user interface of Beale; there is no CLI or TUI. It allows for security research understanding and steering both locally and remotely. Features usually arrive in the desktop app before other clients.

### iOS App

The iOS app (`apps/ios/`) is a secondary client user interface and it is a work in progress. It is not yet available on the app store, but may be installed and used through developer tools. The iOS app focus is security research and notifications on the go.

## Top Features

Beale has many features, so only the top features are mentioned here. See https://beale.ai/docs/ for additional information.

### Permissions

A multi-layer permission model is enforced through Beale. A scope is maintained in each workspace for the agent to work within. An auto-review feature flags potentially harmful commands and those outside of scope. A guardian feature fully inspects flagged commands for a blocking decision.

### Collaboration

Multi-provider support is central to the harness; default provider support includes OpenAI, Anthropic, xAI, and Open Router. Custom providers may also be imported. Each research session allows for configuration of main + subagent models and their roles.

### Fleet

Easily work across app servers, browsers, and virtual machines with a central configuration. One desktop or iOS app may connect to several app servers. All cross-device networking with an app server is protected with Tailscale + operator token enforcement.

### Surfaces

A hierarchy of surfaces is maintained for each workspace. Beale ranks these surfaces, then explores each with recon, threat modeling, and analysis. Each bit of persistent knowledge is categorized under a surface.

### Findings

Security findings are tracked in a column-based board, making it easy to understand the state of research. Each finding moves through states: lead, observed, reproduced, and verified. Verified findings are rated for CVSS 3.1 impact.

### Memories

A security-focused memory system helps to align research and enrich context. Several memory types are available. Invariants, sinks, and mitigations assist with surface threat modeling, while trajectories assist with their analysis.

### Runbooks

A runbook is a Beale-managed Jupyter notebook. Supporting 40+ programming languages and providing step-based execution, runbooks are ideal for security research because they allow procedures to easily be resused, understood, and documented along the way.

### Browser

A built-in browser allows Beale to perform web-based research and obtain information when needed. Each browser tab is labeled with its own self-contained context, allowing the agent to manage multiple test account sessions concurrently.

### Plugins

Beale implements the Agent Plugins 1.0 standard, meaning you can bring the plugins from your other agents, or create your own with a known format. Several built-in optional plugins are offered for common use cases.

## Installation

### Install from Source

```
curl -fsSL https://beale.ai/i | node --input-type=module -
```

Additional Notes:
- On Windows, run this in PowerShell with curl.exe in place of curl.
- The default location is `~/Beale` on macOS or `%USERPROFILE%\Beale` on Windows; pass `--dir <empty-directory>` after the final `-` to choose another location.
- Start with `Beale.command` or `Beale.cmd` in that directory.
- The installer refuses to change a nonempty directory or an existing installation. This source flow requires the build toolchain and does not provide an automatic updater.

### Install from Web

An standard installer for each available operating system is located at https://beale.ai/downloads/

## Development

Requirements:
- Node.js >= 22.19.0
- pnpm 11

### Install Dependencies

```
pnpm install
```

### Quick Start

```
pnpm run dev:desktop  # start the app-server and desktop app in dev mode
pnpm build            # tsc -b across runtime packages and app-server
pnpm check            # typecheck only
pnpm start            # start the app-server tray host
```

### Test Tiers

```
pnpm test                         # default fast monorepo gate
pnpm test:integration             # hosted-worker and real-session Desktop coverage
pnpm test:all                     # both tiers
pnpm release:check                # check coordinated release versions
pnpm release:verify               # full local release candidate gate
pnpm test:runtime:fast            # isolated research-runtime tests
pnpm test:runtime:integration     # hosted-runtime integration cases
```

### App Server

```
pnpm build            # built together with the packages by tsc -b
pnpm --filter @beale/app-server start            # tray host (Windows, macOS)
pnpm --filter @beale/app-server start:headless   # plain Node process
pnpm test:app-server  # node:test suite against the hosted runtime
```

### Desktop App

```
pnpm --filter @beale/desktop dev          # Electron dev mode
pnpm --filter @beale/desktop build        # typecheck + production bundle
pnpm --filter @beale/desktop start        # run the built app
pnpm --filter @beale/desktop typecheck
pnpm --filter @beale/desktop test         # unit tests (vitest)
```

### Releases

The release procedure, CI gates, data backup, signing setup, and tag rules are in [RELEASING.md](https://github.com/philo-groves/beale-mono/blob/main/RELEASING.md). The tagged Release workflow builds signed macOS and Windows installers when the release environment has the required credentials.

## Safety

This tool is intended only for authorized vulnerability research and testing. Always respect scope, legal boundaries, and responsible disclosure practices.

## License

MIT. See [LICENSE](https://github.com/philo-groves/beale-mono/blob/main/LICENSE).