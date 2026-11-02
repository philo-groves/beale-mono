# Beale desktop preview

This Rust desktop client uses [Rune](https://github.com/anomalyco/rune), pinned to a specific upstream commit in `Cargo.toml`. The initial window keeps Beale's dark icon rail and opens Agent Settings → Providers. The Agent Settings sidenav lists General, Appearance, Providers, Profile, Prompt, Features, and Archive; only Providers has a settings view in this preview. The Providers view has OpenAI and xAI tabs above the current provider's Authentication and Default Models sections. Authentication and model controls connect to the host. The header toggle collapses the sidenav while retaining the icon rail. The header also has File/View/Window menus, a contextual title, and Windows minimize/maximize/close controls. On Windows, the borderless frame has a dark grey 1px edge and supports dragging its edges and corners to resize. Windows 11 uses a system-drawn border that follows its rounded corners; older Windows versions use an inset Rune border and retain their native corner shape. On macOS, Rune retains native traffic lights; on Linux, the system titlebar remains. The window has an 840 × 600 minimum content size. Other rail destinations explain that they are not available in this preview.

Typography follows the v1 UI and monospace font stacks. Rune loads installed host fonts for the desktop window, with its bundled font as a fallback.

Navigation, settings, and header icons use the Lucide shapes from v1 at pixel-aligned sizes. Their license is in [src/LUCIDE-LICENSE](src/LUCIDE-LICENSE).

The Agent Settings sidenav starts at 292px and resizes by dragging its right edge between 240px and 420px. The header title separator follows that edge and stays clear of the header menus when the sidenav is collapsed.

Run from `beale-v2/`:

```text
cargo run -p beale-desktop-app
```

Rune can render a headless snapshot for layout review:

```text
cargo run -p beale-desktop-app -- --snap target/beale-desktop-snap.png --scale 1
```

The desktop checks the app-server's versioned protobuf health endpoint, displays its connection state in the Agent Settings sidenav, and retries every two seconds. Run `cargo run -p beale-desktop-app -- --check-connection` for a one-shot connection check or `--provider-status` to inspect configured methods and the lead provider. Both processes use `BEALE_V2_APP_SERVER_ADDR`, defaulting to `127.0.0.1:43831`; see the [workspace run instructions](../../README.md). The Providers view can start same-device OpenAI and xAI subscription sign-in, configure a masked API key, show configured status, and forget either credential. It also loads each configured method's text-model catalog, shows the chosen lead and small models, and lets the user save or refresh those choices. The Lead control beside the provider tabs selects a configured primary provider; the initial provider view opens to the saved lead. Lead and model choices open in anchored, scrollable menus inside the window. Escape or an outside click closes a menu; arrow keys, Home, End, and Enter support keyboard selection. The app-server stores credentials and sends only status, sign-in prompts, and account-available model metadata to the desktop. This preview does not yet send inference requests from the desktop.
