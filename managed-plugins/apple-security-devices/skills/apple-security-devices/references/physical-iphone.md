# Physical iPhone research

Use only an authorized, paired, physically connected iPhone. The plugin uses Xcode CoreDevice through `xcrun devicectl`; it does not expose Simulator tooling.

## Workflow

1. Call `list_physical_iphones` and select the returned opaque `deviceRef`.
2. Call `describe_physical_iphone` to verify product family, OS/build, connection, and developer state without exposing durable device identifiers.
3. Install only an operator-built `.app` bundle with `install_physical_iphone_app`.
4. Launch by bundle identifier with `launch_physical_iphone_app`. Use `startStopped` when a debugger must attach before execution.
5. Capture evidence from the physical device and record app build/signing identity separately in the research artifacts.

Device references are process-local and intentionally expire when the MCP server restarts. Re-list devices instead of persisting identifiers.

Do not treat pairing, developer-mode availability, code signing, or trust prompts as bypassed by the plugin. Resolve them through normal operator-controlled Xcode and device workflows.

