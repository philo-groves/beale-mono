---
name: apple-target-flags
description: Select, capture, and assess Apple Security Bounty Target Flags for userspace, kernel, privilege-escalation, or TCC findings. Apply when planning an Apple exploit proof, interpreting a crash or panic as a flag capture, checking report readiness, or deciding exactly which primitive the evidence confirms.
---

# Apple Target Flags

Use Target Flags to demonstrate the exploit primitive actually reached by an authorized Apple security test. A flag capture is evidence of a primitive; it does not by itself prove attacker reachability, a vulnerability, real-world exploitability, bounty eligibility, or a reward amount.

Before a final capture or report-readiness decision, read Apple's current [Target Flags guidance](https://security.apple.com/bounty/target-flags/) and the applicable Apple Security Bounty category. Treat those live sources as authoritative when they differ from this skill.

## Select the target

Apple currently defines two flag families:

- **Commpage:** boot-random values and target addresses for userspace and kernel register control, arbitrary read/write, and code execution.
- **TCC:** an `integrity_flag` record used to demonstrate modification of the per-user or system TCC database.

Use the userspace commpage pair for a userspace process, including a process reached after privilege escalation. Use the kernel pair only for a kernel primitive. UID 0 is still userspace; never relabel root process execution as kernel execution.

Use symbolic definitions from current Apple documentation or matching system headers:

- `_COMM_PAGE_ASB_TARGET_VALUE` and `_COMM_PAGE_ASB_TARGET_ADDRESS`
- `_COMM_PAGE_ASB_TARGET_KERN_VALUE` and `_COMM_PAGE_ASB_TARGET_KERN_ADDRESS`

The contents are randomized at boot. Do not reuse a captured value or address after a reboot, and do not substitute a convenient attacker-chosen marker for an Apple Target Flag.

## Confirm the primitive

Read [confirmation.md](references/confirmation.md) before designing a capture or evaluating its evidence.

Apply these minimum conclusions:

- **Register control:** the vulnerable userspace process or kernel crash exposes a general-purpose register equal to the appropriate target value. State the demonstrated bit width. A matching value in an unrelated register is not enough when the crash was caused by an unrelated NULL dereference or assertion.
- **Arbitrary read:** the crash or panic and exact faulting instruction establish a load from the appropriate target address through the vulnerable primitive.
- **Arbitrary write:** the crash or panic and exact faulting instruction establish a store of the appropriate target value to the appropriate target address through the vulnerable primitive.
- **Code execution:** the raw instruction pointer equals the appropriate target address. On Apple silicon, evaluate the raw `pc` in the thread state, not only a symbolicated or canonicalized crash-frame display.
- **TCC modification:** after a controlled baseline/reset, the PoC changes the intended database and `tccutil flag check` reports that database as `modified`.

For every commpage capture, correlate the target tuple and crash artifact from the same boot and run. For kernel read/write claims, resolve the faulting PC against the exact kernel build and show how the decoded instruction's operands bind the recorded registers to the target address and, for writes, target value.

## Preserve exploit provenance

The flag stage must be reached because of the exploit chain being evaluated.

For a privilege-escalation finding, a minimal helper may read the current userspace target address and jump to it after the exploit obtains UID 0. Confirm both that the exploit launched the helper in the elevated context and that the resulting crash report records `userID` 0 with raw `pc` equal to the same-run target address. A standalone helper started by an already-privileged operator is only a harness self-test.

For a kernel finding, record the current kernel target tuple durably before the terminal trigger, then bind it to the resulting panic from the same boot. A non-crashing access recovered by fault handling can support primitive analysis but is not a Target Flag capture when the required crash evidence is absent.

## Report conservatively

- Keep register control, read, write, and code execution as separate claims unless one artifact independently proves more than one primitive.
- Preserve negative and contrary evidence, including failed captures, recovered faults, mismatched boots, and target values present in registers without a causally relevant instruction.
- Hash retained crash, panic, log, and verifier artifacts. Record OS build, hardware or VM model, boot identity, process identity, privilege context, security posture, PoC revision, and exact reproduction steps.
- Distinguish direct evidence from inference. Do not promote a flag result into a confirmed vulnerability conclusion without the tool, artifact, or verifier references that connect the exploit path to the capture.
- Use a stock device or stock guest for final environment-sensitive conclusions. A modified research kernel may establish mechanics but not stock exploitability.
