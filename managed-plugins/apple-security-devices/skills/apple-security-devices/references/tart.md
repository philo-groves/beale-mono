# Tart macOS research

Use Tart when the experiment needs a stock macOS system rather than a modified low-level Darwin guest.

## Baseline

- Begin from an operator-prepared, version-pinned image.
- Record the macOS build, architecture, image provenance, and relevant security settings.
- Prefer a fresh clone or known snapshot for each experiment series.
- Keep SIP and other mitigations enabled for final validation unless the research question explicitly concerns their disabled state.

## Plugin posture

`start_tart_vm` always disables graphics, audio, and clipboard integration. It uses Tart's ordinary shared NAT by default because Tart host-only networking launches Softnet and requires host root privileges. Request `networkMode: "host-only"` only when the operator has deliberately prepared that privilege boundary. The plugin exposes no directory or disk sharing and no image clone, pull, prune, or delete operations.

Call `inspect_tart_vm` first. It checks the named VM, reports concurrent guests, and obtains a fixed product-version, build, architecture, and SIP baseline. The plugin prefers Tart Guest Agent and falls back to the operator-configured SSH identity and pinned known-hosts file used by existing research guests; neither connection detail is exposed to the model. `start_tart_vm` permits concurrent, name-bound guests by default and performs a bounded readiness check. Use `requireExclusive=true` only when the experiment itself requires exclusivity. Never stop or refuse work merely because an unrelated VM is running.

Use `exec_tart_vm` for ordinary guest execution. Pass an argument vector; do not wrap a command in a shell string. The plugin safely quotes the vector if its internal SSH fallback is required. Capture relevant stdout, stderr, exit status, and selected transport. Do not invoke SSH, `tart ip`, Softnet, route repair, or a lifecycle runbook yourself. Use `tart_vm_ip` only when a host-side protocol genuinely requires an IP; it resolves the explicitly named running VM and remains safe when other guests are active.

Use `copy_to_tart_vm` and `copy_from_tart_vm` for single-file staging and evidence retrieval. Supply absolute host and guest file paths, set a per-call byte bound, and request overwrite only when replacement is intentional. The plugin stages and hashes each transfer before committing it, preserves permission bits by default, and uses the same Guest Agent/private-runner selection as `exec_tart_vm`. Do not fall back to direct SCP or expose guest addressing when a transfer fails; preserve the bounded diagnostic once.

If the guest exits during startup, the tool returns the bounded Tart launcher diagnostic instead of reporting a successful start followed by a generic guest-agent failure. If the guest remains running but its agent is not ready after the bounded check, preserve that single diagnostic and continue static research. Do not spend the research session repeatedly changing Tart networking or rebuilding VM wrappers.

Treat Tart as a macOS boundary. It is not a substitute for physical-iPhone iOS validation.
