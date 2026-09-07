# Microsoft Security Devices

An optional managed Agent Plugin for Windows environment selection and first-touch build freshness. Prefer an operator-prepared Windows Insider Preview Canary Hyper-V guest, read the installed guest build and UBR, and compare against current Microsoft release guidance before substantive guest work.

In Beale, open **Plugins**, choose **Add from Filesystem**, and select this plugin directory. It is not installed or enabled automatically. Once added, it follows the normal plugin Enable/Disable control; disabling removes its skill from subsequent session runtimes.

This initial version is guidance-only, with an empty MCP server configuration and a portable TypeScript comparison utility. It adds no model-facing tools or native VM-operation interceptor. First-touch scheduling, guest inventory, live Flight Hub lookup, warning delivery, and evidence retention are instructions followed by the research agent using existing tools. The comparison utility does not perform those operations itself.

The skill requires a fresh check for each session and selected VM, with rechecks after guest state changes or stale release lookups. It warns when behind, preserves uncertainty for missing or mismatched evidence, and handles Canary/Experimental naming differences without asserting bounty eligibility. It does not hardcode current builds, update the guest, or manage accounts, credentials, Hyper-V lifecycle, networking, or checkpoints.

Read [the skill](skills/microsoft-security-devices/SKILL.md) and [the check input contract](skills/microsoft-security-devices/references/first-touch.md). The utility requires Node 22.19 or newer and runs without third-party dependencies. Its JSON result is an assessment of supplied observations, not proof that a network fetch or guest inspection occurred.

Microsoft's [Windows Insider Preview bounty page](https://www.microsoft.com/en-us/msrc/bounty-windows-insider-preview) is authoritative for program requirements. Hyper-V is the preferred Beale lab environment, not a hypervisor mandate on that page.
