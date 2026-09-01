# darwin-vm low-level research

`darwin-vm` is a minimal QEMU-based Darwin environment with a root shell and custom filesystem. It is useful for kernel debugging, custom code, boot experiments, and low-level triage. It is not a full iPhone or Mac emulator: GUI, SpringBoard, graphics, Wi-Fi, Bluetooth, and other product surfaces are absent.

## Preparation boundary

Prepare the checkout outside this plugin. Pin the repository revision, initialize and build `qemu-sptm`, acquire firmware from an authorized source, and match the device profile, firmware build, kernel collection, device tree, trust cache, ramdisk, SPTM, and TXM artifacts exactly. Review upstream setup scripts before running them; the plugin does not run those scripts.

## Workflow

1. Call `inspect_darwin_vm` with the prepared checkout.
2. Review the reported artifact presence and hashes. Resolve warnings before booting.
3. Call `start_darwin_vm`. The plugin invokes the built QEMU binary directly with no emulated network device, host directory share, graphics, or QEMU monitor.
4. Use `read_darwin_vm_log` for bounded serial evidence and `run_darwin_vm_console_command` for a single newline-terminated console command.
5. Call `stop_darwin_vm` when finished.

Treat the environment as intentionally modified. A custom trust cache, patched ramdisk, root shell, fake hardware model, or development kernel changes the security boundary. Use the guest to generate and refine hypotheses, then reproduce environment-sensitive conclusions on the corresponding stock boundary.

