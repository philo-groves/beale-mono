# Beale v2 visual review

This is an implementation-time designer self-review of the native Rune desktop, not an external usability study. Scores assess the rendered interface: hierarchy, typography, spacing, theme contrast, research context, and reachable controls. An 8 means a coherent, polished view with clear actions and no observed blocking layout or contrast defect in the reviewed fixtures. It does not imply that an unavailable product capability has been implemented.

## Evidence and method

- 59 synthetic view states rendered at 1440 x 960 in Light, Dark, Cream, and Midnight: 236 screenshots.
- 16 representative states also checked at 840 x 600 in all four themes: research desk, graph, transcript, Canvas, Findings, providers, custom providers, surface editor, finding details, graph editor, host connection, browser editor, safety rule, Features, Tracking, and confirmation dialog.
- The tab-icon follow-up rendered 15 affected states at 1440 x 960 and 10 at 840 x 600 in all four themes: 100 additional screenshots in `target/ui-tab-icons/` and `target/ui-tab-icons-narrow/`. These check icon alignment, theme colors, disabled workspace steps, and provider-tab reflow.
- The combined breadcrumb/title follow-up rendered 18 states at 1440 x 960 and 12 at 840 x 600 in all four themes: 120 screenshots in `target/ui-breadcrumbs/` and `target/ui-breadcrumbs-narrow/`. Narrow Appearance fixtures include extra-large text and the widest sidenav.
- Token Tracking was reviewed with synthetic daily/model usage, selected days, subagent filtering, empty history, missing cache/token telemetry, loading, offline, and stale-error states. All four themes were rendered at 1440 x 960, with full-page 1440 x 1500 and 840 x 1800 captures. Additional 840 x 600 and 840 x 3000 captures cover extra-large text and the widest sidenav. Evidence is in `target/ui-token-tracking/`, `target/ui-token-tracking-final/`, `target/ui-token-tracking-narrow/`, `target/ui-token-tracking-compact/`, `target/ui-token-tracking-small/`, and `target/ui-token-tracking-tall/`.
- Session Insights reuses the Tracking charts in a session-scoped modal. Populated, empty, missing-telemetry, and running-turn fixtures were rendered in all four themes, with 1440 x 960, 1440 x 1600, and 840 x 600 captures including extra-large text. Evidence is in `target/ui-session-insights-final/`, `target/ui-session-insights-small/`, and `target/ui-session-insights-full/`; `target/ui-session-insights/` includes Tracking regression captures.
- Session Insights agent filters reuse the Tracking segmented control and icons. All/Main/Subagents and the empty-subagent state were reviewed at 1440 x 960 in all four themes, with 840 x 600 normal and extra-large text captures. Evidence: `target/ui-session-filters/` and `target/ui-session-filters-small/`. The shared controls remain within bounds and the body scrolls independently of the close actions; the 8.5/10 rating remains unchanged in each theme.
- Native Rune snapshots use the same renderers as the desktop. Fixtures contain only synthetic workspaces, records, hosts, conversations, and example domains. Preview mode does not load saved desktop state or start app-server polling.
- Artifacts are reproducible with the commands in [README.md](README.md#visual-system-and-review). Full-size PNGs are `target/ui-audit/{scene}-{theme}.png`; contact sheets are `sheet-{theme}-{1..8}.png`. Small-window captures are in `target/ui-audit-narrow/`. These paths are relative to `beale-v2/` and ignored by Git.
- Interaction and layout checks complement the images: 170 desktop tests pass. Tracking adds selection/filter interaction checks, responsive chart/card bounds, and header-control separation in every theme, plus aggregation tests for UTC windows, source identity, missing telemetry, and saturating totals. Theme tests require at least 4.5:1 contrast for default body, secondary, success, warning, and error text against panel, card, and sidebar surfaces. Custom palette overrides are operator choices and are not constrained by that test.

## Corrections driven by screenshots

The initial review found a blank Home view, weak hierarchy, inconsistent surfaces, and neutral text colors that bypassed Light/Cream theme translation. The shared visual system now gives research context a consistent place in navigation, headings, cards, status colors, and empty states.

Dark uses a neutral black and charcoal palette with silver accents. Its surfaces, typography, borders, and shell gradient have no green tint; green remains available for semantic success states.

Breadcrumbs and view titles now occupy one row, with muted parent context, a chevron separator, and a 24px current-view title. This removes the separate eyebrow row and its gap. New Research keeps its workspace picker in the final crumb, with a dropdown chevron. Titles remain single-line in constrained layouts.

Text-based view tabs now share an inset segmented control with raised selection, a restrained border and shadow, stable label widths, and consistent hover feedback. Contextual icons use a shared size, spacing, foreground color, and disabled state. Provider tabs reflow into balanced groups based on measured label widths, available space, and text size. Settings, extension filters, Fleet filters, Archive, workspace setup, and Tracking use the same treatment across all four themes.

Token Tracking now has a dedicated sidenav, four summary cards, selectable daily input/output bars, a token-mix ring, and ranked model sources. Input includes cached tokens; cache hit rate uses only input from calls with cache telemetry. Missing telemetry renders as unavailable. Screenshot review aligned the empty-state action with its content and fixed the Tracking header overlapping Canvas with extra-large text and a wide sidenav. The cards reflow from four to two to one column; charts stack at narrow widths. The current Token Tracking review is 8.5/10 in each default theme and 8/10 in the most constrained extra-large-text fixture.

The first Session Insights screenshots exposed the raised research composer drawing over the modal. Its scrim and dialog now share the raised layer, above the composer. The modal retains visible close controls, scrolls to the full model breakdown, and preserves the same chart typography and colors across themes. Values come from the selected session, independently of Tracking enablement; older transcript turns remain represented.

Populated fixtures exposed clipped Findings lanes, crowded surface details, faint status labels, overly heavy menu shadows, and low-contrast destructive actions. Findings now reflows, surface details use a clear name and ID with an explicit close action, and missing timestamps no longer display the Unix epoch.

The small-window pass found unreadable split Canvas panes, long dialogs outside the viewport, custom-provider action overflow, and workspace tabs pushed off the right edge by long names. Canvas now uses a focused content view below the split threshold, session telemetry stacks above the transcript, dialog bodies scroll with visible actions, provider actions occupy their own row, and workspace titles yield space to navigation.

## Final view scores

Each row identifies a screenshot scene; all four themes were visually reviewed. Scores are out of 10 and describe the recorded synthetic states. The Campaign row covers a compatibility renderer that is not a current navigation tab. External plugin-source installation retains its unavailable state. The HackerOne Research Kit card was added after this visual review and is not scored here.

| View | Scene | Light | Dark | Cream | Midnight |
| --- | --- | ---: | ---: | ---: | ---: |
| Research desk | `home` | 8.5 | 8.5 | 8.5 | 8.5 |
| Research desk / no workspaces | `empty` | 8.0 | 8.0 | 8.0 | 8.0 |
| New Research / graph | `research` | 8.5 | 8.5 | 8.5 | 8.5 |
| Research transcript / telemetry | `session` | 8.5 | 8.5 | 8.5 | 8.5 |
| Runbook Canvas | `canvas` | 8.0 | 8.0 | 8.0 | 8.0 |
| Quick Chat / transcript | `chat` | 8.0 | 8.0 | 8.0 | 8.0 |
| Workspace settings | `workspace` | 8.0 | 8.0 | 8.0 | 8.0 |
| Workspace rules | `rules` | 8.0 | 8.0 | 8.0 | 8.0 |
| Surface network | `surfaces` | 8.5 | 8.5 | 8.5 | 8.5 |
| Surface details | `surface-detail` | 8.0 | 8.0 | 8.0 | 8.0 |
| Surface management | `surface-manage` | 8.0 | 8.0 | 8.0 | 8.0 |
| Findings ledger | `findings` | 8.5 | 8.5 | 8.5 | 8.5 |
| Workspace import | `import` | 8.0 | 8.0 | 8.0 | 8.0 |
| New workspace / General | `create-general` | 8.0 | 8.0 | 8.0 | 8.0 |
| New workspace / Surfaces | `create-surfaces` | 8.0 | 8.0 | 8.0 | 8.0 |
| New workspace / Rules | `create-rules` | 8.0 | 8.0 | 8.0 | 8.0 |
| Features | `features` | 8.5 | 8.5 | 8.5 | 8.5 |
| Plugins | `plugins` | 8.0 | 8.0 | 8.0 | 8.0 |
| Fleet / App Servers | `fleet` | 8.0 | 8.0 | 8.0 | 8.0 |
| Fleet / Virtual Machines | `vms` | 8.0 | 8.0 | 8.0 | 8.0 |
| Automation job | `automations` | 8.0 | 8.0 | 8.0 | 8.0 |
| Session Insights | `session-insights` | 8.5 | 8.5 | 8.5 | 8.5 |
| Tracking / Token Tracking | `tracking` | 8.5 | 8.5 | 8.5 | 8.5 |
| OpenAI provider | `providers` | 8.0 | 8.0 | 8.0 | 8.0 |
| Custom providers | `custom` | 8.0 | 8.0 | 8.0 | 8.0 |
| Safety / Auto-Review | `safety` | 8.0 | 8.0 | 8.0 | 8.0 |
| Safety / Guardian | `guardian` | 8.0 | 8.0 | 8.0 | 8.0 |
| Appearance / Colors | `appearance` | 8.5 | 8.5 | 8.5 | 8.5 |
| Appearance / Typography | `typography` | 8.0 | 8.0 | 8.0 | 8.0 |
| Archive / Sessions | `archive` | 8.0 | 8.0 | 8.0 | 8.0 |
| Archive / Quick Chats | `archive-chats` | 8.0 | 8.0 | 8.0 | 8.0 |
| Discard-workspace confirmation | `dialog` | 8.0 | 8.0 | 8.0 | 8.0 |
| Theme menu | `menu` | 8.0 | 8.0 | 8.0 | 8.0 |
| Color picker | `picker` | 8.0 | 8.0 | 8.0 | 8.0 |
| Anthropic provider | `anthropic` | 8.0 | 8.0 | 8.0 | 8.0 |
| OpenRouter provider | `openrouter` | 8.0 | 8.0 | 8.0 | 8.0 |
| xAI provider | `xai` | 8.0 | 8.0 | 8.0 | 8.0 |
| Z.ai provider | `zai` | 8.0 | 8.0 | 8.0 | 8.0 |
| API key entry | `api-key` | 8.0 | 8.0 | 8.0 | 8.0 |
| Custom provider editor | `custom-editor` | 8.0 | 8.0 | 8.0 | 8.0 |
| Graph node editor | `graph-editor` | 8.0 | 8.0 | 8.0 | 8.0 |
| Research model menu | `model-menu` | 8.0 | 8.0 | 8.0 | 8.0 |
| Goal controls | `goal-menu` | 8.0 | 8.0 | 8.0 | 8.0 |
| Add surface | `surface-add` | 8.0 | 8.0 | 8.0 | 8.0 |
| Remove-surface confirmation | `surface-delete` | 8.0 | 8.0 | 8.0 | 8.0 |
| Surface connection editor | `relationship` | 8.0 | 8.0 | 8.0 | 8.0 |
| Workspace name editor | `workspace-name` | 8.0 | 8.0 | 8.0 | 8.0 |
| Workspace rule editor | `workspace-rule` | 8.0 | 8.0 | 8.0 | 8.0 |
| Feature details | `feature-detail` | 8.0 | 8.0 | 8.0 | 8.0 |
| Plugin source dialog | `plugin-add` | 8.0 | 8.0 | 8.0 | 8.0 |
| Plugin manifest | `plugin-detail` | 8.0 | 8.0 | 8.0 | 8.0 |
| App-server connection dialog | `fleet-connect` | 8.0 | 8.0 | 8.0 | 8.0 |
| Tailscale import | `fleet-import` | 8.0 | 8.0 | 8.0 | 8.0 |
| App-server details | `fleet-detail` | 8.0 | 8.0 | 8.0 | 8.0 |
| Safety rule editor | `safety-rule` | 8.0 | 8.0 | 8.0 | 8.0 |
| Finding details | `finding-detail` | 8.0 | 8.0 | 8.0 | 8.0 |
| Campaign compatibility empty state | `campaign` | 8.0 | 8.0 | 8.0 | 8.0 |

All reviewed views meet the requested 8/10 threshold under this self-review rubric. The strongest improvements are the research desk, theme legibility, Findings hierarchy, and small-window behavior. Live provider sign-in, remote-host availability, and long-running research execution were not exercised by the visual audit; their existing runtime boundaries are unchanged.
