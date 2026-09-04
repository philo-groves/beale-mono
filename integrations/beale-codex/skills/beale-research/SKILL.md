---
name: beale-research
description: Work with canonical Beale research from Codex. Use when a task asks to inspect, continue, verify, or update research in a Beale workspace; coordinate with a Beale agent; or persist Codex evidence, leads, findings, runbooks, reports, and campaign state back to Beale.
---

# Beale Research

Use the `beale` MCP server as the durable research boundary. Use Codex-native filesystem, terminal, browser, computer-use, subagent, and approval capabilities for investigation work, then record durable outcomes through Beale research tools.

## Establish context

1. Call `beale_list_workspaces` and resolve the exact workspace by `workspaceId`.
2. Read recent work with `beale_list_sessions`, `beale_get_session`, and relevant channels.
3. Call `beale_list_research_tools` before research reads or writes. Tool availability and schemas follow the workspace's active research profile.
4. Use `history.search`, through `beale_read_research`, before creating a memory, lead, finding, runbook, or report.
5. When continuing a campaign track, pass its `investigationId` while listing and calling research tools so track-scoped tools are available.

## Investigate and persist

- Prefer Codex-native read and execution tools for source inspection, commands, web access, and computer use. Their normal Codex approval and Auto-Review behavior remains authoritative.
- Invoke read-only Beale tools with `beale_read_research`. The bridge rejects a mutating tool on this path.
- Invoke write, process, or network-side-effect Beale tools with `beale_write_research`. The bridge rejects a read-only tool on this path.
- Treat generated files and verifier output as candidate artifacts until accepted into durable Beale state.
- Record hypotheses with `lead.create`. Promote or revise the same canonical claim with `finding.revise` and `finding.transition`; do not create a second finding for the same claim.
- Attach exact evidence references. Observation, reproduction, verification, report readiness, and disclosure remain governed by the active profile's evidence gates.
- Use `runbook.create` and `runbook.append` for reusable proof procedures. Run formal proof cells with `runbook.run` through `beale_write_research` so the resulting durable run ID can support reproduction evidence.
- Store concise reusable knowledge with memory tools. Do not store transcripts, routine narration, credentials, tokens, or bulk command output as memory.
- Create or revise reports only after the profile's claim and evidence requirements are satisfied.

## Coordinate both ways

- Use `beale_steer_session` for a live Beale agent and `beale_continue_session` for a terminal session.
- Use a durable channel for collaboration that should outlive one session. Read it before posting; keep messages concise and link durable evidence or research IDs.
- Codex-written research is immediately visible to Beale because both use app-server-owned canonical storage. Read Beale again before revision-sensitive writes to avoid stale revisions.

## Safety boundaries

- Work only within the recorded authorized scope. An explicit out-of-scope resource is a hard stop.
- Resource discovery does not grant authorization. Ambient resources require confirmation in Beale before first touch; explicitly in-scope resources may be recorded by the bridge.
- Beale and its app-server run with the current user's host privileges and do not provide OS isolation. Use an operator-managed VM or container when target code needs isolation.
- Never request or reveal the app-server operator token, discovery contents, credential material, or database paths. The MCP server reads authentication internally and returns only API results.
- Confirmed vulnerability conclusions require tool-, artifact-, or verifier-backed evidence references. User assertions may seed leads but are not target observations.
