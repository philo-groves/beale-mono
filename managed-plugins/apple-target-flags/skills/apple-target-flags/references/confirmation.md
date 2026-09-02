# Target Flag confirmation

Use this reference to plan or audit a Target Flag evidence packet. Recheck Apple's current [Target Flags guidance](https://security.apple.com/bounty/target-flags/) before relying on the mappings below.

## Commpage capture matrix

| Context | Claimed primitive | Required target | Confirmation evidence |
| --- | --- | --- | --- |
| Userspace | Register control | `_COMM_PAGE_ASB_TARGET_VALUE` | Vulnerable-process crash report exposes the current-boot value in a general-purpose register. Record whether control is full-width or partial. |
| Kernel | Register control | `_COMM_PAGE_ASB_TARGET_KERN_VALUE` | Kernel panic exposes the current-boot kernel value in a causally relevant general-purpose register. |
| Userspace | Arbitrary read | `_COMM_PAGE_ASB_TARGET_ADDRESS` | Vulnerable-process crash plus decoded faulting instruction establishes a load from the current-boot userspace target address. |
| Kernel | Arbitrary read | `_COMM_PAGE_ASB_TARGET_KERN_ADDRESS` | Kernel panic plus exact-build instruction decoding establishes a load from the current-boot kernel target address. |
| Userspace | Arbitrary write | `_COMM_PAGE_ASB_TARGET_ADDRESS` with `_COMM_PAGE_ASB_TARGET_VALUE` | Vulnerable-process crash plus decoded faulting instruction establishes the target-value store to the target address. |
| Kernel | Arbitrary write | `_COMM_PAGE_ASB_TARGET_KERN_ADDRESS` with `_COMM_PAGE_ASB_TARGET_KERN_VALUE` | Kernel panic plus exact-build instruction decoding establishes the kernel target-value store to the kernel target address. |
| Userspace | Code execution | `_COMM_PAGE_ASB_TARGET_ADDRESS` | Vulnerable-process crash report has raw instruction pointer equal to the current-boot userspace target address. |
| Kernel | Code execution | `_COMM_PAGE_ASB_TARGET_KERN_ADDRESS` | Kernel panic has raw instruction pointer equal to the current-boot kernel target address. |

Apple's public guidance requires a PoC and accompanying crash log for commpage demonstrations used with kernel or userspace privilege-escalation reports. A crash is not automatically exploitable: NULL dereferences, assertions, or unrelated fault causes remain contrary evidence even if a target value happens to be live elsewhere.

## Boot and run binding

The address/value pair is meaningful only for the boot that produced it.

1. Record OS version and build, device or guest model, boot-session identity, security posture, PoC identity or revision, and initial caller privilege.
2. Read the applicable address and value from the live commpage immediately before the terminal stage.
3. Write the tuple and run identifier to durable output; flush and synchronize it before arming a terminal crash when practical.
4. Retain the newly generated crash or panic and exclude stale diagnostic reports.
5. Verify that the retained artifact belongs to the expected process, build, boot, and run.
6. Compare full-width hexadecimal values. Preserve leading zeroes and avoid comparisons based only on symbolicated addresses or truncated display fields.
7. Hash the final artifacts and record the verifier result.

A tuple from another boot, a stale crash report, or an unbound console transcription does not confirm a capture.

## Instruction-level checks

Do not infer the primitive from an address match alone.

- For a read, decode the exact faulting load and identify the register or addressing expression that equals the target address.
- For a write, decode the exact faulting store and identify both the destination expression equal to the target address and the source operand equal to the target value.
- For code execution, compare the raw thread-state instruction pointer with the target address. Symbolication, pointer canonicalization, or crash-frame rendering may display a transformed address; retain and explain both representations rather than silently normalizing them.
- For kernel evidence, use the exact kernel image and build metadata. Register-to-stack or register-to-frame relationships demonstrated on one build require independent correlation on another build.

## UID-0 userspace code execution

For a privilege-escalation chain that reaches root and then executes a flag helper, retain all of:

- evidence that an initially non-privileged caller reached the privileged launch path through the claimed exploit;
- root-stage identity output showing real/effective identity as applicable;
- the helper's same-run userspace target address, written before the jump;
- the attributable helper crash report with `userID` 0 and raw `pc` equal to that address;
- process, parent, launch-time, and artifact hashes sufficient to exclude a manually launched or stale helper.

This confirms userspace code execution in a UID-0 process. It does not confirm kernel code execution, and it does not prove the preceding vulnerability unless the launch provenance is independently established.

## Kernel panic captures

For kernel register, read, or write captures, a strong packet contains:

- a same-boot durable target tuple recorded before the trigger;
- a sole-new or otherwise uniquely attributable panic;
- matching target registers and fault address as required by the primitive;
- exact-build relocation and disassembly of the faulting instruction;
- a concise operand-level explanation of why the instruction proves register control, load, or store;
- an explicit statement of what is not proved, especially when PC control was not achieved.

An access that is swallowed by a no-fault or recovery path may prove the access mechanism, but without the required crash or panic it is not a completed commpage Target Flag capture.

## TCC capture

Use `tccutil flag check` and `tccutil flag reset` exactly as documented by Apple.

1. In an authorized disposable environment, record SIP state and which user and system databases report `default` or `modified`.
2. With operator confirmation, reset the integrity flag to establish a controlled baseline. Resetting is state-changing even though Apple states that this verb does not alter other TCC selections.
3. Run only the PoC under evaluation. Direct SQLite modification is a harness sanity check, not evidence that the vulnerability changed TCC.
4. Run `tccutil flag check` and retain its output identifying whether the user or system database reports `modified`.
5. Preserve video or logs that bind the baseline, PoC, and final check to one run.

Claim only the database boundary actually modified. For a system-database claim, retain evidence that SIP was enabled. The integrity flag grants no permission itself; it is an evidence marker for database modification.

## Failure classifications

- **Not attempted:** no terminal flag action was run.
- **Harness validated:** a direct helper or direct database edit proved the capture harness works, but no vulnerability path reached it.
- **Attempted, not captured:** the exploit ran but required target state is missing, stale, mismatched, recovered, or not present in the crash artifact.
- **Captured primitive:** same-run artifacts satisfy the relevant matrix row and exploit provenance is established.
- **Report-ready:** the capture is accompanied by reachability, root-cause, affected-build, realistic exploitability, reproduction, and contrary-evidence analysis required by the applicable bounty category.

Do not collapse these states. In particular, `Captured primitive` and `Report-ready` are not synonyms.
