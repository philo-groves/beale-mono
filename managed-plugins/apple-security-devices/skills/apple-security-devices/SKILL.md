---
name: apple-security-devices
description: Use realistic Apple security research environments through Tart macOS VMs, physically connected iPhones, or darwin-vm. Apply when selecting, preparing, operating, or interpreting Apple platform experiments, especially when iOS Simulator would give unrealistic security evidence.
---

# Apple Security Devices

Choose an environment by the security boundary the experiment must reproduce.

## Non-negotiable iOS boundary

Never use, recommend, or treat iOS Simulator as security-research evidence. Do not use `simctl`. Simulator results cannot establish physical-iPhone exploitability, mitigation behavior, entitlement enforcement, code-signing behavior, hardware-backed security behavior, or Apple Security Bounty impact.

If a request proposes iOS Simulator security research, stop that path and route the experiment to an authorized physical iPhone. If no physical iPhone is available, say that realistic iOS validation is unavailable. Static source review and host-side unit testing may continue, but label them as non-device evidence.

## Select the environment

- Use a physical iPhone for iOS application behavior, real signing and entitlements, sandbox behavior, device services, hardware-backed boundaries, and final iOS validation. Read [physical-iphone.md](references/physical-iphone.md).
- Use Tart for stock macOS behavior, SIP-on validation, application and service testing, regression matrices, and disposable macOS guests. Read [tart.md](references/tart.md).
- Use `darwin-vm` for kernel debugging, custom kernel or trust-cache work, boot-path instrumentation, and minimal command-line Darwin experiments. Read [darwin-vm.md](references/darwin-vm.md).
- Use more than one environment when a low-level hypothesis needs stock-device confirmation. Treat the physical iPhone or stock macOS guest as the final evidence boundary, as applicable.

## Operating rules

1. Confirm the recorded authorized scope before interacting with a device, guest, app, or firmware build.
2. Call `environment_status` before planning an execution path.
3. Inspect state with read-only tools before requesting any mutation.
4. Explain the exact state-changing action and expected evidence before operations that require confirmation. Tart lifecycle and name-bound guest execution are auto-reviewed and do not require per-call confirmation.
5. Use bounded commands and capture the OS/build, target type, artifact identity, inputs, outputs, and contrary results needed to reproduce the observation.
6. Keep claims proportional to the environment. A modified `darwin-vm` root shell proves behavior in that lab configuration, not reachability on a stock iPhone or Mac.
7. Require stock-device or stock-guest reproduction before promoting an environment-sensitive observation to a confirmed vulnerability conclusion.
8. Treat environment setup as a bounded prerequisite, not the research objective. Spend at most three consecutive tool calls or two minutes establishing device readiness. If readiness still fails, record the exact blocker and continue source analysis, proof design, or evidence review that does not require the guest.
9. For Tart lifecycle, readiness, diagnostics, and transfer, never build a shell/runbook wrapper around SSH, SCP, DHCP, Softnet, route repair, or plugin behavior. Use `inspect_tart_vm`, `start_tart_vm`, `exec_tart_vm`, `copy_to_tart_vm`, and `copy_from_tart_vm`; these address the selected guest by VM name while the plugin selects Tart Guest Agent or its configured bounded SSH fallback internally. Use direct `exec_tart_vm` only for readiness, setup, and diagnostics that do not validate a claim. Before claim-confirming guest execution, create or reuse the proof runbook and execute a stable bounded `tart exec <vm> <artifact>` entry cell with `runbook.run`; this is the sole exception to the no-wrapper rule and must not duplicate lifecycle, transfer, credential, or route logic.
10. Tart operations are bound to an explicit VM name and may run concurrently. An unrelated running VM is not a reason to stop, defer, or refuse a proof. Request `requireExclusive=true` only when the experiment itself requires exclusive host resources or isolation; otherwise leave unrelated guests untouched.
11. Do not create lifecycle runbooks or append a proof cell per failed tweak. Keep iterative proof code in a stable staged artifact, rerun its existing runbook entry cell, and append only when the procedure itself changes. After one reproducible environment failure, preserve the diagnostic once and return to the highest-value unanswered research question.

## Tool map

Read-only tools:

- `environment_status`
- `list_tart_vms`
- `inspect_tart_vm`
- `tart_vm_ip`
- `list_physical_iphones`
- `describe_physical_iphone`
- `inspect_darwin_vm`
- `list_darwin_vm_runs`
- `read_darwin_vm_log`

Auto-reviewed Tart tools:

- `start_tart_vm`, `stop_tart_vm`, `exec_tart_vm`
- `copy_to_tart_vm`, `copy_from_tart_vm`

Confirmation-required tools:

- `install_physical_iphone_app`, `launch_physical_iphone_app`
- `start_darwin_vm`, `stop_darwin_vm`, `run_darwin_vm_console_command`

Do not invent capabilities that these tools do not expose. In particular, this plugin does not automate firmware acquisition, code signing, device pairing, jailbreaking, Tart image deletion, or `darwin-vm` ramdisk modification.
