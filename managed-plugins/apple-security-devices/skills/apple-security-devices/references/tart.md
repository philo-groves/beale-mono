# Tart macOS research

Use Tart when the experiment needs a stock macOS system rather than a modified low-level Darwin guest.

## Baseline

- Begin from an operator-prepared, version-pinned image.
- Record the macOS build, architecture, image provenance, and relevant security settings.
- Prefer a fresh clone or known snapshot for each experiment series.
- Keep SIP and other mitigations enabled for final validation unless the research question explicitly concerns their disabled state.

## Plugin posture

`start_tart_vm` always disables graphics, audio, and clipboard integration and uses Tart host-only networking. The plugin exposes no directory or disk sharing and no image clone, pull, prune, or delete operations.

Use `exec_tart_vm` only when the Tart guest agent is available. Pass an argument vector; do not wrap a command in a shell string. Capture relevant stdout, stderr, and exit status. Use `tart_vm_ip` only when a host-side connection is genuinely needed.

Treat Tart as a macOS boundary. It is not a substitute for physical-iPhone iOS validation.

