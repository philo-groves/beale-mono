# Apple Security Devices

`apple-security-devices` is a portable Agent Plugin for evidence-oriented Apple security research. It deliberately separates three environments with different evidentiary value:

- Tart runs stock macOS guests for reproducible macOS behavior and mitigation validation.
- CoreDevice `devicectl` communicates with authorized, physically connected iPhones for real iOS behavior.
- `darwin-vm` runs an intentionally incomplete, instrumentable Darwin root-shell environment for kernel and low-level userspace work.

iOS Simulator is excluded. It does not reproduce the physical iPhone security boundary and must not be used to support iOS exploitability, mitigation, or bounty conclusions.

## Requirements

- macOS host for Tart and physical-iPhone tools.
- Tart installed and a separately prepared VM image for Tart operations.
- Xcode with `devicectl`, plus a paired and trusted physical iPhone, for device operations.
- A separately cloned and prepared `darwin-vm` checkout, including its built `qemu-sptm` and version-matched firmware artifacts, for low-level Darwin operations.

The plugin never downloads firmware, clones repositories, modifies ramdisks, creates Tart images, or pairs devices. Those setup actions remain operator-controlled.

## Import

In Beale, open Plugins, choose **Add Agent Plugin**, and select this directory. Beale discovers the skill from `skills/apple-security-devices/SKILL.md` and starts the `devices` MCP server from `mcp.json`.

Read operations inspect availability and state. Bounded Tart lifecycle, name-bound guest execution, and single-file transfer are auto-reviewed; physical-iPhone mutation and low-level `darwin-vm` process or console control still require explicit host confirmation.

Tart operations are VM-name-bound and guest-agent-first, with an internal bounded SSH fallback for existing guests that do not run Tart Guest Agent. Concurrent guests are allowed by default, while callers can request strict exclusivity for experiments that need it. Startup uses unprivileged shared NAT by default, with host-only Softnet networking available explicitly on operator-prepared hosts. Startup returns a bounded build/SIP baseline or the actual early launcher failure, and IP lookup resolves the explicitly named running guest even when others are active.

`copy_to_tart_vm` and `copy_from_tart_vm` transfer one regular file between absolute host and guest paths without model-visible connection details. Transfers are capped at 256 MiB, default to a 64 MiB per-call limit, refuse replacement unless `overwrite=true`, preserve file mode by default, stage through a temporary file, and verify byte count plus SHA-256 before committing the destination.

When the Beale host process cannot route directly to a guest but an operator-managed command runner can, place a private `host-config.json` in this plugin's data directory with a `commandRunner` absolute path. The plugin invokes that runner as `run -- <command> <args...>` and never returns the configured path or SSH material to the model. Host-to-guest copies use a path-based SCP invocation through the runner rather than requiring it to relay process stdin. `APPLE_SECURITY_COMMAND_RUNNER` provides the equivalent process-level override; `APPLE_SECURITY_SCP_COMMAND` can select the SCP executable when required by an operator-managed environment.
