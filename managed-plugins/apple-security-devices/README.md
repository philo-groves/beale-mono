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

The plugin never downloads firmware, clones repositories, modifies ramdisks, creates Tart images, pairs devices, or manages Darwin VM checkouts. Darwin tools receive an existing checkout root from the authorized workspace context for each inspection or launch.

Use Darwin VM for low-level Darwin inspection without a physical device. Use Tart for a full macOS VM and application/service behavior. See [Darwin VM guidance](skills/apple-security-devices/references/darwin-vm.md).

## Import

In Beale, open Plugins, choose **Add Agent Plugin**, and select this directory. Beale discovers the skill from `skills/apple-security-devices/SKILL.md` and starts the `devices` MCP server from `mcp.json`.

Read operations inspect availability and state. Bounded Tart and Darwin VM lifecycle, guest execution, transfer, and console operations are auto-reviewed; physical-iPhone mutation still requires explicit host confirmation.

Darwin VM runs are detached from the MCP server so an app-server restart does not terminate the guest. On the next list, log, console, or stop operation, the plugin reattaches only when the persisted PID still identifies the expected QEMU executable with the run's unique serial socket and log arguments. A live process that fails those checks is reported as `orphaned` and is never signaled by the plugin. Optional `bootArguments` are appended to the required baseline rather than replacing it.

Tart operations are VM-name-bound and guest-agent-first. The default compatibility policy permits an internal bounded SSH fallback for existing guests that do not run Tart Guest Agent. Operator-prepared clone sources should instead include Guest Agent RPC support and use the `guest-agent-only` policy so guest execution and transfer never depend on host networking or a privileged command runner. Concurrent guests are allowed by default, while callers can request strict exclusivity for experiments that need it. Startup uses unprivileged shared NAT by default, with host-only Softnet networking available explicitly on operator-prepared hosts. Startup returns a bounded build/SIP baseline or the actual early launcher failure, and IP lookup resolves the explicitly named running guest even when others are active.

`copy_to_tart_vm` and `copy_from_tart_vm` automatically stream one regular file between absolute host and guest paths without model-visible connection details. Transfers support up to 4 GiB by default, accept an optional tighter `maxBytes` bound, refuse replacement unless `overwrite=true`, preserve file mode by default, stage through a temporary file, and verify byte count plus SHA-256 before committing the destination.

Guest Agent execution and transfer commands pass through a host-built native arm64 exec helper at `/tmp/.beale-tart-exec-v3` that closes every inherited non-stdio descriptor before executing the requested argument vector. The plugin samples descriptor pressure at bounded intervals instead of adding a probe to every request and, before saturation, recycles the launchd-managed Guest Agent service without rebooting the VM or replaying the requested command. If an external VM reset removes the cached helper, the first unstarted invocation repairs it and retries once; the requested guest command is never replayed after it starts. Tart operations for one VM are serialized across that maintenance boundary. Cancellation terminates the active host-side Tart process, drops cancelled queued work, and releases the per-VM queue. If recycling fails, readiness returns immediately and guest operations remain latched off until a clean stop/start clears the unhealthy boot. A proof runbook can assign a code cell the `tart-vm` executor with a VM name, host executable reference, and explicit guest or passwordless-root execution posture. The app-server then materializes and streams the host-built executable, inspects immediately before one bounded proof invocation, records guest evidence, and cleans up. The Guest Agent service UID is distinct from a root-mode proof command's effective UID. Runbooks should not contain direct transport RPCs or host-side polling loops.

Set `tartTransportPolicy` in the private `host-config.json` inside this plugin's data directory to `guest-agent-only` or `guest-agent-or-ssh`. The latter is the compatibility default. In `guest-agent-only` mode, a missing Guest Agent fails closed before the plugin opens SSH or invokes any host command runner. `APPLE_SECURITY_TART_TRANSPORT_POLICY` provides the equivalent process-level override.

When the compatibility policy is active and the Beale host process cannot route directly to a guest, `host-config.json` may also contain a `commandRunner` absolute path. The plugin invokes that runner as `run -- <command> <args...>` and never returns the configured path or SSH material to the model. Host-to-guest copies use a path-based SCP invocation through the runner rather than requiring it to relay process stdin. `APPLE_SECURITY_COMMAND_RUNNER` provides the equivalent process-level override; `APPLE_SECURITY_SCP_COMMAND` can select the SCP executable when required by an operator-managed environment.
