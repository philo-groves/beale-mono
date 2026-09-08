# Tart macOS research

Use Tart when the experiment needs a stock macOS system rather than a modified low-level Darwin guest.

## Baseline

- Begin from an operator-prepared, version-pinned image.
- Record the macOS build, architecture, image provenance, and relevant security settings.
- Prefer a fresh clone or known snapshot for each experiment series.
- Keep SIP and other mitigations enabled for final validation unless the research question explicitly concerns their disabled state.

## Plugin posture

`start_tart_vm` always disables graphics, audio, and clipboard integration. It uses Tart's ordinary shared NAT by default because Tart host-only networking launches Softnet and requires host root privileges. Request `networkMode: "host-only"` only when the operator has deliberately prepared that privilege boundary. The plugin exposes no directory or disk sharing and no image clone, pull, prune, or delete operations.

Call `inspect_tart_vm` first. It checks the named VM, reports concurrent guests, and obtains a fixed product-version, build, architecture, and SIP baseline. The plugin prefers Tart Guest Agent. Its compatibility policy can fall back to the operator-configured SSH identity and pinned known-hosts file, while `guest-agent-only` fails closed without opening host networking. Neither connection detail is exposed to the model. `start_tart_vm` permits concurrent, name-bound guests by default and performs a bounded readiness check. Use `requireExclusive=true` only when the experiment itself requires exclusivity. Never stop or refuse work merely because an unrelated VM is running.

Use `exec_tart_vm` for readiness, setup, and diagnostics. Pass an argument vector; do not wrap a command in a shell string merely to reproduce transport behavior. For Guest Agent transport, the plugin streams a host-built native arm64 exec helper to `/tmp/.beale-tart-exec-v3`, closes every inherited non-stdio descriptor, and then executes the requested argv. It probes descriptor pressure before each operation and recycles the launchd-managed Guest Agent before saturation without rebooting the VM or replaying the requested command. Claim-confirming execution belongs in an existing or new proof runbook: call `inspect_tart_vm` immediately before `runbook.run`, then use one stable bounded `tart exec <vm> /tmp/.beale-tart-exec-v3 <artifact> ...` entry command for that proof phase. Never make a second direct RPC for diagnostics or host-side polling; it bypasses recovery and recreates the control-socket leak. Wait or poll inside the single guest command. Capture relevant stdout, stderr, exit status, and selected transport. Do not invoke SSH, SCP, a host root runner, `tart ip`, Softnet, route repair, or a lifecycle runbook yourself. Historical runbooks containing those mechanisms are evidence, not reusable transport procedures. Use `tart_vm_ip` only when a host-side protocol genuinely requires an IP; it resolves the explicitly named running VM and remains safe when other guests are active.

Use `copy_to_tart_vm` and `copy_from_tart_vm` for single-file staging and evidence retrieval. Supply absolute host and guest file paths, set a per-call byte bound, and request overwrite only when replacement is intentional. The plugin stages and hashes each transfer before committing it, preserves permission bits by default, and applies the same transport policy as `exec_tart_vm`. Do not fall back to direct SCP or expose guest addressing when a transfer fails; preserve the bounded diagnostic once.

If the guest exits during startup, the tool returns the bounded Tart launcher diagnostic instead of reporting a successful start followed by a generic guest-agent failure. If the guest remains running but its agent is not ready after the bounded check, preserve that single diagnostic and continue static research. The plugin recycles recoverable Guest Agent descriptor pressure before a requested command starts. If preparation or recovery still reports `Too many open files`, pipe creation failure, or descriptor exhaustion, the requested command is not replayed; stop/start the disposable clone once before more guest execution. Do not spend the research session repeatedly changing Tart networking or rebuilding VM wrappers.

Treat Tart as a macOS boundary. It is not a substitute for physical-iPhone iOS validation.
