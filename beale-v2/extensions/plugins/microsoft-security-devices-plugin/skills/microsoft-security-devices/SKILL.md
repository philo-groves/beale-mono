---
name: microsoft-security-devices
description: Prefer a Windows Insider Preview Canary Hyper-V guest for Windows validation. On the first VM touch in each session, compare its guest build and revision with current Microsoft release guidance and warn about outdated or unverified environments.
---

# Microsoft Security Devices

Prefer an operator-prepared Windows Insider Preview Canary VM in Hyper-V for Windows validation. Microsoft's [Windows Insider Preview bounty guidance](https://www.microsoft.com/en-us/msrc/bounty-windows-insider-preview) requires the latest applicable Canary build; Hyper-V is Beale's preferred lab environment, not a bounty requirement stated on that page.

## First VM touch in every session

Before the first substantive guest operation, perform a read-only readiness check for the selected VM. A connection needed to collect version data is part of this check. Do not use an earlier session's result, the host's Windows version, a VM name containing “Canary,” an ISO filename, or a desktop watermark as guest version evidence.

1. Identify the intended Hyper-V guest and confirm that the connection reaches it. Read its installed build plus update revision, exact `BuildLabEx`, architecture, and Insider channel/track. Follow [first-touch.md](references/first-touch.md) for the read-only inventory and comparison procedure.
2. Fetch [Microsoft Flight Hub](https://learn.microsoft.com/en-us/windows-insider/flight-hub/) during this session and open the applicable release announcement. Resolve the latest released build for the guest's architecture and channel/track, including servicing revisions and any withdrawal or superseding notice. Do not hardcode a “latest” build or choose the numerically largest value across unrelated tracks.
3. Run the bundled comparison utility against the observed guest data and the freshly read release reference. If offline or unable to establish the applicable release, report **freshness unknown** instead of treating the VM as current. The utility compares supplied evidence; it does not fetch or authenticate source contents itself.
4. Tell the researcher the result before continuing: guest build/revision, reference build/revision, channel/track, source URL, retrieval time, and either **behind**, **matches reference**, or **freshness unknown**. When behind, explicitly warn that the guest is older than the applicable release and recommend updating the selected VM before final validation. A matching build is not a bounty-eligibility determination.
5. Retain a concise readiness record with the session's evidence, keyed to the selected VM and its current boot/build. Recheck after an update, reboot, checkpoint restore, guest replacement, or channel change; also recheck before final validation if the release lookup is more than 24 hours old. Do not repeat the same warning on every unchanged guest operation.

Microsoft has announced Canary-to-Experimental transitions, while the bounty page may still use Canary terminology. Read current sources when labels differ. Preserve the exact channel and track, warn about the mismatch, and leave eligibility unresolved; do not silently equate every Experimental build with Canary. Historical Canary rows in Flight Hub are not necessarily the current release stream.

## Environment and evidence boundaries

- Keep Windows validation in the selected guest. A Linux host, WSL instance, Windows container, retail Windows installation, or the Hyper-V host does not establish behavior on the requested Insider guest.
- Prefer a stock guest with its normal security settings. Record changes to security posture and distinguish guest behavior from host behavior. Hardware-dependent behavior may require an appropriate physical system; a VM does not establish hardware fidelity.
- Use the operator's existing guest connection and credential handling. This plugin supplies guidance and a local comparison utility, with no MCP tools, VM lifecycle service, credential collection, or arbitrary guest executor.
- Treat an outdated or unavailable VM as a warning and an evidence limitation. Continue source review or other useful work when appropriate; do not silently update, reboot, restore, enroll, or change the VM's channel. Those actions are separate operator-controlled setup work.
- Keep environment preparation bounded. After one clear readiness failure, record it and return to work that does not depend on the guest instead of repeatedly repairing the environment.

Read [environment.md](references/environment.md) for setup sources and limitations. Consult the current Microsoft program page before making eligibility claims; this skill does not encode award amounts or attack-scenario procedures.
