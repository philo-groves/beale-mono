# Beale iOS

Beale iOS is a small SwiftUI client for the Beale app-server and shared live app-server sessions:

- launch presents saved app-server connections as a selectable list plus an Add Connection path for QR or manual provisioning;
- each saved connection keeps its own operator token in the iOS Keychain; tokens are never compiled into the application;
- `/health` is checked for control-version and capability compatibility;
- after connecting, the main Workspaces catalog lists registered workspaces and each workspace's three newest named sessions, with expandable session lists;
- workspace rows open remote workspace views that can start a new research session with fillable canonical suggestions, an Add Context checkbox that expands the selected goal with bounded campaign-aware host context before launch, optional persistent Goal intent, connected-provider model dropdowns, visible host Lead and subagent defaults, collaborator reasoning, Simple or Advanced subagent mode, and shell safety mode; they also provide separate Claims and Memories destinations, where Claims prioritizes Findings above Leads, supports text and classification filters, and opens canonical claim details, while Memories retains its type-filtered catalog and details;
- session rows open their canonical transcript with flat Desktop-style commentary, compact grouped tool summaries, responses, user bubbles, and the same Lucide Brain reasoning icon as Desktop; the icon-only status circle opens Active and Completed subagent rows with their latest message, channel, model, and activity age, and each row opens that subagent's commentary;
- active session views attach with an independently minted WebSocket token, can steer or stop the same live session as Desktop from the composer's trailing action, retain Stop alongside pending shell or computer-use approval actions, and continue polling the cursor-based transcript feed for canonical cross-device reconciliation;
- the leading menu includes Settings with horizontal General and Connections views; Connections uses the same collection-based catalog and Add Connection flow;
- an opt-in notification setting under General presents local iPhone alerts only for Medium or High findings that become Observed, Reproduced, or Verified;
- active session views check the path-free memory notification feed every six seconds, while an iOS app-refresh task provides opportunistic background checks;
- the leading menu's Workspaces action returns to that catalog from nested workspace or session views;
- the home-screen icon uses the same Beale artwork as Desktop;
- the canonical commentary stream reduces tool activity to safe labels such as `Read parser.ts` and `Ran rg`; no tool results, command arguments, host paths, app-server storage, provider credentials, or CLI arguments cross the mobile boundary.

The app targets iOS 17 or newer and can be built with Xcode 27 beta:

```sh
DEVELOPER_DIR=/Applications/Xcode-27-beta.app/Contents/Developer \
  xcodebuild -project apps/ios/Beale.xcodeproj \
  -scheme Beale \
  -sdk iphonesimulator \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' \
  build
```

## Connect over Tailscale

In Beale Desktop, open **Settings > Remote**, detect or enter this machine's full MagicDNS name, enable **Tailscale Serve**, and apply the setting. Desktop keeps the app-server on `127.0.0.1:47173`, configures a dedicated Tailscale Serve HTTPS listener on port `47174`, restarts the app-server, and advertises `https://<machine>.<tailnet>.ts.net:47174` to mobile clients. The dedicated listener leaves unrelated Serve routes unchanged.

For a headless/manual setup, use the same ports and advertised origin:

```sh
BEALE_APP_SERVER_PORT=47173 \
BEALE_APP_SERVER_PUBLIC_URL=https://your-mac.your-tailnet.ts.net:47174 \
pnpm --filter @beale/app-server start:headless

tailscale serve --bg --yes --https=47174 http://127.0.0.1:47173
```

In the app-server tray menu, choose **Show QR Code**, then choose **Scan QR Code** in Beale iOS. The versioned code contains the advertised HTTPS origin and operator token and connects immediately after iOS validates it. Treat the code like the operator token: only show and scan it around trusted devices. You can still enter both values manually. For a headless development launch, the token is in the private `~/.beale/app-server.json` discovery record. Do not put that token in source control, screenshots, logs, or Tailscale configuration.

The simulator uses the Mac's network stack, so a Mac already signed in to the tailnet can reach the Tailscale Serve URL. A physical iPhone must run Tailscale and be authorized by the tailnet ACLs. The app intentionally does not add an App Transport Security exception for plain HTTP tailnet traffic.

## Memory notifications

Enable **Settings > General > Research Attention** and grant iOS notification permission. The first successful feed read establishes a baseline; later claim revisions produce an alert only when the claim has an explicitly untrusted Medium or High rating and its lifecycle status is Observed, Reproduced, or Verified. Leads, knowledge-memory nodes, Informational, Low, or Critical ratings, and other lifecycle states remain silent. The app-server response intentionally omits memory bodies, attributes, evidence details, and all host paths.

Notifications use the same outbound HTTPS Tailscale Serve connection as the rest of the app. No connection from the Mac to the iPhone and no additional Desktop setting are required. While a live session view is open, checks run every six seconds. In the background, Beale submits an app-refresh task, but iOS decides when that task runs, so delivery is opportunistic rather than immediate. Guaranteed delivery while the app is suspended or terminated would require a future APNs provider and device-token flow.

Disable **Settings > Remote > Tailscale Serve** to remove only Beale's dedicated HTTPS listener. For a manual setup, use:

```sh
tailscale serve --https=47174 off
```
