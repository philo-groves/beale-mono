---
name: meta-bug-bounty-tools
description: Use Meta Bug Bounty researcher tools and the Muse VM boundary harness when the active Beale workspace uses the Meta Bug Bounty Research Kit. Do not apply to other research kits or ordinary Meta development.
---

# Meta Bug Bounty tools

Use the [Meta Bug Bounty tools page](https://bugbounty.meta.com/tools/) to choose the tool that matches the current authorized research question. Check the workspace's imported scope and rules against Meta's current [scope](https://bugbounty.meta.com/scope/) and [terms](https://bugbounty.meta.com/terms/) before live testing. This skill provides usage guidance; it does not grant access to Meta tools or expand the workspace's authorization.

| Tool | Use it for |
| --- | --- |
| [SSRF validator](https://www.facebook.com/whitehat/ssrf/attempts) | Confirm a suspected server-side request forgery with Meta's validation workflow. Keep the submitted URL and observed result tied to the specific authorized hypothesis. |
| [Test accounts](https://www.facebook.com/whitehat/accounts) | Create and manage dedicated Facebook test accounts for reproducible account and relationship scenarios. Keep test identities separate from real users. |
| [FBDL](https://www.facebook.com/whitehat/fbdl) | Set up complex Facebook test environments with Facebook Bug Description Language when the scenario needs repeatable social objects or relationships. |
| [Access token debugger](https://developers.facebook.com/tools/debug/accesstoken/) | Inspect details of a token and its owner when validating an authorized token or permission question. Record only the necessary conclusions and redact token values from research artifacts. |
| [Graph API explorer](https://developers.facebook.com/tools/explorer/) | Make focused Graph API calls with an authorized test app or account to reproduce and compare API behavior. Preserve the relevant request and response evidence without credentials. |
| [m.facebook.com request logger](https://bugbounty.meta.com/login/?next=%2Fprofile%2F) | Inspect HTTP requests made on the researcher's behalf while browsing m.facebook.com; Meta describes the output as appearing in the browser JavaScript console. |

Some tools require a Meta login or program access. Follow the linked product's current UI and documentation when access is available. If a tool is unavailable, record that limitation and use another authorized way to verify the hypothesis. Do not treat a tool's presence on the page as authorization for a target outside the workspace's recorded scope.

## Muse Secure VM boundary research

Meta [describes Muse's runtime cell and host-side safeguards](https://research.meta.ai/blog/security-and-safety-for-ai-agents-our-approach-with-muse). Muse's public bug bounty is open to responsible reports, while the [product rollout](https://about.fb.com/news/2026/09/introducing-muse-personal-ai-agent/) is through Muse's ordinary US app and web access. Public documentation does not say bounty membership grants a host shell or a separate research VM. The later Muse Confidential VM program is a separate limited-access effort. Check current program scope and the researcher's actual Muse access before live work.

For an owned Muse instance, use `scripts/muse-vm-harness.mjs` to create a run under the workspace's `evidence/` directory. Its `muse-runtime-probe.sh` collects only the current UID mapping and Linux capability masks from the runtime cell. It does not execute on Meta's VM host, touch credential stores, enumerate other users, or send network traffic. Transfer and run it only through the normal Muse interface if that interface permits running an owned script. Save the resulting console output as a capture file, then assess it locally:

```text
node scripts/muse-vm-harness.mjs init --out <workspace>/evidence/muse-boundary-run
sh muse-runtime-probe.sh <run-id> > capture.txt
node scripts/muse-vm-harness.mjs assess --run <workspace>/evidence/muse-boundary-run --input <workspace>/evidence/muse-boundary-run/capture.txt
```

The middle command runs **inside the researcher's Muse runtime cell**, if available; the other commands run locally. Preserve the original transcript and the generated `assessment.json` together. A baseline deviation is a lead for a bounded follow-up, not a VM escape conclusion. A report of host or cross-user access requires separate reproducible evidence from an authorized test, with no real credentials or other users' data collected.
