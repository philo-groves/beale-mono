# Graph definitions

`graphs.json` is the bundled source for the four Graphs topologies. Each graph declares its entry profile, scheduler, ordered nodes and layout, allowed delegation, and peer handoffs. `profiles.json` supplies profile IDs, labels, instructions, spawn guidance, execution kind, context requirements, model choice limits, and concurrency bounds. Both files use `schema_version: 1`; references such as `mapper@1` identify profile revisions. Runtime selection and desktop rendering use catalog-backed `ProfileId` values and maps, with no fixed Role enum or role-indexed arrays. Adding a bundled task profile requires a JSON catalog and graph update; profile files are not loaded from user directories.

The five current task profiles are:

| Profile | Spawn when | Deliverable |
| --- | --- | --- |
| Mapper | An area needs coverage or its structure is unknown | Entry points, trust boundaries, dependencies, and coverage gaps |
| Investigator | A concrete question or independent lead needs focused analysis | Observations, assumptions, counterevidence, and the next decisive check |
| Experimenter | An uncertainty can be resolved through a bounded experiment | Reproducible procedure, artifacts, and observed results |
| Challenger | A consequential claim needs independent scrutiny | Alternative explanations, missing prerequisites, and unresolved objections |
| Builder | Research needs an implementation artifact | Harness, reproducer, instrumentation, regression test, or patch |

Persistent graph agents retain stage ownership. The graph declares which worker profiles each stage can spawn. Simple Orchestra enables all five; pipeline stages expose relevant subsets. Models, per-profile concurrency, scan worker budget, initial prompt, and sequential stage duration remain scan inputs.

## Worker tasks and results

New worker assignments require a bounded `contract` object with `question`, `scope`, `evidence_refs`, `deliverable`, `completion_criteria`, `context_policy`, and `budget`. The budget specifies `max_model_turns` (1–128) and `max_duration_minutes` (1–120). Each contract is limited to 16 KiB and saved with the child transcript, including canceled work. Duration starts when the queued worker acquires capacity. The host enforces model-turn and duration limits and retains partial activity when work is interrupted.

`context_policy` is `shared` or `evidence_only`. Both start with fresh task context. Shared workers can inspect or message siblings in the same parent group. Evidence-only workers can contact their parent for clarification but cannot inspect or message siblings; Challenger requires this policy through its profile definition. This controls graph context exchange; it does not isolate the workspace or hide artifacts accessible through enabled tools.

Workers return `task_result@1` JSON with `completion` (`complete`, `incomplete`, `blocked`), `assessment` (`supported`, `contradicted`, `inconclusive`, `not_applicable`), `summary`, `observations`, `evidence_refs`, `artifacts`, and `uncertainties`. Supported or contradicted assessments require evidence references. Invalid envelopes fail the task. Budget exhaustion and cancellation produce an incomplete, inconclusive result. Task completion never advances a durable finding; stage ownership and finding evidence checks remain authoritative.

One worker semaphore and per-profile limits are shared by every parent in the same scan. The default scan limit is 8, configurable from 1 to 64. All five profiles currently cap concurrency at 8 each. Queued work waits for both capacities, and cancellation releases permits. Persistent coordinators use the graph's separate top-level cap, so workers can finish while their parent waits. The mutation lock is also shared across the scan's workers. Model/provider request limits and token budgets are not implied by the concurrency setting.

## Peer handoffs and custom templates

`task@1` remains a bounded free-text peer handoff; it is distinct from a worker task contract. `ranked_targets@1` requires an ordered batch of targets with ranking signals, rationale, and evidence references. Only the Audit Targets to Discovery route uses that ranked contract.

Custom peer templates can use bundled profiles. Place a standalone JSON definition in the app-server data directory's `graph-templates/` folder; [example-review.json](examples/example-review.json) is synthetic. Desktop lists valid templates in New Scan. Each file is limited to 16 KiB, and the host reads at most 64 templates. Invalid files are ignored. Every persistent profile must have an allowed task delegation. Custom single-agent templates are not supported; use No Graph or Simple Orchestra.

Custom template IDs include a SHA-256 digest of file contents. A new scan saves its selected JSON definition; later edits create a new ID while existing scans restore their snapshot. Bundled graph IDs remain stable and use the installed topology, including on resume. Legacy Scout, Reviewer, and Prover profiles remain readable and usable by saved custom templates; new bundled graphs use the five current profiles. Existing saved model selections remain stored, but selections for retired bundled workers do not configure their replacements. Profile instructions at a given revision remain stable; instruction changes require a new revision.

Protocol 2.34 adds the persisted worker budget to scan creation and detail. Missing or zero values use the default of 8. No storage schema migration is required. Update connected desktops and app-servers together.
