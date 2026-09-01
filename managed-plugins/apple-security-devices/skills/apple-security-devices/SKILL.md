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
4. Explain the exact state-changing action and expected evidence before calling a confirmation-required tool.
5. Use bounded commands and capture the OS/build, target type, artifact identity, inputs, outputs, and contrary results needed to reproduce the observation.
6. Keep claims proportional to the environment. A modified `darwin-vm` root shell proves behavior in that lab configuration, not reachability on a stock iPhone or Mac.
7. Require stock-device or stock-guest reproduction before promoting an environment-sensitive observation to a confirmed vulnerability conclusion.

## Tool map

Read-only tools:

- `environment_status`
- `list_tart_vms`
- `tart_vm_ip`
- `list_physical_iphones`
- `describe_physical_iphone`
- `inspect_darwin_vm`
- `list_darwin_vm_runs`
- `read_darwin_vm_log`

Confirmation-required tools:

- `start_tart_vm`, `stop_tart_vm`, `exec_tart_vm`
- `install_physical_iphone_app`, `launch_physical_iphone_app`
- `start_darwin_vm`, `stop_darwin_vm`, `run_darwin_vm_console_command`

Do not invent capabilities that these tools do not expose. In particular, this plugin does not automate firmware acquisition, code signing, device pairing, jailbreaking, Tart image deletion, or `darwin-vm` ramdisk modification.

