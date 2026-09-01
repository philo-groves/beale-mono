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

Read operations inspect availability and state. Starting, stopping, executing, installing, launching, or sending a console command is annotated for explicit host confirmation.

