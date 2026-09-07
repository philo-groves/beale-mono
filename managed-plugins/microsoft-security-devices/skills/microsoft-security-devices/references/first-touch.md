# First-touch version check

Run the inventory inside the selected Windows guest through an existing operator-configured connection. Do not run it on the Hyper-V host and label the result as guest evidence. Use the 64-bit guest PowerShell environment:

```powershell
$guestVersion = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
[pscustomobject]@{
  CurrentBuildNumber = $guestVersion.CurrentBuildNumber
  UBR = $guestVersion.UBR
  BuildLabEx = $guestVersion.BuildLabEx
  BuildBranch = $guestVersion.BuildBranch
  DisplayVersion = $guestVersion.DisplayVersion
  Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
  ObservedAt = [DateTime]::UtcNow.ToString('o')
} | ConvertTo-Json -Compress
```

This query reads version metadata only. Inspect the guest's **Settings > Windows Update > Windows Insider Program** for channel and track. Preserve an unknown channel when unavailable; a build number alone does not prove enrollment. Retain the selected VM identity and boot/checkpoint identity with the session evidence, without copying credentials or account details.

Combine `CurrentBuildNumber` and `UBR` as `build.revision`. Compare those integers numerically: revision 10 is newer than revision 9. Keep `BuildLabEx` verbatim as separate provenance; its embedded revision is not a substitute for UBR. Do not use marketing versions such as “Windows 11” or “26H1” for numerical comparison. Missing UBR is unknown, not zero.

Fetch Flight Hub live, then read the announcement for the applicable channel/track and architecture. A blank channel column does not announce a release for that channel. Confirm that the referenced release is not withdrawn and that the branch is applicable; do not compare different tracks merely because one has a larger build number. If sources disagree or only historical rows are available, pass `latest: null` and warn that freshness is unknown. For a documented Experimental successor, compare only the same exact track and preserve the separate Canary eligibility warning.

Run `node check-canary.ts <input.json>` using the file at `scripts/check-canary.ts` within this skill directory. Resolve that path from this skill, not from the research workspace or an assumed global installation. Node 22.19 or newer is required. The input file must contain only these observed/derived fields:

```json
{
  "sessionStartedAt": "2026-01-01T10:00:00.000Z",
  "guest": {
    "build": "99000.9",
    "buildLabEx": "99000.1.amd64fre.example_branch.260101-0000",
    "architecture": "x64",
    "channel": "Canary",
    "track": "example-track",
    "observedAt": "2026-01-01T10:01:00.000Z"
  },
  "latest": {
    "build": "99000.10",
    "architecture": "x64",
    "channel": "Canary",
    "track": "example-track",
    "sourceUrl": "https://learn.microsoft.com/en-us/windows-insider/flight-hub/",
    "publishedAt": "2026-01-01T09:00:00.000Z",
    "checkedAt": "2026-01-01T10:02:00.000Z"
  }
}
```

All values above are synthetic examples, not a current build recommendation. Populate the input from actual guest observations and the live source; do not copy the example timestamps. If the source gives only a publication date, use `YYYY-MM-DD` for `publishedAt` rather than inventing a publication time. `track` is the exact applicable release stream established from the guest settings and official announcement, not an invented match. `latest: null` represents an unavailable or ambiguous release reference.

The utility returns bounded JSON with status, warnings, compared versions, and source provenance. It never contacts a guest or network, changes files, updates Windows, or persists a cross-session cache. It requires observations and lookup timestamps from this session and within 24 hours. A future timestamp, architecture/channel/track mismatch, missing provenance, or guest ahead of the supplied reference yields an unknown result. Same-track Experimental comparisons retain a channel-review warning even when the numbers match.

Surface warnings in session commentary. Save the input and result through existing workspace artifact/evidence facilities, scoped to that session and VM. The comparison alone does not authenticate the source, certify Hyper-V isolation, enforce a runtime hook, or prove a vulnerability. Re-run after a boot/build/checkpoint change or stale lookup, and use the current program terms for final eligibility review.
