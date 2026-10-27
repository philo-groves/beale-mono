# Releasing Beale

Beale, app-server, and the two runtime packages advance together. The current coordinated candidate is `0.9.0`. A version number in source is not a published release. The macOS and Windows source installer clones the `release` branch and builds locally. Native installers with in-app updates are published by the Release workflow on a version tag.

## Change lanes

- Pull requests and pushes to `main` run `.github/workflows/ci.yml`: frozen install, version consistency, TypeScript build, fast suites, and Desktop production bundle.
- Before a release tag, run the **Release gate** workflow on the candidate commit from the Actions tab. It runs `pnpm release:verify` on macOS and Windows, including the hosted integration suites.
- A `vX.Y.Z` tag repeats the release gate. The tag must be annotated, match every package and release-facing version, point to the tested commit, and be reachable from `main`. The workflow only verifies; it does not publish or upload an installer.
- After merging verified source into `release`, zip the `website/beale-source-installer` directory and update the WordPress plugin on beale.ai. Check that `https://beale.ai/i` returns the exact `install.mjs` content before advertising the command.
- The same tag starts `.github/workflows/release.yml`, which builds a macOS arm64 dmg and zip and a Windows x64 NSIS installer with `pnpm package:desktop`. Its publish job waits for approval in the `release` environment, then creates the GitHub Release with the installers, `latest*.yml` update feed files, and `SHA256SUMS.txt`. Installed builds read that feed.

Protect `main` with the CI `fast` check and require review before merging. Restrict who may create or replace `v*` tags. Keep the release gate result attached to the exact candidate commit; rerun it after any candidate change.

## Prepare a candidate

1. Complete the candidate's numbered changelog section, grouped by Beale and app-server, and leave `## Unreleased` ready for subsequent changes. Describe migrations, compatibility requirements, and operator actions. Keep all examples synthetic and review the full diff for live workspace content and machine-local data.
2. For a new candidate version, run `pnpm release:version X.Y.Z`. This updates the five package manifests and the release-facing runtime identifiers. Keep independent managed plugin versions and the iOS repository's version on their own lifecycle.
3. Run `pnpm install --frozen-lockfile`, `pnpm release:check`, and `pnpm release:verify` locally. Live-provider tests are opt-in and require operator-supplied credentials.
4. Merge the candidate. Run the manual Release gate on the merged commit and review both OS results. Fix failures and rerun on the new commit.
5. Stop all app-server processes before taking a restorable backup of `~/.beale/memory.sqlite` and the affected research workspaces. Verify the backup can be read. Test any migration on a copy before upgrading an irreplaceable workspace.
6. Create an annotated `vX.Y.Z` tag on the verified commit and push it. Confirm the tag-triggered Release gate passes.

Do not treat a green source gate as installer validation. Approve the `release` environment only after the tag's Release gate passes on both operating systems.

## Installers and updates

`pnpm package:desktop` builds the workspace, stages the app-server runtime under `apps/desktop/build/runtime` (`apps/desktop/scripts/stage-runtime.mjs`), and runs electron-builder with `apps/desktop/electron-builder.yml`. Installed builds run app-server with Beale's own Electron binary in Node mode, so they do not need a system Node.js. The packaged app-server runs headless; the tray host still needs a source checkout.

Installed builds check the latest GitHub Release a few seconds after launch and every four hours. When a newer version exists, the header shows **Update to X.Y.Z**. The download starts only when the user clicks it, and **Restart to update** stops the local app-server first. While research sessions are active the app-server refuses to stop, so the restart waits and the button explains why. Updating Desktop does not update connected remote app-servers.

One-time repository setup:

- Create a `release` environment with required reviewers.
- For macOS, add `MACOS_CERTIFICATE` (base64 Developer ID Application .p12), `MACOS_CERTIFICATE_PASSWORD`, `APPLE_API_KEY` (App Store Connect .p8 contents), `APPLE_API_KEY_ID`, and `APPLE_API_ISSUER`. macOS applies updates only to signed apps; unsigned builds install but cannot update in place.
- For Windows, optionally add `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD`. Unsigned installers update but show SmartScreen warnings.

Before the first public release, install the previous published build on each OS, publish the candidate, and confirm the in-app update relaunches into the new version, reconnects to app-server, and keeps workspaces intact.

## Recovery

If a candidate fails, fix it on a new commit and rerun the gate. Do not move an existing release tag. If an installed build fails after a data migration, stop app-server and restore the matching binary, global database, and workspace backup together. Older binaries are not assumed to read newer data. Document any migration or restore procedure in the release notes before distribution.
