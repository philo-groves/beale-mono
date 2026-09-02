# Apple Target Flags

`apple-target-flags` is a portable Agent Plugin that teaches research agents how Apple Security Bounty Target Flags map to exploit primitives and what evidence confirms each capture.

The plugin covers userspace and kernel commpage flags for register control, arbitrary read/write, and code execution, plus the TCC integrity flag. It also defines evidence boundaries for same-boot correlation, UID-0 userspace execution, kernel panic analysis, negative results, and claims that exceed the captured primitive.

The skill follows Apple's current public guidance at <https://security.apple.com/bounty/target-flags/>. Agents must recheck that page and the applicable bounty category before preparing a report because eligibility requirements can change.

## Import

In Beale, open Plugins, choose **Add Agent Plugin**, and select this directory. Beale discovers `skills/apple-target-flags/SKILL.md`.

This plugin is guidance-only. It does not include an MCP server, run proof-of-concept code, change TCC state, collect crash reports, or submit bounty reports.
