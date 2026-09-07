# Windows Insider Hyper-V environment

Use a dedicated, operator-prepared Hyper-V guest for Windows validation. Keep the host's operational state distinct from guest observations. Beale runs with the user's host privileges; the operator manages Hyper-V isolation, networking, shared resources, accounts, and VM lifecycle.

Microsoft's [Hyper-V creation guidance](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/get-started/create-a-virtual-machine-in-hyper-v) describes prerequisites and recommends Generation 2 unless there is a specific reason otherwise. Follow current Windows 11 guest requirements. Use supported installation media, licensing, and the [Windows Insider enrollment guidance](https://learn.microsoft.com/en-us/windows-insider/get-started); do not embed accounts or credentials in prompts, scripts, or records.

The [bounty program](https://www.microsoft.com/en-us/msrc/bounty-windows-insider-preview) calls for the latest applicable Canary build and the tested revision's `BuildLabEx` in submissions. It does not mandate Hyper-V. Keep normal security settings and record any deviation. A snapshot provides a recovery point, not proof of a stock configuration or current build.

Use [Flight Hub](https://learn.microsoft.com/en-us/windows-insider/flight-hub/) and its linked announcements for release selection. Microsoft's [channel-transition announcement](https://blogs.windows.com/windows-insider/2026/04/10/improving-your-windows-insider-experience/) explains that Canary users move to Experimental according to their Windows core version. Track names, architectures, release availability, and bounty terminology can diverge; report ambiguity rather than inventing equivalence or downgrading a newer guest to match a historical Canary row.

No VM creation, checkpoint restore, OS installation, channel enrollment, update, reboot, or security-setting change is performed by this plugin. Readiness warnings do not authorize those operations. An operator can prepare or update the selected guest separately, then repeat the first-touch check.
