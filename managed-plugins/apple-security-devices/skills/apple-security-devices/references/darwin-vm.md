# darwin-vm low-level research

`darwin-vm` is a minimal QEMU-based Darwin environment with a root shell and custom filesystem. It is useful for kernel debugging, custom code, boot experiments, and low-level triage. It is not a full iPhone or Mac emulator: GUI, SpringBoard, graphics, Wi-Fi, Bluetooth, and other product surfaces are absent.

## Choosing Darwin VM or Tart

Darwin VM offers an inspectable low-level Darwin lab without a physical device, including the upstream QEMU fork's SPTM support and custom boot artifacts. Tart supplies a full macOS VM for application, service, and stock macOS behavior. Neither substitutes for physical-iPhone validation of device-specific behavior. Firmware modifications and an emulated device model limit what Darwin VM observations establish.

## Session setup

With this plugin enabled, Desktop offers **Not now**, **Never ask again**, or **Set up Darwin VM** before a new research session launches. Accepting opens a guide and a checkout field. The app-server validates nonempty required files, rejects an incomplete SPTM/TXM pair, and saves the checkout for future plugin processes; it does not boot the guest as part of validation. A missing checkout is offered again unless prompts were permanently declined.

Follow the [upstream project setup guide](https://github.com/jprx/darwin-vm#quick-start). Firmware preparation requires a Mac and upstream prerequisites; the QEMU execution host may be separate. Beale's current launcher requires a macOS or Linux app-server host with the Unix QEMU binary and serial socket support. Native Windows onboarding can be declined, but cannot save a runnable checkout. Upstream supporting additional hosts does not imply this plugin launcher supports them.

The app-server owns `darwin-vm-setup.json` in its Agent Plugin registry directory. The preference applies to new Desktop research launches across workspaces using that registry; automated launches and session continuations do not open interactive onboarding. Existing tool calls may still supply a checkout explicitly. Changing the prompt preference does not delete a checkout.

## Preparation boundary

Prepare the checkout outside this plugin. Pin the repository revision, initialize and build `qemu-sptm`, acquire firmware from an authorized source, and match the device profile, firmware build, kernel collection, device tree, trust cache, ramdisk, SPTM, and TXM artifacts exactly. Review upstream setup scripts before running them; the plugin does not run those scripts.

## Workflow

1. Call `environment_status`, then `inspect_darwin_vm` with `hashArtifacts: true`. Omit `checkoutRoot` for the saved checkout, or supply an explicitly prepared checkout. Record repository revision and firmware/build provenance separately: file hashes and presence alone do not establish compatibility.
2. Review the reported artifact presence and hashes. Resolve warnings before booting.
3. Call `start_darwin_vm`. The plugin invokes the built QEMU binary directly with no emulated network device, host directory share, graphics, or QEMU monitor.
4. Use `read_darwin_vm_log` for bounded serial evidence and `run_darwin_vm_console_command` for a single newline-terminated console command.
5. Call `stop_darwin_vm` when finished.

Treat the environment as intentionally modified. A custom trust cache, patched ramdisk, root shell, fake hardware model, or development kernel changes the security boundary. Use the guest to generate and refine hypotheses, then reproduce environment-sensitive conclusions on the corresponding stock boundary.

