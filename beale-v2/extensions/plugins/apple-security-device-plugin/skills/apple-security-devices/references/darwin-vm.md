# darwin-vm low-level research

`darwin-vm` is a minimal QEMU-based Darwin environment with a root shell and custom filesystem. It is useful for kernel debugging, custom code, boot experiments, and low-level triage. It is not a full iPhone or Mac emulator: GUI, SpringBoard, graphics, Wi-Fi, Bluetooth, and other product surfaces are absent.

## Choosing Darwin VM or Tart

Darwin VM offers an inspectable low-level Darwin lab without a physical device, including the upstream QEMU fork's SPTM support and custom boot artifacts. Tart supplies a full macOS VM for application, service, and stock macOS behavior. Neither substitutes for physical-iPhone validation of device-specific behavior. Firmware modifications and an emulated device model limit what Darwin VM observations establish.

## Workflow

1. Call `environment_status`, then `inspect_darwin_vm` with the existing `checkoutRoot` supplied by the authorized workspace context and `hashArtifacts: true`. The plugin does not configure or retain that path. Record repository revision and firmware/build provenance separately: file hashes and presence alone do not establish compatibility.
2. Review the reported artifact presence and hashes. Resolve warnings before booting.
3. Call `start_darwin_vm` with the same `checkoutRoot`. Optional `bootArguments` extend the required baseline; they do not replace it. The plugin invokes the built QEMU binary directly with no emulated network device, host directory share, graphics, or QEMU monitor.
4. Use `read_darwin_vm_log` for bounded serial evidence and `run_darwin_vm_console_command` for a single newline-terminated console command.
5. Call `stop_darwin_vm` when finished.

QEMU remains detached if the MCP process restarts. A later list, log, console, or stop call reattaches only after matching the live PID to the expected QEMU executable and the run's unique serial socket and log arguments. Treat an `orphaned` state as a failed identity check; the plugin will not signal that process.

Treat the environment as intentionally modified. A custom trust cache, patched ramdisk, root shell, fake hardware model, or development kernel changes the security boundary. Use the guest to generate and refine hypotheses, then reproduce environment-sensitive conclusions on the corresponding stock boundary.
