# Swarms and Fleet implementation plan

Status: Implementation is complete for Stages 1-10. The final Stage 10 review found no remaining implementation blockers in the successful-completion finalization fix and regression coverage. Stage 10 remains open for the explicitly outstanding interactive and platform/deployment validation recorded below.

This plan targets Beale v2. It records the agreed behavior and the engineering work needed to deliver it. Only completed stages and their progress notes describe implemented behavior; remaining sections describe planned behavior. Stages 1-9 are complete within the boundaries recorded below. Stage 10 records both exercised acceptance boundaries and remaining deployment/platform validation. Earlier-stage validation remains recorded at each checkpoint below.

## Scope and ownership

Swarms defines agent roles, delegation, and workflows. Fleet supplies execution environments and allocation. The leader's app-server owns the scan, canonical research records, provider routing, and durable orchestration state. Desktop remains a client.

Fleet remains a default capability with no agent-facing Fleet tools. Swarms remains optional. New scans created with Swarms disabled or **No Subagents** use the ordinary local research agent without swarm orchestration or the leader-only review rule. Fleet destination selection applies only to swarm deputies/workers in this release; remote single-agent execution is outside this plan.

Disabling Swarms changes eligibility for new scans, not the configuration of existing scans. Running, paused, and retained scans keep their snapshotted hierarchy, destinations, and leader rule and may continue or resume. Ending an existing swarm is a separate explicit scan action; disabling the feature never silently converts it into a single agent. A build that cannot execute a saved swarm reports an unsupported configuration rather than downgrading it.

Docker is the initial execution backend. User-entered images are supported through preparation and validation, with Linux distributions such as Alpine and Ubuntu as explicit targets. Hyper-V, Tart, dedicated `dockurr/windows` support, and v1 compatibility are outside this implementation. Existing Hyper-V implementation may be discarded rather than preserved as a compatibility requirement.

The implementation targets the existing Rust components:

| Component | Responsibility |
| --- | --- |
| `beale-contracts` | Versioned desktop, peer, worker, and persisted configuration contracts |
| `beale-graphs`, renamed to `beale-swarms` | Declarative workflows, profiles, handoff schemas, and swarm tools |
| `beale-sessions` | Agent lifecycle, parent relationships, context, execution, and routing |
| `beale-fleet` | Peer inventory, Docker preparation, allocation, resources, and cleanup |
| `beale-providers`, `beale-tracking` | Inference routing, usage accounting, and pacing |
| `beale-safety` | Leader-specific auto-review |
| `beale-storage`, `beale-workspaces` | Canonical persistence, snapshots, artifacts, and retention |
| `beale-core`, Desktop | Host integration, settings, scan creation, and live projections |

## Stage tracking and dependencies

Mark a stage complete only after its implementation and acceptance tests pass. Record validation evidence and any deviations with the stage when work begins.

- [x] Stage 1: Contracts and durable state
- [x] Stage 2: Declarative Swarms and bundled workflows
- [x] Stage 3: Local hierarchy, tools, and leader review
- [x] Stage 4: Workspace snapshots, artifacts, and environment-storage contracts
- [x] Stage 5: Distributed routing and recovery
- [x] Stage 6: Docker preparation and worker execution
- [x] Stage 7: Resource allocation and retention enforcement
- [x] Stage 8: Provider accounting and token pacing
- [x] Stage 9: Desktop, Fleet, New Scan, and research-tool preferences
- [ ] Stage 10: Integrated validation and documentation — implementation complete and final review passed within tested boundaries; remaining interactive/platform validation stays open

| Stage | Deliverable | Depends on |
| --- | --- | --- |
| 1 | Contracts and durable state | None |
| 2 | Declarative Swarms and bundled workflows | 1 |
| 3 | Local hierarchy, tools, and leader review | 1-2 |
| 4 | Workspace snapshots, artifacts, and environment-storage contracts | 1, 3 |
| 5 | Distributed routing and recovery using supervised test workers | 1, 3-4 |
| 6 | Docker preparation, worker execution, and same-host environment recovery | 1, 4-5 |
| 7 | Resource allocation, cross-host environment recovery, and retention enforcement | 1, 3-6 |
| 8 | Provider accounting and token pacing | 1, 3, 5 |
| 9 | Desktop, Fleet, New Scan, and research-tool preferences | 2-8 |
| 10 | Integrated validation and documentation | All preceding stages |

Backend stages include their focused tests. Stage 10 validates their composition rather than postponing testing until the end.

Early stages validate contracts, stores, and local or simulated adapters. Stage 1 does not require live peer connections; Stage 3 does not require Docker or pacing enforcement; Stage 4 does not require container creation or restart. Stage 5 exercises real transport with supervised test workers without requiring Docker. Actual container filesystem retention and restart acceptance belongs to Stage 6, and cross-host restore, real allocation, and cleanup races belong to Stage 7. Earlier stages must not be marked incomplete merely because later integration acceptance has not run.

### Review amendment coverage

The following engineering resolutions address the eight initial plan-review findings and four follow-up findings. They are planned contracts and tests, not claims of implemented behavior.

| Finding | Resolution and acceptance ownership |
| --- | --- |
| Worker identity versus host control | Stage 1 defines scoped worker credentials and authoritative caller context; Stage 5 tests dispatch and revocation over transport |
| Execution authority versus running processes | Stages 1 and 5 define attempts, leases, process termination, and uncertain outcomes; Stages 6-7 test container and replacement behavior |
| Idle deputies and RAM reservations | Stages 1 and 3 define durable waiting/suspended states; Stage 7 tests admission, waking, and constrained-capacity progress |
| Retention races | Stage 1 defines lifecycle generations and cleanup claims; Stage 7 tests atomic resume, Keep, publication, and deletion |
| Account-wide pacing authority | Stage 1 defines opaque account-budget identity and ownership; Stage 8 tests multiple leaders, accounts, and coordinator failure |
| Leader-rule application and settings | Stages 1 and 3 define trusted role scope and versioned session rules; Stage 9 presents their scope and editing behavior |
| Circular stage acceptance | Stage 4 covers storage contracts and file adapters; Stage 6 owns same-host container recovery; Stage 7 owns cross-host recovery |
| Cross-platform snapshots and isolation wording | Stage 4 specifies preservation/rejection rules and tests; Stage 10 validates cross-platform routing without claiming an OS boundary |
| Single-slot spawn admission | Stages 1 and 3 separate queued assignment acceptance from execution admission; Stage 7 tests parent yield, contention, and genuine resource denial |
| No Subagents with selected destinations | Stages 1 and 9 reject invalid combinations and make local-only draft changes visible; Stage 10 verifies no silent local fallback |
| Disabling Swarms during an existing scan | Stages 1-3 retain existing scan snapshots and apply feature toggles to new scans; Stage 9 explains and tests running/paused/resumed behavior |
| Storage exhaustion after admission | Stages 4-5 inject checkpoint/publication/commit failures; Stage 7 protects retained state and Stage 10 verifies recovery after space returns |

## Stage 1: Contracts and durable ownership

Establish explicit types before replacing the existing runtime.

### Implementation

- Separate role (leader, deputy, worker) from specialist profile and individual agent identity.
- Persist scan configuration, the selected swarm definition and profile revisions, parent relationships, assignments, handoffs, lifecycle events, and resumable context.
- Model agent lifecycle separately from assignment completion and execution-instance state. Include durable waiting and suspended states, wake reasons, inbox cursors, and resumable checkpoints. An agent can finish an assignment or wait for children while retaining context and files without a running container.
- Define worker registration, capability negotiation, inference requests, canonical operations, events, artifact transfers, and execution ownership.
- Give retryable mutations stable operation IDs and explicit acknowledgements.
- Define container configurations separately from their allocated instances.
- Persist destination selections, resource reservations, pacing configuration, and retention metadata.
- Separate durable assignment acceptance from resource admission and execution start. Spawn results explicitly distinguish queued, starting/running, and denied; a queued result includes a stable child/assignment identity and wait reason and never claims an instance has started.
- Persist the scan's orchestration mode and feature/rule snapshot at creation. Feature settings govern new-scan creation; existing-scan resume resolves its saved mode independently of the current Swarms toggle.
- Move execution-relevant Fleet peer connection state into app-server ownership using the existing pairing model. Desktop-local connection presentation must not be required to keep research running.

### Identity, authority, and lifecycle contracts

- Keep paired host-control credentials on trusted app-server hosts. Never install them in a research container. Provision a separate short-lived worker credential bound to workspace, scan, agent, execution attempt, and the worker-protocol audience; support renewal and revocation.
- Resolve the caller's role, parent, authorized scope, enabled tools, provider routing, and applicable review rules from leader-owned state. Caller-supplied IDs and model text cannot change that context. Worker endpoints expose only the operations required for the assigned execution, not general host administration.
- Bind idempotency keys to operation identity and request content. A repeated key with different content is an error. An old execution credential cannot renew itself or mutate current assignment state after replacement.
- Persist an execution-attempt generation and renewable lease separately from the stable agent ID. Define a tool-operation journal distinguishing not started, running, completed, canceled, and outcome unknown. Lease expiry is not proof of process termination.
- Define environment references, lifecycle generations, cleanup claims, and publication acknowledgements. Resume, Keep, and cleanup eligibility must be serialized against the same authoritative lifecycle record.
- Define an opaque account-budget group, participant identity, accounting-owner generation, usage-event ID, and admission-reservation ID. Account identity is separate from provider ID and rotating credentials.
- Add typed review applicability and trusted agent-role context. Persist the active swarm's leader-rule revision alongside the scan configuration; do not derive applicability from prompt text.

Stage 1 introduces the versioned Swarms contract namespace. Stage 2 owns the existing Graph feature/crate/runtime rename and removal of obsolete Graph compatibility paths; it must not introduce aliases or fallback readers. Handle incompatible development state explicitly; do not silently reinterpret or delete it.

### Acceptance tests

- Contract round trips and rejection of malformed roles, invalid parent relationships, unknown states, and incompatible protocol versions.
- Replaying an acknowledged mutation returns the original result without creating another record.
- Reopening the store reconstructs pending assignments, handoffs, suspended agents, execution generations, and cleanup claims.
- Stored peer connection state is owned by the app-server and can be loaded without Desktop; live disconnect behavior is tested in Stage 5.
- Credentials, database handles, and machine-local configuration remain outside model-visible results.
- Contract/dispatcher tests reject an impersonated parent, wrong workspace or scan, expired/revoked worker credential, and worker requests for host administration.
- Reusing an operation key with different content fails; replacing an execution invalidates its prior mutation authority.
- Lifecycle compare-and-swap tests permit only one successful cleanup or resume claim for a generation.
- Account-budget identity survives credential refresh and distinguishes two accounts using the same provider.
- A No Subagents/new Swarms-disabled request with nonempty Fleet destinations is rejected before any research executes; saved draft combinations are validated by the same contract.
- Turning Swarms off prevents new swarm scans but does not remove existing scans' hierarchy, destination, or leader-rule snapshots. Unsupported-build resume fails explicitly instead of changing mode.
- Repeating a queued spawn request returns the same child/assignment identity; queued acceptance does not imply a resource reservation or running instance.
- Completed tool results, including JSON `null`, survive contract and store round trips and permit subsequent writes.
- Delayed checkpoints and tool-journal writes from expired, revoked, replaced, or mismatched attempts fail transactionally; initialization-only checkpoint APIs cannot bypass the execution fence.
- Ending a scan revokes every active worker's research/renewal authority without asserting process termination; reopening does not reactivate old credentials.
- Independent scans using separate database connections cannot claim the same host instance. Schema upgrades preserve valid existing ownership and reject conflicting ownership without modifying saved state.
- Environment registration racing with or following scan end inherits the persisted end deadline; premature deadlines fail and cleanup respects the exact expiry boundary.
- Identical acknowledged pacing reservations replay after ownership handover for registered participants; altered content, stale new reservations, and removed participants are rejected.
- Late Starting/Running instance reports for fenced/stopped attempts fail without creating ownership claims; termination-confirmed environments cannot retain live instance metadata. Registration racing with stop confirmation converges to stopped state or rejected registration.
- Duplicate stop confirmations preserve scan revisions and lifecycle generations, including existing cleanup claims and completed deletion acknowledgements.
- Trusted journal recovery resolves only existing operations from confirmed-stopped attempts. It preserves original attempt attribution, requires evidence references for known completion/cancellation, cannot restore worker authority or alter assignments, and commits outcome, audit event, and content-bound receipt atomically. Unknown outcomes can be resolved explicitly; known outcomes cannot be overwritten.

### Stage 1 progress and validation

Completed on 2026-11-25. No Stage 2 implementation was started.

Implemented boundaries:

- `beale-contracts::swarms` defines explicit JSON version 1 contracts; the host protocol is now 2.37. Bounded decoders validate versions, roles, operation shapes, required capabilities, and unknown fields. Roles, specialist revisions, assignments, attempts, instances, and environment lifecycles are distinct.
- `beale-storage::swarm_state::SwarmStore` owns validated scan documents, immutable configuration snapshots, queued acceptance, checkpoints, handoffs, lifecycle events, assignment transitions, execution generations, tool journals, instance ownership metadata, resource reservations, and retention claims. SQLite immediate transactions serialize mutations and generation checks. Mutation receipts bind operation IDs to request content and replay the original acknowledgement; failed transactions roll back both effects and receipts.
- Worker credentials contain separate random bearer material and are stored as digests. They bind scan/workspace, agent, attempt generation, and audience. Leases are bounded to fifteen minutes per issue/renewal. Dispatch derives role, parent, scope, tool allowance, provider routes, and leader-rule applicability from saved state. Expiry/revocation denies authority but does not establish process termination; replacement requires explicit host confirmation.
- `beale-storage::fleet_coordination::FleetCoordinationStore`, re-exported by `beale-fleet`, stores paired host connections, container configurations, stable account-budget identities, ownership generations, participants, usage events, and pacing reservations. App-server startup initializes this independent repository. It does not require Desktop or Swarms schema initialization. Peer control tokens and stable provider account keys are host-only; model projections contain only the explicit agent summary fields.
- Resume, Keep, pending publication references, retention deadlines, and cleanup claims use the same lifecycle generation. Resuming an ended scan clears its previous end clock atomically. The persisted defaults remain seven days after scan end and a 5 GiB storage reserve; actual capacity enforcement and deletion remain Stage 7.
- Current schema owners are `swarms` version 2 and `fleet-coordination` version 1. Swarms tables are initialized only when its repository is opened. The Swarms 1-to-2 migration adds and backfills exclusive host-instance claims; conflicting claims abort the entire migration and preserve existing documents/schema. Unknown saved contract/schema versions fail without rewriting or deleting state. No Graph/v1 conversion or compatibility reader was added.

Stage boundary clarification: the original Stage 1 wording coupled protocol names with the full Graph rename, while Stage 2 explicitly owns that replacement. Stage 1 adds the new contract and durable APIs without replacing the existing Graph feature, workflows, or UI. Host peer storage is available and tested independently; pairing/transport and Desktop presentation wiring remain Stages 5 and 9. Existing Desktop Fleet files are not imported or consulted as a fallback. Worker network endpoints, live inference/tool routing, model review execution, snapshots/artifact bytes, Docker, and resource/pacing schedulers are not implemented by this stage.

Acceptance evidence uses synthetic data and local dispatch adapters:

| Acceptance boundary | Executable coverage |
| --- | --- |
| Versioned round trips, malformed roles/states/parents, unknown authority fields, bounded decoding and required capabilities | `beale-contracts/tests/swarm_protocol.rs` (4 tests); `contracts_reject_malformed_roles_states_versions_and_parent_relationships` |
| Durable canonical mutation replay, changed-content rejection, rollback, new-generation reconciliation and old-attempt fencing | `acknowledged_database_mutations_replay_across_reopen_and_fence_old_attempts` |
| Reopen assignments, suspended checkpoints, handoffs, events, tool uncertainty, execution generations and cleanup claims | `reopen_reconstructs_checkpoints_handoffs_journal_attempts_and_cleanup_claims` |
| Host-owned peers without Desktop, independent Fleet initialization, credential redaction and exact model projection fields | `host_fleet_state_is_independent_of_desktop_and_model_projection_is_small` |
| Impersonation, wrong workspace/scan/attempt/audience, expiry/revocation, direct-child boundaries, host administration and trusted scoped rules | `authoritative_dispatch_rejects_impersonation_expiry_revocation_and_bypassed_parent` plus strict protocol decoder tests |
| Concurrent lifecycle claims across separate SQLite connections, publication protection, unconfirmed termination and reopened retention clock | `cleanup_resume_and_keep_are_serialized_across_independent_connections`; `publication_and_unconfirmed_execution_protect_retained_state` |
| Stable account grouping across credential changes/reopen, distinct provider accounts, owner-generation fencing and usage deduplication | `account_identity_survives_refresh_and_accounting_records_replay_without_merging_accounts` |
| Local-only draft validation, feature toggle applies to new scans, immutable saved hierarchy/destinations/rules and unsupported-build rejection | `feature_toggle_only_gates_new_scans_and_never_downgrades_saved_configuration` plus local contract decoder test |
| Queued replay preserves identities without reserving capacity or starting execution | `queued_spawn_is_idempotent_and_does_not_claim_admission` |
| Assignment completion retains agent context; execution-instance ownership and prior termination remain separate | `assignment_completion_does_not_destroy_agent_context_and_instances_have_one_owner` |
| Incompatible durable documents and future schema versions remain untouched | `incompatible_durable_state_is_rejected_without_reset_or_fallback` |

The twenty-six store/dispatcher tests are in `beale-storage/tests/swarm_foundation.rs`. Together with the four protocol tests, all thirty Stage 1 acceptance tests pass. Fifteen permanent tests were added across both implementation reviews: nine for the initial six findings, then six for the three follow-up findings.

#### Implementation-review fixes

All six findings from the Stage 1 implementation review were fixed and validated on 2026-11-25. These changes remain within Stage 1.

| Finding | Fix and permanent regression evidence |
| --- | --- |
| Completed null result made the saved scan unreadable | `ToolOperation.result` is a JSON value; `state` indicates completion. `tool_result_null_is_distinguished_by_completion_state_on_round_trip` covers all journal states. `completed_tool_results_including_null_survive_reopen_and_further_writes` covers null, scalars, arrays, and objects, then checkpoints and stops the attempt after reopen. The wire representation is unchanged. |
| Replacement did not fence delayed checkpoints | `checkpoint_execution` and journal writes validate trusted callback attempt ID, generation, current lease, and scan state in the mutation transaction. The initialization checkpoint API rejects agents with any attempt history. `checkpoint_and_journal_writes_are_fenced_by_attempt_generation_and_expiry` verifies unchanged durable state on expired, wrong-generation, replaced, mismatched, and identity-free writes, then accepts a current-attempt checkpoint. |
| Ended scans retained mutation/renewal authority | Scan end atomically revokes all worker credentials and marks outstanding attempts/instances termination unconfirmed, preserving environment protections. New inference, canonical mutations, execution checkpoint/journal writes, renewal, and spawn admission are denied. `scan_end_revokes_research_without_claiming_process_termination` covers leader and child credentials, repeated end, reopen, old-token rejection, and replacement waiting for trusted stop confirmation. Recovery is through explicit host operations; old worker bearers are not recovery credentials. |
| Instance ownership was only unique within a scan | Schema 2 uses a shared primary key on host and instance ID. Claim and scan-document insertion share a transaction, including exact registration replay. `independent_scans_cannot_claim_the_same_host_instance` races separate SQLite connections, checks one winner/no loser document, reopens the store, and distinguishes the same instance ID on different hosts. `instance_ownership_upgrade_preserves_existing_claims` and `conflicting_instance_ownership_aborts_upgrade_without_deleting_state` verify upgrade safety. Claims survive stopped execution while private instances remain retained; physical release remains Fleet integration work. |
| Late environment registration lost the end clock | Registration derives its deadline from persisted scan end and retention policy in the same transaction; an explicitly conflicting deadline is rejected. `environment_registration_and_scan_end_share_the_authoritative_deadline` covers both race outcomes, deterministic late registration, rejected premature expiry, reopen, and cleanup immediately before/at the deadline. |
| Pacing replay failed after accounting handover | After checking participant registration, identical saved reservations replay before the generation check for new admission. `pacing_replay_survives_handover_without_admitting_stale_work` verifies replay across reopen/handover, changed-content rejection, stale new-request rejection, valid current-generation admission, removed-participant rejection, and unchanged reservation counts. |

#### Follow-up implementation-review fixes

The three remaining findings were fixed and validated on 2026-11-25. No Stage 2 work was started; the host protocol, JSON contract, and schema versions remain unchanged by these follow-up fixes.

| Finding | Fix and permanent regression evidence |
| --- | --- |
| Late live-instance registration bypassed cleanup protection | Instance validation now checks the owning attempt's state and scan lifecycle. Starting/Running requires an attempt in Running state and an open scan; any non-stopped instance conflicts with a stopped attempt or a termination-confirmed environment. `late_instance_registration_cannot_bypass_termination_and_cleanup_guards` verifies rejection, unchanged durable state, rollback of ownership claims, accepted stopped inventory, and cleanup safety. `racing_instance_registration_and_stop_confirmation_never_leave_live_metadata` races independent database connections and verifies either a stopped recorded instance or rejection with no leaked claim. |
| Duplicate stop confirmation invalidated cleanup claims | `fence_attempt` changes only facts that differ and advances an environment generation only when termination becomes confirmed. Redundant true/false callbacks after confirmation do not regress state or rewrite scan/lifecycle revisions. `duplicate_stop_confirmation_preserves_cleanup_claim_and_deleted_generation` confirms that the original cleanup claim still acknowledges deletion and remains unchanged after later duplicate confirmations and reopen. |
| Fenced journals lacked a trusted recovery path | `recover_operation` is a host-only API, absent from `WorkerOperation`. It targets an existing operation whose original attempt is confirmed stopped, uses revision checks, and preserves its agent, request, and attempt. Running operations can become OutcomeUnknown; known completion/cancellation requires bounded, distinct evidence references. OutcomeUnknown can be resolved to a known outcome, while Completed/Canceled cannot be overwritten. An atomic `tool-outcome-recovered` audit event records original attribution and evidence; stable recovery IDs replay the original acknowledgement and reject altered content. This does not infer process termination, restart work, restore credentials, or complete a replacement assignment. |

Journal recovery acceptance is covered by three permanent tests:

- `stopped_journals_can_be_recovered_after_expiry_or_scan_end_without_worker_authority`: expiry and scan-end paths, rejection before stop confirmation, durable uncertainty, completion with a null result and evidence references, audit provenance, replay after reopen, unchanged assignments/agents/attempts, and continued worker mutation/renewal rejection.
- `journal_recovery_is_content_bound_and_cannot_overwrite_known_outcomes`: evidence validation, missing-operation and stale-revision rejection, cancellation recovery for an old attempt while a replacement exists, unchanged replacement authority, exact replay, conflicting-content rejection, and terminal-outcome immutability.
- `failed_journal_recovery_rolls_back_outcome_event_and_receipt_together`: injected receipt-write failure rolls back the already-written journal and event, leaves no acknowledgement, and allows one successful retry with the same identity.

These are durable host/store APIs and simulated-boundary tests. Worker supervision, transport reconciliation, evidence-byte verification, and physical deletion remain their previously assigned later-stage work.

Validation from `beale-v2/` after both review-fix rounds:

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace` | Passed, including the existing default feature configuration and Desktop |
| `cargo check -p beale-core --no-default-features` | Passed independently |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-fleet -p beale-core --all-targets -- -D warnings` | Passed |
| `cargo test -p beale-contracts -p beale-storage -p beale-fleet` | Passed: 78 tests including all 30 Stage 1 acceptance tests |
| `cargo test -p beale-core` | Passed: 22 tests; the existing Windows tray tests required an unsandboxed rerun after the sandbox denied tray-icon creation |

Changed source, tests, and documentation were checked for live research material and machine-local data. The v2 README and root changelog describe the implemented foundation and its integration limits. Live peers, providers, Docker, and cross-host execution were not required or tested for Stage 1.

## Stage 2: Declarative Swarms and bundled workflows

Rename the feature, crate, configuration, tools, and UI terminology. Preserve useful specialist behavior while replacing peer-to-peer orchestration.

Each enabled definition describes one leader, permitted deputy and worker profiles, delegation relationships, handoff routes, and payload schemas. Definitions describe possibilities; they do not launch every profile immediately.

### Bundled structures

| Swarm | Deputy | Worker profiles |
| --- | --- | --- |
| Basic Subagents | Research Deputy; additional instances for distinct research areas | Mapper, Investigator, Experimenter, Challenger, Builder |
| VDH VVS | Discovery Harness | Mapper, Investigator, Challenger |
| VDH VVS | Validation System | Builder, Experimenter, Challenger |
| OAI 948 | Audit Targets | Mapper, Investigator |
| OAI 948 | Discovery | Mapper, Investigator, Challenger |
| OAI 948 | Dynamic Validation | Builder, Experimenter, Challenger |
| OAI 948 | Reporter | Investigator, Challenger |
| OAI 948 | Patch Generation | Builder, Experimenter, Challenger |

### Workflows and handoffs

- **Basic:** start with one Research Deputy. Deputies own their assigned research areas end to end. The leader may create additional deputies for distinct areas when resources and pacing permit. Worker requests return to the deputy; cross-deputy requests go through the leader.
- **VDH VVS:** Discovery submits candidates and evidence for Validation. Validation returns specific gaps or completed assessments. Validation receives evidence and a labeled claim, without automatically inheriting Discovery's conversation. Failed reproduction remains an observation, not automatic refutation.
- **OAI 948:** Audit Targets supplies ranked targets to Discovery; Discovery supplies candidates to Dynamic Validation; Validation supplies supported findings to Reporter; Reporter supplies remediation context to Patch Generation. Preserve return routes for missing information. Add independent patch checking through Dynamic Validation, returning results to Patch Generation and Reporter.
- **No Subagents:** ordinary single-agent research, outside the hierarchy.

OAI 948 operates on individual work items concurrently. It does not make the entire scan wait for each stage to finish. Deputies and their workers inherit the same stage responsibilities; delegation must not bypass stage boundaries.

All workflow destinations are reached through parents, not direct sibling messaging. Workers submit to their deputy; deputies submit to the leader. Parents choose whether to resume a relevant child, create a child, defer the request, or return it for clarification.

Handoff payloads contain the task, scope, intended destination profile, completion criteria, record revisions, evidence/artifact references, and unresolved questions. Specialized routes add requirements such as ordered target rankings or patch/test references. Routing preserves original observations while adding assignment context.

Persist pending, assigned, completed, blocked, and canceled handoffs. Retries preserve handoff identity. Results distinguish completion from supported, contradicted, or inconclusive assessments and their evidence.

Remove the retired Sequential Research Pipeline and legacy-only profiles.

### Acceptance tests

- Every bundled definition validates; malformed delegation and handoff routes fail. Workerless definitions fail both direct registration and file-catalog loading before users can select them.
- Basic permits multiple Research Deputies with distinct assignments.
- Every VDH VVS and OAI 948 forward/return route is exercised.
- Independent patch checking reaches Validation through the leader.
- Templates are snapshotted so later edits do not alter an existing scan. Cold restoration preserves bundled and custom snapshots with different installed profile revisions, instructions, leader rules, and topology. Multiple saved revisions of the same definition ID coexist without changing the new-scan catalog; Desktop validates and renders the matching saved snapshot.
- Snapshot-cache pressure cannot reject valid saved revisions. Eviction bounds cached entries, releases unused views, preserves active views, and permits evicted revisions to reopen. Cache hits refresh recency; invalid snapshots do not evict or reorder valid entries.
- New scans with Swarms disabled or No Subagents need no swarm runtime state. Existing saved swarm scans retain their definition and execution semantics when the new-scan feature setting is disabled.
- Task completion never automatically promotes a finding's research status.
- The rendered Swarm selector uses Swarms terminology, including its menu title/accessibility label.

### Stage 2 progress and validation

Completed and validated on 2026-11-25. At that checkpoint, Stage 3 had not started.

Implemented boundaries:

- Renamed the optional Cargo feature/crate to `swarms` / `beale-swarms`, the client DTOs and configuration fields to Swarms, and the authenticated catalog route to `/v1/swarms/templates`. Protocol 2.38 reserves the retired sequential-timer tags. Desktop uses Swarms terminology and shows Basic's Leader and Research Deputy model controls.
- Replaced the Graph peer scheduler and ephemeral sibling-messaging executor with declarative Basic Subagents, VDH VVS, and OAI 948 definitions. Removed sequential-stage timing, retired IDs, and Scout/Reviewer/Prover profiles. No Graph aliases or conversion readers were added. Incompatible development sessions are rejected without changing their stored content.
- `beale-contracts::swarm_workflows` defines validated role/delegation topology, directed route schemas, bounded handoff envelopes, ordered target rankings, labeled claims, evidence-backed assessments, remediation references, and patch/test references. The five worker specialist instructions remain in the bundled profile catalog. Initial deputy intent does not launch processes, and OAI 948 has no global stage scheduler.
- `beale-swarms::create_scan` snapshots the definition, profile revisions/instructions, and coordinator-only leader rule. No Subagents returns without initializing Swarms tables. Feature settings gate creation; saved snapshots load independently of that setting. Unsupported saved configuration fails explicitly.
- `SwarmStore` enforces the saved delegation edges at acceptance and worker authorization. Handoffs persist pending, assigned, completed, blocked, and canceled states with revision checks and content-bound receipts. Worker submissions traverse deputy then leader; deputy submissions traverse leader. Destination profiles do not grant sibling communication. Parent assignment context is separate from immutable original observations. Completion stores a separate research assessment and does not promote a finding.
- VDH VVS Validation receives a labeled candidate and evidence; handoff schemas reject inherited conversation fields. OAI 948 includes forward and missing-information return routes plus Patch Generation → Validation → Patch Generation/Reporter for independent patch checking.

Stage boundary: this stage deliberately removes the retired executable peer behavior without implementing the Stage 3 hierarchy engine. Definitions, snapshots, and durable handoff APIs are available, but the feature exposes zero model-facing orchestration tools. Requests to execute a new or saved Swarm return an explicit unsupported response; they never run as a local scan. No Subagents continues ordinary local research. Stage 3 will install the local hierarchy, tools, resumption, and leader review; later Fleet stages still own transport, Docker, pacing, and physical resource management.

Acceptance evidence:

| Acceptance boundary | Executable coverage |
| --- | --- |
| Exact bundled hierarchy/delegation, malformed definitions, removed profiles and retired IDs | `bundled_structures_have_one_leader_and_exact_stage_delegation`; `malformed_definitions_and_authority_shortcuts_are_rejected` |
| Multiple Basic deputies with distinct tasks, queued acceptance without execution/admission | `basic_accepts_multiple_deputies_without_allocating_instances` |
| Every VDH VVS/OAI 948 forward and return route, independent patch review, worker/deputy parent paths | `all_forward_return_and_independent_patch_routes_are_durable_and_parent_mediated` |
| Stage-specific worker permissions and handoff payload enforcement | `stage_workers_cannot_bypass_their_deputys_delegation_or_handoff_contract`; `specialized_payloads_reject_missing_evidence_rankings_and_patch_references` |
| All handoff states, unchanged observations, retry identity, stale/terminal transition rejection, transactional failure | `handoff_transitions_preserve_observations_and_separate_completion_from_assessment`; `canceled_handoffs_are_terminal_and_invalid_outcomes_roll_back` |
| Saved definition/profile/rule snapshots, template edits, feature toggle and unsupported resume | `templates_and_profile_instructions_are_snapshotted_and_toggle_only_gates_creation`; session `catalog_edits_get_new_ids_without_changing_saved_snapshots` |
| No Subagents/disabled creation needs no Swarms tables; Fleet destinations rejected | `local_selection_needs_no_swarm_tables_and_rejects_fleet_destinations` |
| No local fallback or rewriting of unsupported saved state; authenticated new route with no old alias | session `unsupported_hierarchy_never_falls_back_to_local_or_rewrites_saved_state`; `swarm_template_route_requires_auth_and_has_no_graph_alias` |
| Client names, profile/model forms, local usage classification, draft-versus-active hierarchy, and saved snapshot validation | Updated Desktop unit suites in `main.rs`, `new_research.rs`, `research_header.rs`, `sessions.rs`, and `tracking_data.rs` |

The Stage 2 acceptance additions comprise ten `beale-swarms/tests/workflows.rs` tests and three session boundary/catalog tests. All thirty Stage 1 acceptance tests are retained; their synthetic snapshot/handoff fixture now uses the interpreted workflow contract. Runtime-specific Graph tests were retired with the removed runtime, rather than treated as coverage of the future hierarchy.

Validation from `beale-v2/`:

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --quiet` | Passed with the host default feature set and Desktop |
| `cargo check -p beale-core --no-default-features --quiet` | Passed independently |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-swarms -p beale-sessions -p beale-core -p beale-desktop-app --all-targets -- -D warnings` | Passed |
| `cargo test -p beale-swarms -p beale-contracts -p beale-storage -p beale-sessions -p beale-desktop-app --quiet` | Passed: 10 workflow, 18 contract, 42 storage, 47 default-session, and 267 Desktop tests |
| `cargo test -p beale-sessions --all-features --quiet` | Passed: 54 tests, including the feature-gated template/catalog boundary tests; the session crate's default feature set is empty |
| `cargo test -p beale-sessions --no-default-features unsupported_hierarchy_never_falls_back_to_local_or_rewrites_saved_state --quiet` | Passed independently |
| `cargo test -p beale-core --quiet` | Passed: 22 tests; run outside the sandbox for the existing Windows tray tests |
| `git diff --check` | Passed |

All thirteen new Stage 2 acceptance tests and all thirty retained Stage 1 acceptance tests pass. Desktop regressions caught during the rename were corrected: local `main` usage remains local activity, draft selections do not act like running hierarchies, and leader/deputy controls and feature keys use the new definitions. The all-feature session gate also updates its aggregate count to the 28 currently exposed tools; Stage 2 Swarms exposes none.

Changed source, fixtures, and documentation were inspected for live research material and machine-local identifiers; new fixtures are synthetic. README and changelog entries describe the execution boundary. No live provider, peer, Docker, or Stage 3 hierarchy execution was implemented or tested.

### Stage 2 review fixes

Addressed all three review findings on 2026-11-25. Stage 3 remains unstarted.

- **Saved snapshots:** split current-template construction from snapshot restoration. Restored bundled and custom views use saved topology, profile revisions/instructions, and leader rules. A separate cache keyed by the complete snapshot lets distinct saved revisions share a definition ID without replacing catalog entries or becoming selectable for new scans. Desktop requires matching snapshot data for all saved Swarms and uses it for validation, hierarchy tabs, selected roles, and workflow labels. Unsupported profile identities still fail explicitly; no compatibility fallback was added.
- **Workerless templates:** workflow validation now requires worker profiles alongside the leader and deputies, matching durable scan validation. The file catalog omits such invalid definitions.
- **Selector terminology:** the rendered menu title/accessibility label is `Scan Swarm`; template-loading errors also use Swarms terminology.

Permanent regression coverage:

| Review finding | Coverage |
| --- | --- |
| Cold snapshot restoration, saved instructions/rules/layout, storage reopen, and catalog isolation | New workflow tests `cold_saved_snapshots_preserve_profiles_rules_and_bundled_revisions` and `saved_views_with_the_same_id_coexist_without_changing_the_catalog` |
| Desktop uses saved topology and names, rejects missing/malformed/mismatched snapshot data, and handles separate saved views with the same bundled ID | New Desktop test `saved_swarm_views_validate_and_render_their_own_snapshot`; strengthened existing session-detail validation fixture |
| Workerless definitions are rejected before selection | New workflow test `workerless_definitions_are_rejected_before_catalog_registration`; strengthened file-catalog test `catalog_edits_get_new_ids_without_changing_saved_snapshots` |
| Swarm selector title is rendered with current terminology | Strengthened Desktop test `swarm_selector_options_show_descriptions_as_tooltips` checks the rendered menu label |

Review-fix validation:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --quiet` | Passed |
| `cargo check -p beale-core --no-default-features --quiet` | Passed independently |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-swarms -p beale-sessions -p beale-core -p beale-desktop-app --all-targets -- -D warnings` | Passed |
| `cargo test -p beale-swarms -p beale-contracts -p beale-storage -p beale-sessions -p beale-desktop-app --quiet` | Passed: 13 workflow, 18 contract, 42 storage, 47 default-session, and 268 Desktop tests |
| `cargo test -p beale-sessions --all-features --quiet` | Passed: 54 tests, including file-catalog rejection coverage |
| `cargo test -p beale-sessions --no-default-features unsupported_hierarchy_never_falls_back_to_local_or_rewrites_saved_state --quiet` | Passed independently |
| `cargo test -p beale-core --quiet` | Passed: 22 tests; run outside the sandbox for the existing Windows tray tests |
| `git diff --check` | Passed |

Four new regression tests and the strengthened existing coverage pass, together with all thirty retained Stage 1 acceptance tests. Changed source, fixtures, and documentation were inspected for live research material and machine-local identifiers; new data is synthetic. The README and changelog reflect the restored-view and validation boundaries. This follow-up adds no execution, tools, transport, Docker, or Stage 3 behavior.

### Stage 2 snapshot-cache follow-up

Fixed the remaining cache-exhaustion finding on 2026-11-25. Stage 3 remains unstarted.

Restored views now use owned strings and reference-counted handles instead of leaked static allocations. The cache holds at most 1,024 entries and evicts the least recently used entry when a new valid snapshot arrives. Hits refresh recency. Validation and view construction complete before eviction, so malformed snapshots and unsupported profile identities cannot displace valid entries. Active callers keep their views after eviction; dropping the final handle releases the view. Evicted revisions can be restored again without changing the new-scan catalog or requiring a process restart. Desktop retains owned handles while validating and rendering snapshots. Protocol and durable schema formats are unchanged.

Permanent regression coverage:

| Boundary | Coverage |
| --- | --- |
| More than 1,024 saved revisions remain readable; evicted snapshots reopen exactly; active snapshots and catalog entries remain unchanged | `saved_scans_reopen_after_more_than_1024_snapshot_revisions` in `beale-swarms/tests/cache_pressure.rs` exercises 1,100 distinct revisions and checks reference release |
| Least-recently-used ordering, bounded entry count, active-view survival, unused-view release, and reopening | `eviction_releases_unused_views_and_preserves_active_views` uses an isolated small cache and weak references to verify ownership |
| Malformed and unsupported snapshots cannot evict or reorder valid cached views | `invalid_snapshots_do_not_evict_or_reorder_cached_views` |

Cache-fix validation:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets --quiet` | Passed, including all test targets |
| `cargo check -p beale-core --no-default-features --quiet` | Passed independently |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-swarms -p beale-sessions -p beale-core -p beale-desktop-app --all-targets -- -D warnings` | Passed |
| `cargo test -p beale-swarms -p beale-contracts -p beale-storage -p beale-sessions -p beale-desktop-app --quiet` | Passed: 16 Swarms tests (13 workflow, 2 cache unit, 1 public cache-pressure), 18 contract, 42 storage, 47 default-session, and 268 Desktop tests |
| `cargo test -p beale-sessions --all-features --quiet` | Passed: 54 tests |
| `cargo test -p beale-sessions --no-default-features unsupported_hierarchy_never_falls_back_to_local_or_rewrites_saved_state --quiet` | Passed independently |
| `cargo test -p beale-core --quiet` | Passed: 22 tests; run outside the sandbox for the existing Windows tray tests |
| `git diff --check` | Passed |

All three new cache regressions pass, along with the existing Stage 2 suites and all thirty retained Stage 1 acceptance tests. Changed source, tests, and documentation were inspected for live research material and machine-local identifiers; new fixtures are synthetic. README and changelog entries describe the cache lifecycle. No Stage 3 execution, tools, or orchestration was implemented.

## Stage 3: Local hierarchy, tools, and leader review

Prove orchestration locally before adding remote execution.

### Tool contract

| Tool | Available to | Boundary |
| --- | --- | --- |
| `swarm_list_subagents` | Leaders and deputies | Descendant tree and statuses |
| `swarm_spawn_subagent` | Leaders and deputies | Leaders create deputies; deputies create workers; acceptance may return queued before execution admission |
| `swarm_stop_subagent` | Leaders and deputies | Direct children only |
| `swarm_message_subagent` | Leaders and deputies | Direct children only; can resume relevant completed agents |
| `swarm_message_parent` | Deputies and workers | Parent only; optionally contains a structured handoff |

Workers cannot spawn children or directly message siblings. The leader can see workers under deputies but cannot directly message or stop them.

Parents decide whether to reuse an agent, spawn one, defer work, or return a handoff for clarification. An intentional deputy stop stops its branch. A temporary deputy disconnect enters recovery instead.

### Waiting, suspension, and wake-up

A deputy waiting for children records the outstanding assignments and inbox cursor at a safe checkpoint. Once it has no active inference or managed tool operation, the harness may suspend its execution without completing or canceling those assignments. Its mailbox remains on the leader's app-server. Child results, direct-parent messages, and explicit resume requests create durable, deduplicated wake requests.

The first child need not already exist or be running. If a placement can become feasible by safely yielding the requesting deputy, accept the spawn as a durable queued assignment and return its identity and wait reason without waiting for a running instance. Persist that tool result and a suspension intent, then yield at the next safe checkpoint. Stage 7 performs the capacity check again after release before starting the child. If no placement is feasible even after the identified release, deny the request without creating an unbounded backlog. Stage 3 tests this handshake with a simulated admission adapter, not real Fleet allocation.

The scheduler reacquires eligible capacity before resuming the same agent and delivers messages after the saved cursor. If capacity is unavailable, the agent remains visibly waiting; the harness does not poll by running repeated model turns. Completion/cancellation of a child must not require the parent to be running. Do not suspend by abandoning an in-flight command; finish it or follow the explicit cancellation/recovery path first. Stage 3 tests these transitions with a local execution adapter; Stage 7 tests actual resource release and allocation.

### Leader review and execution

Add the leader-only custom auto-review rule. Leaders retain managerial tools, but calls for dedicated research are rejected. Include role and assignment context in review, and preserve that rule through any Guardian reconsideration. Deputies, workers, and ordinary single-agent sessions do not receive it. Review-service failures follow bounded recovery rather than silently bypassing the rule.

Treat the leader rule as a versioned, swarm-scoped custom rule automatically attached to a scan created with an enabled swarm, not a global user rule copied into every session. The active scan retains its rule snapshot. Display it as an active scoped rule in Safety/session details; global rule toggles do not disable or delete it, and the active snapshot is read-only. Changes to its instruction are made through a new swarm-definition revision for future scans. Bundled definitions retain the coordinator-only instruction. Creating a new scan with No Subagents or with Swarms disabled omits this scoped rule. Disabling Swarms afterward does not remove the rule from an existing running, paused, or resumed swarm.

The leader derives trusted role and rule applicability before review. Review-required worker operations use the same leader-owned policy through routing, not a guest's local settings. Guardian reconsideration receives the applicable leader rule and trusted role and cannot discard them. With all unrelated global rules disabled, the leader rule still applies; ordinary non-swarm behavior continues to follow the existing global settings. These are model-review rules, not new host permission prompts or OS isolation.

Replace user-facing turn/token-count budgeting with the agreed pacing model; retain only clearly justified operational safeguards against malformed or non-progressing execution. Stage 3 introduces the admission interface and removes obsolete user-facing contract requirements; Stage 8 supplies pacing enforcement, so Stage 3 acceptance does not depend on Stage 8.

Reuse context for related follow-up work, but start a fresh independent reviewer when prior participation would undermine the intended review.

### Acceptance tests

- Tool availability and direct-child restrictions are enforced at dispatch, not just through prompts.
- Leader managerial operations pass representative review cases; dedicated research cases fail.
- Deputy research remains available.
- Resumed leaders retain the review rule.
- Completed children resume with their original identity and relevant context.
- Fresh independent reviewers do not inherit contaminated participant context.
- Branch cancellation releases execution capacity and preserves partial results.
- A waiting deputy can suspend with pending children; results and direct-parent messages wake it once and replay only unread inbox entries.
- A child can complete while its parent is suspended, and the parent's next turn observes that result after capacity returns.
- Unrelated global rules disabled, Guardian reconsideration, and changed prompt text cannot remove or spoof the leader-rule scope.
- No Subagents and deputies/workers never receive the scoped leader rule; global settings retain their existing effect outside that scope.
- Updating a swarm definition changes future rule snapshots, not active/resumed scans. Remote policy enforcement is additionally tested in Stage 5.
- Starting with a deputy and no children, a queued spawn result reaches a durable checkpoint before the deputy suspends; no model turn blocks waiting for the child's execution admission.
- If checkpoint/suspension fails, the queued assignment remains non-running and enters bounded recovery or an explicit blocked result; no child is launched against capacity that was never released.
- Disable Swarms while children are active or the deputy is suspended: existing assignments and review snapshots remain intact, results still wake their parents, and later resume retains the same workflow.

- Inject failures into both transcript persistence and the supervisor checkpoint after an accepted spawn: retry the same write without fresh inference or duplicate assignments. Exhaustion blocks only the affected subordinate, or stops a broken leader, and never releases an uncommitted child dependency.
- Transient checkpoint failures roll back atomically, then save the same completed turn once; ordinary sessions retain their single-attempt persistence behavior.
- Empty completions and provider errors keep the public stream open through recovery. Success and exhausted recovery each produce exactly one final supervisor outcome, matching durable session state.
- An accepted handoff reaches the parent's next model prompt with its complete typed envelope, including routing, record revisions, observations, and evidence references. Receipt replay does not duplicate delivery; large batches preserve excess messages unread.
- A follow-up queued before the first child execution supplements the original question, completion criteria, and evidence references, including retries of an uncheckpointed first turn.
- Long multibyte completion and stop summaries fit the mailbox byte limit with the host prefix included. Both checkpoint successfully and deliver once without splitting UTF-8 characters.

- After an accepted tool operation, provider errors and empty replies retry the same exchange without repeating the assignment. Repeated provider call identities replay their saved result; changed content is rejected, while a distinct identity can intentionally perform new work. Exhaustion saves partial activity and blocks only the failed branch (or stops a broken leader).
- At the ordinary mailbox limit, completion, queued/active cancellation, and exhausted-recovery notifications still commit. Reopening and draining the mailbox delivers each retained notification once; multiple agents' recovery events retain scan-wide ordering.
- Every bundled deputy's workers receive its saved stage instructions and immediate parent identity on execution and resume. Disabling new Swarms does not remove those instructions; workers do not inherit the coordinator-only leader role.

### Stage 3 implementation progress

Completed and validated on 2026-11-25. Stage 4 has not started.

- Added strict typed contracts for the five hierarchy tools. The host resolves role, profile, parent, and execution identity from durable state; prompt text cannot grant authority. Dispatch enforces the saved delegation edges and direct-child controls.
- Added the `swarm-local` schema for model settings, execution controls, accepted tool results, central mailboxes, and inbox sequence cursors. Queued admission persists its result and release dependency before returning. Only a successful safe checkpoint releases that dependency; claim rechecks available capacity. Repeated operation identities are content-bound and replay without duplicate messages.
- Connected the local supervisor to the existing session/provider/tool engine. Child transcripts have durable identities, related follow-ups reuse them, and independent review starts with a new identity and empty conversation. Multiple saved model choices for a profile are selected by stable agent order. Child session IDs cannot be submitted as ordinary top-level sessions.
- Added suspension with outstanding assignments, unread-only message delivery, parent wake-up on results/cancellation, branch stop, and bounded recovery. Managed tools finish before execution acknowledgement; a stop prevents subsequent calls. Three failed attempts block a subordinate and report to its parent; leader exhaustion stops the tree. Failure before the first saved provider turn still produces a visible, durable stopped session.
- Applied the immutable leader-only coordinator rule to leader tool review, including normally exempt managerial tools and Guardian reconsideration. Host role and assignment context accompany review. Global rules retain their existing behavior for deputies, workers, and ordinary sessions. Review failures fail closed and enter bounded local recovery.
- Protocol 2.39 exposes hierarchy identities/status and active scoped rules in session details. Safety renders the selected session's rule read-only. Existing scans retain their model/workflow/rule snapshots when new Swarms are disabled.
- Updated README and changelog entries. Existing Stage 1 and Stage 2 regressions remain in the gates.

Implementation boundary: the Stage 3 local adapter executes model turns serially on the trusted host and tests resource pressure through a simulated admission adapter. It uses the existing workspace; private writable copies and artifact transfer are Stage 4 work. Fleet destinations are explicitly rejected by this local adapter. Physical concurrent placement, retained container filesystems, resource pacing, and token pacing remain in their assigned later stages. Separate Goal Mode/repeating-automation loops are not combined with this local hierarchy scheduler. No live-provider or remote-worker behavior is claimed by these tests.

### Stage 3 review fixes

Addressed all five review findings on 2026-11-25. Stage 4 remains unstarted.

1. **Checkpoint replay:** the engine and supervisor retry their exact atomic persistence operation up to three times. Exhaustion has a distinct checkpoint-failure path that blocks a subordinate or stops the leader without another model turn. Accepted tool results remain durable, and queued children retain uncommitted release dependencies. Post-commit projection failures also cannot trigger automatic inference replay.
2. **Premature stream termination:** local engine attempts leave terminal events to the session route after supervisor recovery completes. Ordinary session error streaming is unchanged.
3. **Invisible handoff payloads:** durable inbox messages now carry the complete typed handoff. Parents receive it in their next prompt; content-bound receipts, source checks, and cursor acknowledgement still apply. Serialized Swarm commands are limited to 64 KiB, and bounded prompt batching defers excess unread mail.
4. **Lost initial assignment:** a child's first uncheckpointed execution includes the complete assignment together with queued follow-ups. Successful resumed turns continue from saved context and unread messages.
5. **Multibyte checkpoint failure:** completion and stop summaries reserve bytes for their prefix and truncate results only at UTF-8 boundaries within the 16 KiB mailbox limit.

Added eight permanent regression tests and expanded the existing handoff regression to verify complete envelope delivery and acknowledgement. Engine fault injection covers both transcript-save and supervisor-checkpoint exhaustion; HTTP tests cover empty responses, provider errors, successful recovery, and exhausted recovery. Storage tests cover blocked-branch isolation, full handoff batching, initial task preservation, oversized command rejection, and multibyte completion/stop persistence. README and changelog descriptions match these tested boundaries.

### Stage 3 follow-up review fixes

Addressed the three remaining findings on 2026-11-25. Stage 4 remains unstarted.

1. **Provider recovery after accepted work:** local provider steps with tool activity retry the same exchange up to three times, retaining tool results instead of starting another turn with the original assignment. Within that execution, provider call identities are bound to the tool name and canonical JSON arguments; exact replays reuse the original result without another tool dispatch or transcript operation. Changed content is rejected, and distinct identities remain valid new operations. Exhausted provider/review recovery preserves the partial transcript and accepted results, then blocks the subordinate or stops a broken leader without automatic replay. This does not add process-restart or remote-worker recovery.
2. **Mailbox pressure during lifecycle changes:** the 1,024-entry admission limit continues to reject ordinary messages. Host-generated completion, cancellation, and exhausted-recovery notices for accepted work can exceed that limit, so notification pressure cannot roll back execution release. They retain the same durable ordering and acknowledgement cursor and are drained without dropping or duplicating ordinary mail. The pressure regression also exposed recovery-event numbering per agent; numbering now follows the scan-wide event sequence so multiple failed branches can commit.
3. **Inherited worker stages:** the engine resolves the immediate parent from durable identity and includes the deputy's saved profile instructions and revision in each worker's system prompt, alongside the worker's specialist instructions. It supplies parent identity for structured handoffs without adding the leader's coordinator-only rule to workers. Resume uses the saved snapshot even after Swarms is disabled.

Four new permanent tests cover transient and exhausted provider recovery, empty replies, replayed/changed/distinct tool identities, review-service failure after accepted work, isolation of a failed worker from successful siblings, full-mailbox lifecycle transitions and delivery after reopen, and execution/resume under all eight bundled deputy stages. No Stage 4 workspace copies or artifact-transfer implementation was added.

### Stage 3 acceptance coverage

| Acceptance boundary | Permanent coverage |
| --- | --- |
| Role-specific tool availability; direct-child controls; leader sees descendants; no worker spawn or sibling control; no obsolete tool budgets | `local::tests::tools_follow_host_roles_without_legacy_budgets`; `direct_child_authority_is_enforced_in_transactions` |
| Managerial leader approval; dedicated-research denial; global rules off; Guardian retains rule; prompt role spoofing; deputy/worker research; ordinary-session bypass; saved rule revisions remain immutable/read-only | `swarm_tests::saved_scope_survives_disabled_globals_guardian_and_prompt_spoofing` using a synthetic loopback review provider |
| Related completed child retains identity and checkpoint context; unread cursor advances; result delivered after capacity returns; repeated wake is deduplicated | `queued_result_is_durable_before_release_and_wakes_replay_only_unread` |
| Fresh independent reviewer receives a new identity and empty context | `independent_review_has_new_identity_and_no_participant_context` |
| Branch cancellation preserves partial results and unrelated work; execution remains occupied until acknowledgement | `cancellation_keeps_capacity_until_ack_preserves_partial_and_unrelated_branches`; `local_stop_finishes_managed_tool_before_releasing_execution` |
| First-child result persists before yield; child cannot start before checkpoint or while capacity is absent; impossible admission leaves no backlog | `queued_result_is_durable_before_release_and_wakes_replay_only_unread`; `local_admission_requires_feasibility_after_the_identified_release`; `infeasible_spawn_leaves_no_backlog_and_leader_failure_stops_the_tree` |
| Failed checkpoint keeps child non-running; retries are bounded; subordinate failure does not stop the leader; leader failure stops the tree | `checkpoint_failure_blocks_child_until_success_and_recovery_is_bounded`; `infeasible_spawn_leaves_no_backlog_and_leader_failure_stops_the_tree` |
| Parent messages wake a suspended deputy once; another parent turn cannot invalidate an already committed release; feature disable preserves pending work | `parent_messages_wake_suspended_deputy_once_without_invalidating_child_release` |
| Parent-only structured handoff, durable replay, stale execution and spoofed-source rejection | `parent_handoffs_are_durable_content_bound_and_reject_spoofed_sources` |
| End-to-end leader/deputy/worker execution and wake-up, disable during child work, saved leader resume, child-session bypass rejection, disabled new scan rejection, graceful failure before first turn | `hierarchy_tests::local_tree_executes_and_resumes_after_feature_disable` using a synthetic loopback provider |
| Transcript/supervisor checkpoint faults cannot repeat accepted spawns; transient writes retry atomically; exhausted subordinate remains isolated and child unreleased | `hierarchy_tests::checkpoint_exhaustion_does_not_repeat_accepted_spawn`; `hierarchy_tests::transient_checkpoint_retries_same_atomic_write`; `exhausted_checkpoint_blocks_branch_without_replaying_or_releasing_child` |
| Public HTTP stream survives empty responses/provider errors and completes once after successful or exhausted recovery | `hierarchy_tests::local_stream_stays_open_through_provider_recovery_and_exhaustion` |
| Full typed handoff reaches parent; large payload batches preserve unread envelopes; oversized commands cannot create work | `parent_handoffs_are_durable_content_bound_and_reject_spoofed_sources`; `full_handoffs_are_batched_without_losing_unread_payloads`; `oversized_commands_are_rejected_before_creating_work` |
| Early follow-ups retain original task, criteria, and evidence through first-turn retries | `queued_followup_preserves_initial_assignment_and_evidence` |
| Multibyte completion/stop summaries checkpoint once and fit mailbox bytes including prefix | `multibyte_completion_and_stop_fit_mailbox_and_commit_once` |
| Provider recovery retains accepted results; replay is content-bound; distinct operations remain valid; exhaustion never repeats accepted work | `hierarchy_tests::provider_recovery_preserves_exchange_and_deduplicates_tool_replies` |
| Failed worker preserves its accepted message and partial transcript; siblings and parents complete | `hierarchy_tests::exhausted_provider_blocks_only_failed_worker_and_keeps_accepted_message` |
| Full ordinary mailbox cannot block completion, cancellation, or exhausted recovery; reopen/drain delivers once; recovery events are ordered across agents | `full_mailbox_cannot_block_lifecycle_or_lose_notifications` |
| Workers inherit saved stage instructions and parent identity under every bundled deputy, including resume after feature disable | `hierarchy_tests::workers_inherit_saved_deputy_stage_on_execution_and_resume` |
| Fleet selection never falls back locally | `fleet_destinations_cannot_silently_execute_locally` |
| Client accepts multiple identities of one profile, suspended status, and enabled scoped rules; rejects duplicate identities and disabled scoped rules | `local_hierarchy_projection_accepts_repeated_profiles_and_scoped_rules` |

### Stage 3 validation results

Commands were run from `beale-v2/`.

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets --quiet` | Passed |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-safety -p beale-swarms -p beale-sessions -p beale-desktop-app --all-targets --all-features -- -D warnings` | Passed |
| `cargo clippy -p beale-sessions --all-targets --no-default-features -- -D warnings` | Passed |
| `cargo check -p beale-core --no-default-features --quiet` | Passed |
| `cargo test -p beale-contracts -p beale-storage -p beale-swarms --quiet` | Passed: contracts 18, storage 57, Swarms 18 tests |
| `cargo test -p beale-sessions --all-features --quiet` | Passed: 62 tests |
| `cargo test -p beale-sessions --no-default-features --quiet` | Passed: 47 tests |
| `cargo test -p beale-safety --quiet` | Passed: 10 tests |
| `cargo test -p beale-desktop-app --quiet` | Passed: 269 tests |
| `cargo test -p beale-core --quiet` | Passed: 22 tests |
| `git diff --check` | Passed |

The mocked-provider suites used temporary loopback servers outside the sandbox because Windows sandbox networking denied the local connection. The host suite likewise ran outside the sandbox for its existing tray tests. No live provider credentials or research targets were used. New fixtures and changed documentation were checked for real research material and machine-local identifiers. The final gates include all thirty retained Stage 1 acceptance tests and the Stage 2 snapshot-cache pressure/eviction regressions.

## Stage 4: Snapshots, artifacts, and environment-storage contracts

Use the same ownership model for local and remote subagents.

### Implementation

- Create one immutable initial research-workspace snapshot per scan, with private writable copies for subagents.
- Record file identity, size, content hash, and required filesystem metadata in a manifest.
- Define a snapshot boundary excluding host credentials, databases, and runtime state. Explicitly handle linked files, external source inputs, repository metadata, permissions, and files changing during capture.
- Implement chunked, resumable transfer with integrity verification and atomic publication.
- Create central artifact references with producer, assignment, revision, and provenance.
- Automatically publish a local file when attached to a finding or handoff. Acknowledge a durable attachment only after central acceptance.
- Materialize artifacts explicitly into another agent's working directory. Do not continuously synchronize everyone's edits.
- Return source changes as separate versions or patches; do not automatically overwrite another agent's files.

Define an environment-storage interface for checkpoint references, retained filesystem components, publication dependencies, and restore compatibility. Stage 4 implements and tests local directory storage and simulated transfer adapters only. Stage 6 implements retained container volumes and writable filesystems and verifies installed-tool persistence. Stage 7 implements cross-host restore/reconstruction and explicit failure when a retained environment cannot be recovered on an eligible host.

### Snapshot preservation and rejection rules

- Preserve regular-file bytes, relative paths, and known executable metadata. Prefer authoritative source metadata such as a repository's executable bit when the source filesystem cannot represent it. Do not infer executable permission solely from a filename; surface missing required metadata.
- Preserve relative symlinks only when they resolve within declared snapshot inputs and the destination can represent them. Reject escaping links, unsupported link types, and undeclared external targets with a path-specific diagnostic. External source roots must be declared inputs and materialized under explicit snapshot-relative locations; do not traverse ambient host paths automatically.
- Preserve distinct case-sensitive paths only on destinations that can represent them. Reject case-folding collisions, unsupported filenames, and permission semantics required by the input that the destination cannot preserve. Never silently rename, flatten links, or overwrite one colliding file.
- Materialize required repository history and worktree metadata into a self-contained representation. Exclude credential-bearing configuration and machine-local worktree/alternate pointers; translate required dependencies into snapshot-owned data or report unsupported metadata. Do not copy a linked worktree's pointer to its original host.
- Verify file content and metadata during capture and validate the manifest before sealing. Retry boundedly on detected mutation, then report that the source must be quiesced. Do not claim an atomic whole-workspace snapshot without filesystem snapshot support or a quiesced source. Use only sealed immutable inputs for workers.

Private copies prevent accidental conflicts through harness-managed paths and routing. They do not prevent arbitrary same-user shell commands from accessing other host paths. Beale does not introduce an OS sandbox for local agents.

Artifact acceptance establishes durable storage and provenance, not vulnerability verification.

Checkpoint and publication writes use temporary state followed by verified durable commit. Storage exhaustion must leave the last acknowledged checkpoint/artifact valid and must not advertise partial bytes as committed. Keep unacknowledged outputs and their existing lifecycle protections until publication can be reconciled. Stage 4 tests filesystem and persistence failures through fault-injection adapters; Stages 5 and 7 cover routed acknowledgements and retention behavior.

### Acceptance tests

- Two agents modifying the same relative file through their assigned workspace paths produce separate results; harness routing never aliases their writable copies.
- Interrupted transfers resume and reject corrupt or changed content.
- Findings and handoffs never expose acknowledged references to missing artifact bytes.
- Local and simulated remote materialization preserve the same declared file semantics; unsupported destinations fail explicitly. Real worker and Docker transport are covered in Stages 5-6.
- Snapshot traversal cannot accidentally import unrelated host files.
- Reopening local environment storage preserves workspace edits and checkpoint references; installed container tooling is verified in Stage 6.
- Snapshot/transfer contracts exclude canonical database files; Stage 5 verifies canonical research operations over routing.
- Windows-to-Linux fixtures preserve executable metadata, supported symlinks, and self-contained repository inputs without host pointers or credentials.
- Case collisions on a case-insensitive destination, unsupported names, escaping links, undeclared external roots, and unrepresentable required permissions are rejected explicitly.
- Mutating a file during capture triggers bounded retry or failure; no unverified manifest becomes a worker input.
- Inject out-of-space failures during checkpoint write/commit and artifact chunk write/seal: the previous committed state remains readable, partial state is not acknowledged, and retry after capacity returns uses the same operation identity.
- Inject failure after bytes are staged but before artifact/record metadata commits: no finding or handoff receives an acknowledged dangling reference, and staged bytes remain recoverable or safely reclaimable after reconciliation.

### Stage 4 progress — completed 2026-11-25

Implemented in `beale-contracts`, `beale-storage`, `beale-tools`, `beale-sessions`, `beale-swarms`, and the canonical Findings store. Stage 5 progress is tracked separately below.

- Added version-1 snapshot, artifact, materialization, and environment JSON contracts and the independently migrated `swarm-files` schema. Host paths remain outside these contracts.
- New scans seal one initial workspace snapshot before execution. Deputies and workers receive private writable copies and reuse retained directories on related resume; leaders retain the original workspace. Native file tools admit only the assigned environment within private host storage, while sibling directories and canonical databases remain unavailable through those tools.
- Capture uses two content/metadata passes and at most three attempts. It hashes and copies file content through bounded buffers, excludes host state and credential paths, handles explicit external mounts, and rejects unsupported paths, links, permissions, or changing inputs. Git bundles preserve self-contained history and executable index metadata, including linked worktrees, without copying source configuration or administrative pointers. Exclusions also apply to repository history.
- Transfers retain offsets and content-bound identities, reject changed retries and corrupt seals, and commit accepted artifact metadata only after verified bytes are durable. Artifact provenance includes workspace, scan, producer, assignment, source path, revision, and optional predecessor.
- `file:relative/path` attachments in finding artifact evidence and handoff artifact fields publish automatically before canonical mutation. Both the tool adapter and canonical finding/handoff transactions validate accepted central references. A failed record commit can leave recoverable unreferenced bytes, but cannot acknowledge a dangling record reference.
- Added `swarm_materialize_artifact` with typed `{artifact, destination}` arguments for every role. It creates an explicit separate file and rejects existing destinations. There is no continuous synchronization or automatic source overwrite.
- Local environment descriptors retain workspace components, checkpoint references, publication dependencies, and an OS-specific restore compatibility identifier. Checkpoints flush retained files and commit their references before supervisor completion. Failures preserve the last acknowledged checkpoint and pending outputs; missing or incompatible retained directories fail explicitly.

Implementation boundaries: automatic local capture uses the workspace root; additional inputs are available through the host storage API and must be mounted explicitly. Native Windows materialization rejects symlinks and standalone required executable bits it cannot represent; repository indexes preserve known executable metadata. Linux metadata is exercised through simulated destinations on this Windows host; the Unix native symlink regression is platform-gated. Staged/unmerged indexes, submodules, unsupported permission semantics, excluded history paths, and Git platform path-length failures are explicit rejections. Git must be installed for repository inputs. Earlier development scans without an initial filesystem snapshot are not silently recaptured. Physical cleanup, Docker volumes, distributed worker recovery, cross-host restoration, and retention enforcement remain in their assigned later stages.

### Stage 4 acceptance evidence

The permanent storage suite is `beale-storage/tests/swarm_files.rs`; runtime attachment/routing tests are in `beale-sessions/src/research_files.rs`. The native-tool boundary regression lives in `beale-tools/src/lib.rs`. The initial implementation added 19 executed Stage 4 tests on Windows, with an additional native Unix symlink test gated to Unix.

| Acceptance case | Permanent evidence |
| --- | --- |
| 1. Separate writable copies and routing | `immutable_snapshot_private_copies_and_retained_edits`; `native_tools_route_to_private_copies_and_materialize_only_explicitly`; native-tool assigned-directory/host/sibling guard regression |
| 2. Resumable integrity-checked transfer | `interrupted_transfers_resume_and_reject_changed_or_corrupt_bytes`; chunk write/commit fault cases |
| 3. No acknowledged missing artifact bytes | `attachment_resolution_is_all_or_nothing_and_checks_central_bytes`; actual Findings and handoff commit tests, including direct canonical-store and handoff-transition bypass attempts |
| 4. Local and simulated destination semantics | `declared_external_roots_and_simulated_transfer_preserve_bytes_and_metadata`; simulated destination rejection and Windows-to-Linux Git metadata tests |
| 5. No ambient host-file traversal | Canonical path rejection, excluded host-root capture, explicit external mounts, escaping/undeclared link rejection; native Unix traversal fixture |
| 6. Reopen preserves edits and checkpoint references | Fresh database/store reopen in `immutable_snapshot_private_copies_and_retained_edits`; missing-directory and environment-commit failure tests |
| 7. No canonical databases in snapshots/transfers | `snapshots_exclude_canonical_databases_credentials_and_runtime_state`; excluded-path transfer checks; excluded Git history regression |
| 8. Windows-to-Linux executable/link/repository metadata | `windows_git_link_and_executable_metadata_survive_simulated_linux_transfer`; self-contained Git repository and linked-worktree regressions |
| 9. Explicit incompatibility failures | `simulated_destinations_reject_collisions_names_links_and_required_permissions`; staged metadata and excluded history tests |
| 10. Bounded mutation handling | `changing_sources_retry_boundedly_and_only_seal_verified_inputs`, covering both retry success and three-attempt exhaustion |
| 11. Storage exhaustion and stable retry identity | `publication_faults_keep_pending_bytes_and_retry_identity`; `checkpoints_keep_previous_ack_on_write_and_commit_exhaustion`; environment commit failure |
| 12. Bytes staged before metadata/record failure | Artifact-commit fault injection, generic record rollback, and actual finding/handoff transaction triggers; retries recover the same accepted artifact identity |

### Stage 4 validation results

| Check | Result |
| --- | --- |
| `cargo test -p beale-contracts -p beale-storage -p beale-swarms -p beale-tools -p beale-findings --quiet` | Passed: contracts 18, storage 73, Swarms 18, tools 16, Findings 13 tests |
| `cargo test -p beale-sessions --all-features --quiet` | Passed: 64 tests; the final canonical-transition additions also passed the focused `research_files` rerun |
| `cargo test -p beale-sessions --no-default-features --features swarms --quiet` | Passed: 58 tests |
| `cargo test -p beale-sessions --no-default-features --quiet` | Passed: 47 tests |
| `cargo check --workspace --all-targets --all-features --quiet` | Passed |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-tools -p beale-sessions -p beale-swarms -p beale-findings --all-features --all-targets -- -D warnings` | Passed |
| `cargo clippy -p beale-sessions --no-default-features --features swarms --all-targets -- -D warnings` | Passed |
| `cargo clippy -p beale-sessions --no-default-features --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `git diff --check` | Passed |

Native filesystem tests ran outside the Windows sandbox because it rejected atomic hard-link publication in the workspace. Synthetic loopback-provider tests likewise used the existing approved test execution path. No live providers, containers, or research targets were used. Fixtures and changed documentation were inspected for real research material and machine-local identifiers. This gate covered Stage 4; Stage 5 has its own acceptance matrix below.

### Stage 4 review corrections — 2026-11-25

The review found seven issues and corrected them before advancing. Eight permanent regressions were added; the existing native-tool guard regression also now covers a workspace replaced with a junction/symlink. Stage 4 currently has 27 executed feature-specific tests on Windows and one additional native Unix symlink test.

1. **Checkpoint integrity on replay and resume.** Replaying a checkpoint previously returned its digest without verifying retained bytes, and reopening an environment did not verify its saved context. Both paths now verify digest, bounded document size, and scan/agent ownership. Missing or corrupt checkpoints fail explicitly.
2. **Retained directory aliases.** A replaced environment root could resolve into another agent's directory. Reopen, new checkpoints, native tool dispatch, publication paths, and artifact materialization now reject linked roots. A Windows junction/Unix symlink regression checks that sibling files remain untouched.
3. **Manifest entry ordering.** Valid transferred manifests could fail when files preceded parent directories. Materialization now orders entries independently of wire order and applies directory permissions from children to parents.
4. **Git merge history exclusions.** Default Git log traversal omitted files introduced only by a merge commit. Exclusion checks now include root and first-parent merge diffs before bundling. A synthetic merge-only database path reproduces the previous gap and must reject capture without sealing a manifest.
5. **Execution provenance.** Artifact contracts now require the producing execution attempt and positive generation. Child publications retain their actual assignment; coordinator publications explicitly have no child assignment instead of inventing one from the scan ID. Related-child resume tests verify distinct assignments/attempts and preserved attribution. Interim development artifacts missing the required fields fail decoding; no compatibility fallback was added.
6. **Cross-session canonical validation.** Managed artifact IDs previously bypassed integrity and workspace checks when a record was edited outside a Swarm session. Known managed references and reserved central artifact IDs now retain those checks across session types, without initializing Swarms for ordinary non-managed evidence.
7. **External input exclusion boundary.** Explicit external roots could import host runtime subdirectories outside the artifact store. Capture now excludes the complete configured host-data boundary, consistent with workspace creation, including explicitly mounted inputs.

Permanent evidence: `checkpoint_replay_and_environment_reopen_require_verified_checkpoint_bytes`, `retained_directory_links_never_alias_another_agents_workspace`, `transferred_manifest_order_does_not_change_materialization_semantics`, `merge_only_excluded_git_paths_never_enter_a_sealed_snapshot`, `artifact_provenance_requires_an_attempt_and_positive_generation`, `artifacts_keep_real_assignment_and_attempt_provenance_across_resume`, `central_artifact_integrity_survives_edits_from_non_swarm_sessions`, and `explicit_external_mounts_cannot_import_any_host_runtime_directory`.

Final review validation passed: contracts 18, storage 80, Swarms 18, tools 16, and Findings 13 tests; sessions 65 with all features, 58 with only Swarms, and 47 without optional features. The Windows native-tool junction fixture initially failed because `mklink` interpreted forward slashes as switches; normalizing fixture paths to native separators corrected setup, and all 16 tool tests then passed. This fixture correction does not change production behavior.

Workspace checking (`--all-targets --all-features`), affected-crate Clippy with warnings denied, separate Swarms-only and feature-disabled session Clippy, formatting, and whitespace checks passed. The changed source, synthetic fixtures, and documentation were audited for real research material and machine-local identifiers. No remaining Stage 4 blocker was identified. Stage 4 is ready for Stage 5; subsequent work is recorded below.

### Stage 4 acceptance revalidation — 2026-11-25

The requested Stage 4 implementation was already present in the working tree, including the seven review corrections above. Revalidated it without changing production code or advancing existing Stage 5 work.

| Check | Result |
| --- | --- |
| `cargo test -p beale-contracts -p beale-storage -p beale-swarms -p beale-tools -p beale-findings --quiet` | Passed: 160 tests across these crates, including the current 24-test Windows snapshot/artifact integration suite |
| `cargo test -p beale-sessions --all-features research_files --quiet` | Passed: all 3 private-directory routing, provenance, and canonical attachment tests |
| `cargo test -p beale-sessions --no-default-features --features swarms research_files --quiet` | Passed: 1 applicable routing test with Swarms alone |
| `cargo test -p beale-sessions --no-default-features --quiet` | Passed: 48 tests |
| `cargo check --workspace --all-targets --all-features --quiet` | Passed |
| `cargo clippy -p beale-contracts -p beale-storage -p beale-tools -p beale-sessions -p beale-swarms -p beale-findings --all-features --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` and whitespace checks | Passed |

The current crate totals include previously existing later-stage tests; these results do not mark Stage 5 complete. Expected error logs in attachment tests exercise missing files, occupied destinations, and injected storage failures. The Stage 4 source and fixtures use synthetic inputs; a focused machine-local path scan found no matches. Native Unix filesystem behavior remains unverified on this Windows host. Docker lifecycle and cross-host restore remain later-stage acceptance. Stage 4 remains complete, with no acceptance blocker found in this revalidation.

## Stage 5: Distributed routing and bounded recovery

### Completion record

Completed and validated on 2026-11-26, including the final-review background-recovery correction. This stage covers supervised native workers and real HTTP transport, including a two-host test adapter. Docker provisioning, real placement/admission, and physical peer checkpoint/restore remain Stages 6-7.

Implemented boundaries:

- The app-server worker entry point bypasses guest host services. Scoped execution identity, saved policy, leader-owned inference, and canonical research routing remain authoritative on the leader; native execution uses private writable workspaces.
- Journals, immutable output captures, transactional receipts, temporary worker spooling, and generation-scoped operation identities preserve accepted results. Unknown external effects require parent reconciliation rather than automatic replay.
- Independent leases and bounded supervision fence stale attempts, stop managed execution, and require termination proof plus a durable workspace checkpoint before releasing ownership. Cancellation remains sticky; unrelated branches continue when their own dependencies are usable.
- Host-owned launch intents and exclusive ownership locks support restart discovery and late original-provenance evidence. Windows named jobs and Linux subreaper supervision have native coverage. macOS orphan recovery conservatively retains termination-unconfirmed state; automatic macOS orphan recovery and native validation are not claimed.
- Session detail reads reconstruct bounded, host-authored recovery-history commentary from persisted events, including after reopen or feature disable. Raw guest payloads are not displayed. Stage 9 still owns dedicated current-state warning controls and hierarchy presentation.

Final-review recovery correction: Core now constructs one `SessionRuntime` for the HTTP router and background scheduler. Maintenance retries retained local execution owners without a new user/model turn, holding their ownership locks until checkpoint, transcript/publication, and completion-marker persistence succeeds. Coordinator-loss recovery runs before retained-worker acceptance. Each pass processes up to 64 retained owners outside the queue mutex; failed owners rotate behind other pending work. Restart discovery continues to skip live owners.

The former chronological slice notes are consolidated here. The implementation and acceptance requirements below remain the stage's scope; later-stage integration is not a Stage 5 completion prerequisite.

### Acceptance evidence map

Paths below are relative to `beale-v2/apps/beale-app-server/beale-harness/` unless specified. Tests use synthetic providers, files, identities, and research scenarios.

| Acceptance area | Permanent regression coverage |
| --- | --- |
| Local process launch, workspace reuse, checkpoint/receipt failure, selected-destination rejection | `beale-sessions/src/distributed/gateway_tests.rs`: `local_scheduler_*`; `restart_tests.rs` |
| Generation-scoped receipts and same-generation replay | `gateway_tests.rs`: `local_scheduler_reuses_workspace_and_scopes_receipts_to_each_execution`; worker spool/storage suites |
| Findings, descendant creation, assignments, messages, and handoff deduplication | `gateway_tests.rs`: `remote_finding_lost_response_*`, `deputy_remote_spawn_*`, `worker_spawn_receipt_failure_*`, `assigned_worker_publishes_*` |
| Leader provider route, tools, scope, review rules, and credential boundaries | `gateway_tests.rs`: inference, bootstrap, saved-tool-list, review, and identity-rejection tests; worker process and Core executable tests |
| Canonical tools and mixed local-execution/central-persistence operations | Findings, Runbooks, canonical error, plugin, plugin-resource, Surface, Memory, Agent Feedback, and Webfetch regressions under `beale-sessions/src/distributed/` |
| Recovery after a completed request and restored storage | `hierarchy_maintenance_tests.rs`: checkpoint and publication fault regressions use the public `SessionRuntime` router and normal background scheduler; verify retained ownership during faults, automatic recovery, original artifact bytes, one transcript/notification, and no repeated research |
| Client disconnect and reconnect during managed research | `beale-sessions/src/hierarchy_closeout_tests.rs`: `client_disconnect_during_managed_research_preserves_completion_on_reconnect` (real HTTP client and managed worker) |
| Durable, bounded recovery notices after reconnect | `hierarchy_closeout_tests.rs`: `recovery_notices_survive_reopen_are_bounded_and_never_expose_event_payloads`; `gateway_tests.rs`: `worker_warnings_commit_before_ack_and_survive_reopen_without_a_desktop` |
| Temporary faults, exhausted recovery, and healthy sibling progress | `partition_tests.rs`; `hierarchy_tests.rs`: provider/checkpoint recovery and exhaustion regressions |
| Leader loss, terminal leader failure, and branch cancellation | `coordinator_tests.rs`; `gateway_tests.rs`: `supervisor_reconciliation_preserves_cancellation_and_terminal_leader_failure_stops_descendants` |
| Expiry/revocation, process-tree termination, replacement fencing, and cancel during reconnect | `partition_tests.rs`; `distributed/lease.rs`, `process.rs`, `supervisor.rs`; `beale-tools/src/linux_worker/` |
| Unknown command outcomes and original-provenance late evidence | `partition_tests.rs`, `late_evidence_tests.rs`; `gateway_tests.rs`: failed outcome/checkpoint recovery |
| Restart, duplicate reconnect, host ownership, and peer transport | `restart_tests.rs`, `peer_host_tests.rs`, `peer_backend_tests.rs`, `peer_coordinator_tests.rs`; peer recovery tests and `beale-storage/tests/cases/peer_workers.rs` |
| Worker/leader storage exhaustion, warning-store failure, and no false completion | Worker spool fault tests; `canonical_error_tests.rs`, `late_evidence_tests.rs`, `restart_tests.rs`; gateway transaction/receipt/publication/warning failure tests |

### Validation and remaining boundaries

Closeout gate, 2026-11-26:

| Gate | Windows | Native Linux (WSL) |
| --- | --- | --- |
| Sessions, all features | 206 passed | 199 passed |
| Storage, all test targets | 114 passed | 117 passed |
| Fleet | 27 passed | 27 passed |
| Tools/process supervision | 17 passed | 26 passed |
| Core unit and worker executable tests | 22 + 1 passed | 18 + 2 passed |
| Sessions and Core all-target/all-feature Clippy, warnings denied | Passed | Passed |

Five ignored Session functions, one ignored Storage function, and two ignored Linux Tools functions are subprocess fixtures exercised by their parent tests. The new disconnect test uses an actual HTTP client, production session routes, a managed research worker, and a synthetic provider; it reconnects to the central leader and child session records after completion. The warning projection test covers reopen, disabled new Swarms, duplicate event delivery, repeated reads, bounded output, payload exclusion, and preservation of model transcripts. Two additional maintenance regressions finish a normal session request under checkpoint/publication faults, retain ownership through a background tick, restore storage, and observe automatic recovery without a direct recovery-helper call or new model turn. Later ticks preserve the same transcript and parent notices; publication retains the original captured bytes despite a private-file edit.

Windows feature-isolation gates passed: Sessions with only Swarms enabled (163 tests) and with optional features disabled (48 tests), plus separate Sessions/Core all-target Clippy runs with warnings denied for both configurations. The workspace all-target/all-feature check, formatting, whitespace checks, and inspection of changed source, synthetic fixtures, and documentation for machine-local or live research data passed. The background-maintenance amendment passed its acceptance gate. Both new maintenance regressions were also rerun successfully on Windows and Linux after preserving coordinator-first recovery ordering. Stage 5 is complete; no Stage 6 work was added. Native Windows tests ran outside the sandbox because it denies loopback sockets; native Linux tests used the isolated test-home script. No live providers, research targets, or Docker research execution were used.

At the Stage 5 checkpoint, Core installed `UnavailableBackend`; the normal hierarchy rejected selected destinations and peer physical checkpointing remained unavailable. The partial Docker groundwork had not passed Stage 6 acceptance. Stage 6 below now installs the prepared Docker backend; selected-scan admission and coordinator checkpoint integration remain Stage 7 work. No local fallback is permitted. No macOS native gate was available.

### Implementation

- Add a worker mode of app-server with local execution and temporary recovery state.
- Route inference to the leader's app-server using the leader's configured providers and credentials.
- Route canonical research tools to the leader, including feature-owned operations such as findings.
- Execute filesystem, shell, build, and experiment operations locally. Mixed operations such as runbooks route persistence centrally and execution to the assigned environment.
- Persist transcripts, status events, messages, and handoffs centrally, with temporary worker spooling during interruption.
- Implement heartbeats, bounded retry/backoff, reconnect reconciliation, and execution ownership preventing duplicate active replacements.
- Persist warning and recovery events so they remain visible after Desktop reconnects.

### Execution expiry, termination, and reconciliation

Each attempt uses the Stage 1 lease and generation. Lease expiry/revocation prevents new inference, tool dispatch, and canonical mutations from that attempt. The worker supervisor uses a bounded local deadline to stop issuing work when it cannot renew. It requests cancellation and then termination of managed process trees; Docker backend termination is added in Stage 6. An intentional branch stop follows the same process and cannot be undone by a late reconnect.

Fencing an attempt's API access is not proof that its processes have stopped. Before replacement executes research, require confirmation from the owning supervisor that the old managed execution ended or from the execution backend that its instance stopped. If termination cannot be established, retain an explicit termination-unconfirmed state and defer replacement research. Other branches continue. Bounded recovery can stop the affected assignment without declaring its external effects known.

Journal each tool operation before dispatch and record its outcome before acknowledgement. On reconnect, reconcile persisted operation IDs and acknowledgements. Completed canonical operations replay their saved result; commands with an uncertain outcome remain outcome unknown and are not automatically rerun. New research may proceed only after the parent receives the uncertainty and reconciles the assignment. Process termination does not roll back external effects, and the protocol does not promise exactly-once shell execution.

Reject stale-attempt mutations and assignment completion. Late artifacts may be preserved as candidate recovery evidence only through host-mediated reconciliation, attributed to the original attempt; they cannot complete a replacement's work. Allow protocol acknowledgement/recovery traffic to proceed without a running model turn.

### Acceptance tests

- With no Fleet destinations selected, the queue launches deputies/workers through managed worker processes, retains their private workspace across related turns, and checkpoints it before releasing ownership. Checkpoint/receipt failure retains the process handle and blocks replacement until recovery.
- Reusing a provider tool-call ID in a later execution generation cannot replay an earlier generation's resource or tool receipt; same-generation recovery still replays once.
- The local launcher rejects selected Fleet destinations, including preparation/recovery failures, without executing local research.
- A remote `findings_create` appears once on the leader.
- A deputy-created worker becomes visible in the leader's descendant list.
- Conflicting provider configuration on a remote host is ignored for the assigned scan.
- Provider credentials never enter the worker environment.
- Desktop disconnect does not stop research.
- Lost responses do not duplicate findings, assignments, or handoffs.
- Known argument, missing-record, conflict, and state rejections return durable, bounded tool feedback. Failed mutations roll back before rejection receipts commit; checkpoint recovery preserves the feedback and permits a corrected call. Storage faults, authority failures, and corrupt accepted data must not be acknowledged as completed input rejections.
- Temporary communication failures recover without stopping unrelated branches.
- Exhausted recovery gracefully stops affected work and preserves its outputs.
- Leader failure pauses dependent work; terminal leader failure can stop the swarm.
- Worker transport rejects wrong-scan/workspace requests, parent impersonation, host-administration operations, and expired or revoked execution credentials.
- Remote execution uses the leader's enabled tools, scope, and review-rule snapshot even when guest settings disagree; guest-supplied role text cannot change review applicability.
- A partition during a running command stops new dispatch on expiry and initiates managed process-tree termination. Replacement waits for termination confirmation.
- Late results from an old attempt do not mutate current assignment state; preserved recovery evidence keeps its original provenance.
- Cancel during reconnect remains canceled and cannot resurrect the branch or its processes.
- A command whose completion acknowledgement was lost is reconciled from its journal where possible and otherwise reported as outcome unknown, never blindly replayed.
- Leader/worker restart and duplicate reconnect cannot grant two current execution generations or dispatch the same acknowledged canonical mutation twice.
- Exhaust worker storage during checkpointing: report incomplete persistence without acknowledging a checkpoint, preserve the last durable state, and suspend/stop affected execution through bounded recovery.
- Exhaust leader storage during artifact publication or canonical record commit: acknowledge no failed mutation, retain worker-side unacknowledged outputs, and reconcile/retry once space returns without duplicate records.
- When central storage cannot persist warnings or failure markers, surface the immediate error where possible and retain preexisting lifecycle protections; no successful completion, publication, or cleanup eligibility is inferred from a failed write.

## Stage 6: Docker preparation and worker execution

Status: implemented and validated on 2026-11-26. Stage 7 has not started. Native Arm64 execution is not claimed; the available Docker test host runs AMD64 Linux.

Preserve image-name entry as the normal user experience.

### Implementation

- Resolve and pull images on the owning Fleet host using that host's registry authentication.
- Detect platform compatibility, pin image digests, and package a versioned worker runtime.
- Inject the worker without assuming a particular package manager or preinstalled scripting runtime.
- Handle startup for base OS images and images whose original entrypoint must remain active.
- Provide optional configuration for images requiring environment variables or startup customization.
- Probe runtime startup, workspace writes, execution tools, leader connectivity, and artifact transfer before assigning research.
- Report per-host readiness states and concrete incompatibility reasons.
- Allocate one subagent per container instance, including deputies.
- Manage instance identity, retained storage, startup/shutdown supervision, and recovery.

Retain both private working volumes and the stopped container's writable filesystem. Installed packages outside the workspace must survive ordinary same-host reuse. Store the pinned base/prepared image identity and required mounts with the environment; container recreation must not silently discard retained changes. Same-host restart is a Stage 6 acceptance requirement. Cross-host transfer/reconstruction and its scheduling decisions belong to Stage 7.

Connect lease expiry and branch cancellation to the owning Docker host's supervisor. Confirm instance termination before releasing execution ownership for replacement. A Docker outage that prevents confirmation leaves termination unconfirmed, not successfully stopped.

Incompatible images remain clearly unavailable. There is no automatic image substitution or local fallback. Do not add special provisioning for Windows-inside-Docker images.

### Acceptance tests

- Alpine and Ubuntu images support preparation, research execution, artifact return, restart, and reuse.
- Exercise supported architectures on available test hosts; unsupported combinations fail clearly.
- Minimal images do not fail merely because an assumed bootstrap utility is absent.
- Required application startup survives where supported.
- Pull failure, missing registry access, incompatible runtime, and failed readiness probes produce actionable states.
- Concurrent subagents receive separate instances and filesystems.
- Cleanup affects only Beale-owned resources.
- Stop/restart on the same host preserves both workspace edits and packages/files installed outside the workspace.
- Lease expiry and branch cancellation stop the managed container and its research processes before replacement is authorized.
- Docker control failure leaves termination unconfirmed and prevents a concurrent replacement research attempt.
- Real container transfer preserves supported Stage 4 metadata and reports incompatible filesystem semantics before research starts.
- Preparation maintenance reloads candidates under their instance locks and cannot overwrite completed preparation or newer ownership.
- Restart termination inspection survives normal maintenance deadlines; replacement stays fenced until durable termination confirmation.
- Worker execution preserves image and explicitly configured `PATH` values, including tools outside standard directories and an original image entrypoint.
- Interrupted image pulls and failed final readiness writes recover through normal maintenance; stale preparations cannot overwrite newer readiness/configuration results.
- Default worker PID 1 reaps exited orphan children while preserving live children, including after same-container restart.
- A successful preflight retry restores `Ready`, clears the failure reason, and preserves the pinned image; stale observations cannot overwrite newer attempt, preparation, or configuration results.
- Retained-instance inspection, startup, and runtime-probe failures durably record `Unavailable` before workspace preflight, preserve termination fencing, and clear their reason after a successful repaired retry.

### Stage 6 progress and acceptance evidence

The implementation adds versioned image/startup/readiness contracts, authenticated host configuration routes, the independently migrated `fleet-docker` schema, static runtime packaging and injection, and a Docker adapter for the existing assigned-worker supervisor. Core installs that adapter for execution control and restart recovery. Host-owned preparation requires a reserved instance ID; it does not choose a host, admit capacity, or schedule selected scans.

Each instance records its pinned image, original container ID, named volume, configuration revision, and logical agent/attempt ownership. Creation intent precedes Docker mutations. Guest preflight verifies the initial snapshot and actual artifact upload/download before inference; resumed attempts verify retained publication receipts instead of reseeding the workspace. Cancellation, lease expiry, lost daemon access, interrupted preparation, and failed durable stop writes retain ownership until termination and persistence are confirmed. Recovery also handles cancellation before a new attempt's spawn future runs. Stop observations survive short maintenance polls. No image substitution, local fallback, or automatic recreation of missing retained state was added.

Permanent opt-in acceptance tests are in `beale-sessions/src/distributed/docker_tests.rs`; fast package/configuration/ownership regressions are in `beale-fleet/src/docker/tests.rs`. Retained snapshot identity and missing-marker coverage is in `beale-storage/tests/swarm_files/snapshot_transfer.rs`.

| Requirement | Acceptance evidence |
| --- | --- |
| Alpine and Ubuntu research, artifacts, restart, reuse | `docker_alpine_and_ubuntu_research_artifacts_metadata_and_retained_reuse` runs real injected workers against the production scoped gateway and synthetic provider. Both images complete research, publish a large output artifact, reopen destination state, and run a related second attempt in the same container. |
| Supported/unsupported platforms and runtime integrity | The AMD64 static musl worker runs on both images. Package tests reject architecture mismatch and changed runtime bytes; the real preflight test rejects Arm64 selection without a matching bundle before allocating an instance. Native Arm64 remains untested because no matching host/bundle was available. |
| Minimal image bootstrap and default startup | `docker_default_idle_minimal_image_and_actionable_incompatibility` uses the default worker PID 1, removes conventional executable directories in its disposable container, and successfully restarts/probes the injected worker and Bash. No guest package manager or scripting runtime is used for injection. |
| Required application startup and customization | The Ubuntu case keeps its configured application process running through preparation/restart; the image-default path preserves the entrypoint and supports explicit command, environment, and user settings. |
| Actionable preparation failures | Missing runtime, unavailable Docker/pull access, unmatched architecture, and an unwritable selected user produce unavailable states. Workspace writability failures expose their exact bounded reason. Configuration HTTP tests cover host authorization, unsupported versions, unknown fields, and revision conflicts. |
| Separate instances and owned cleanup | Concurrent Alpine/Ubuntu research uses distinct container IDs and volumes. Foreign ownership fails cleanup checks without deleting the instance. Successful tests remove only their owned resources; failed synthetic fixture resources were also removed after verifying their stored identities and labels. |
| Retained workspace and installed files | Both images retain workspace edits and an installation file outside the workspace across host-state reopen and a second research attempt. Default-idle restart also preserves an outside-workspace file after removing image utilities. Deleting the retained container makes restart fail rather than recreating it. |
| Cancellation, lease expiry, daemon outage, and restart | `docker_cancellation_lease_expiry_daemon_loss_and_background_recovery` stops whole containers, including detached processes; daemon loss leaves termination unconfirmed and rejects replacement. Normal preparation maintenance stops an interrupted setup without recreating it. |
| Failed persistence after termination | The same regression injects a failing durable stop write, verifies that no stopped record is acknowledged, restores storage, and confirms retry persists the stop before reuse. It also covers cancellation before a resumed attempt binds its instance. |
| Real transfer metadata and filesystem readiness | Container snapshots preserve file bytes, executable bits, Unix modes, symlinks, and empty directories across a Windows-owned host/real Linux guest boundary. Writable-storage failure is rejected before research. Git repository snapshots retain Stage 4's requirement that Git be installed in the selected image; Git is not automatically installed by this stage. |

Validation passed:

- Windows: Contracts 39 tests, Storage 115, Fleet 33, Sessions 206, Tools 17, Core 22 plus its worker-entry integration case. The three opt-in Docker cases passed with the packaged Linux worker; the final stop-persistence and concrete-readiness amendments each passed their focused Docker rerun.
- Native Linux: Contracts 39 tests, Storage 118, Fleet 33, Sessions 199, Tools 26, Core 18 plus two worker-entry integration cases. Subprocess fixture functions and opt-in Docker cases remain ignored in ordinary native gates; Docker was exercised separately from the Windows host against the Linux daemon.
- Optional-feature isolation: Swarms-only Sessions 163 tests; disabled Sessions 48 tests; disabled Core check and disabled Core/Sessions Clippy passed. Full affected-crate Clippy passed on Windows and Linux, and the Windows workspace all-target/all-feature check passed.
- Static AMD64 musl runtime build/package, formatting, whitespace, and inspection of changed code, fixtures, and documentation for live research or machine-local data passed. No live provider or research target was used.

### Stage 6 review fixes (2026-11-26)

All four review findings have implementation fixes and permanent regression coverage:

| Finding | Fix and regression coverage |
| --- | --- |
| Stale preparation recovery overwrites completed state | Recovery pages contain candidate IDs; instance and journal state are reloaded under the instance lock. `preparation_recovery_reloads_candidates_after_acquiring_ownership` completes preparation and records ownership after page selection, then verifies recovery preserves readiness, ownership, and target validation. |
| Short maintenance polls repeatedly cancel restart inspection | The destination supervisor retains a shared recovery task and its ownership across dropped polls. `peer_restart_recovery_survives_normal_background_maintenance_deadlines` uses the production maintenance loop with a six-second backend inspection, verifies replacement remains fenced past the five-second poll budget, and observes durable completion without repeating recovery or spawning research. |
| Worker launch replaces the image's `PATH` | The injected Bash wrapper prepends the runtime directory to the guest's effective path using positional arguments. Container research commands use non-login Bash; the new real-container regression exposed and now covers login-profile path replacement as well. `container_shell_preserves_custom_path_and_skips_login_profile` adds fast Unix coverage. `docker_image_entrypoint_and_custom_path_survive_research_and_reuse` exercises both image-provided and explicitly overridden paths in disposable images with a real original entrypoint; a synthetic research tool exists only outside standard directories. Both variants perform research, artifact publication, restart, and reuse. |
| Interrupted preparation leaves readiness stuck | The `fleet-docker` schema version 2 journals preparation atomically with `Preparing` before image pulls. Completion commits readiness and journal removal together. `preparation_journal_recovers_interrupted_pull_without_disturbing_live_owner` and `preparation_journal_retries_failed_final_readiness_write_after_restart` exercise normal restart maintenance at both failure boundaries. `stale_preparation_cannot_overwrite_newer_preparation_or_configuration_readiness` checks concurrent preparations and configuration revision changes. |

Review-fix validation:

- Windows native acceptance: Contracts 39, Storage 115, Fleet 37, Sessions 207, Tools 17, Core 22 plus its worker-entry integration case passed.
- Native Linux: Fleet 37, Sessions 200, and Tools 27 passed. Docker and subprocess fixtures remain opt-in/ignored in ordinary native suites.
- Optional-feature isolation: Swarms-only Sessions 164 and disabled Sessions 48 passed; disabled Core check and Core/Sessions Clippy passed.
- Changed Tools/Fleet/Sessions all-feature checks and all-target Clippy passed on Windows; all-target Clippy passed on Linux. Static AMD64 musl worker rebuild/package, formatting, whitespace checks, and inspection for real research material or machine-local data passed.
- Real-Docker acceptance: all four opt-in tests passed with the rebuilt static AMD64 worker. This includes the original Alpine/Ubuntu, cancellation/lease/daemon-recovery, and minimal-image cases, plus both custom-path/original-entrypoint variants. The new test initially exposed a second path reset in login-shell execution; the final run passes with container-only non-login Bash. Native Arm64 remains untested because no matching host/bundle was available.

### Stage 6 follow-up fixes (2026-11-26)

- Default `worker_idle` PID 1 now reaps exited children in bounded batches on `SIGCHLD` and periodic sweeps. Shutdown signals retain priority; live children remain running. `docker_default_idle_reaps_orphans_and_preserves_live_children` creates 32 short-lived orphans plus a live child, verifies exited PIDs disappear while the container/live child survive, and repeats after restarting the same container. Image-default entrypoints retain responsibility for their own init behavior.
- Successful guest preflight now restores `Ready` and clears the prior failure reason. The `fleet-docker` schema version 3 adds a monotonic readiness revision; observation tokens bind the configuration, instance, pinned image, and exact execution attempt. Superseded observations are ignored, and failed current persistence prevents research startup. Fast tests cover durable success after failure, duplicate/late observations, newer attempts and preparations, configuration edits, and retry after a failed readiness write. `docker_preflight_retry_restores_readiness_after_leader_rejection` rejects a synthetic credential before research, confirms termination, then retries with valid authority, completes research/artifact return, and reuses the original instance with restored readiness.

Follow-up validation:

- Windows: Fleet 40, Sessions 207, Core 22 plus one worker-entry integration test passed.
- Native Linux: Fleet 40, Sessions 200, Core 18 plus two worker-entry integration tests passed.
- Swarms-only Sessions 164 and disabled Sessions 48 passed; disabled Core check and Core/Sessions Clippy passed. Changed-crate checks, all-target Clippy on Windows/Linux, formatting, whitespace checks, and inspection for real research material or machine-local data passed.
- Static AMD64 musl worker rebuilt and packaged. All six real-Docker acceptance tests passed, including both new regressions, Alpine/Ubuntu execution and reuse, minimal-image compatibility, custom entrypoints/PATH, and background recovery. Native Arm64 remains untested because a matching host/runtime bundle was unavailable.

### Stage 6 early-readiness fix (2026-11-26)

- Retained-container inspection, startup, runtime probing, and workspace preflight now share one guarded failure path. Early errors persist `Unavailable` with the bounded failure reason; successful preflight still restores `Ready`. The existing observation revision guards and termination-confirmation requirements remain in force. No schema change is required.
- `docker_early_preflight_failures_persist_readiness_and_retry` exercises an unavailable inspection CLI, an entrypoint that exits on startup, and a non-executable injected shell. Each failure starts from `Ready`, verifies the exact failed boundary, reopens storage to check the failure reason and pinned image, confirms no inference occurred, and verifies replacement remains fenced until recovery confirms termination. After repair, the normal supervisor completes research/artifact return and a second related turn using the same container; both turns verify `Ready` with no failure reason.
- Validation: Windows Fleet 40 and Sessions 207 passed; native Linux Fleet 40 and Sessions 200 passed. Swarms-only Sessions 164 and disabled Sessions 48 passed. Enabled/disabled Sessions checks, all-target Clippy on Windows/Linux, formatting, whitespace, and inspection for real research material or machine-local data passed.
- All seven real-Docker acceptance tests passed, including the new failure/retry regression with exact failure-boundary assertions. The existing verified static AMD64 worker bundle was used because this change affects host preflight orchestration, not guest execution. Native Arm64 remains untested; no matching host/runtime bundle was available. Stage 7 was not started.

Stage 6 supplied the prepared-environment adapter. Stage 7 below integrates reservations, automatic placement, checkpoints, cross-host reconstruction, and retention/deletion policy. The later Fleet UI remains unimplemented.

## Stage 7: Allocation, storage reserves, and retention

Allocation is harness-owned; agents request subagents without selecting machines.

### Placement and admission

- For swarm deputies/workers, no selected destinations means execute locally. No Subagents/new Swarms-disabled scans are local-only and reject nonempty destination selections under Stage 1's contract.
- Selected destinations: use only eligible configurations/VMs on session-relevant Fleet hosts.
- Start or allocate instances automatically when capacity permits.
- Never fall back locally after a selected destination fails.
- Consider RAM and storage, not CPU utilization.
- Preserve the 1 GB available-RAM admission floor.
- Require the configurable 5 GiB storage reserve to remain after estimated startup needs and outstanding reservations.
- Account for Docker storage, private working storage, and central artifact storage without double-counting shared filesystems or cached image layers.
- Deny new spawns when no feasible placement exists after accounting for the requesting parent's safe, identified release, with a useful reason and guidance to stop lingering subagents using `swarm_stop_subagent`. Feasible but not-yet-admitted work may return queued as specified below; queued acceptance never bypasses execution resource checks.
- Schedule fairly across scans and branches. Idle deputies must not indefinitely occupy all capacity needed for their workers.

### Resource reservations and waiting agents

Measure capacity in the actual execution domain, including applicable Docker/VM memory limits and backing storage, rather than assuming the app-server host's total memory is usable by every destination. Reservations are issued atomically by the host that owns the capacity and are shared across participating scans/leaders; leader-side cached inventory is advisory.

Reserve an estimated incremental startup/working-memory footprint for each new or resumed instance. Derive it from preparation measurements and configuration, with an explicit conservative estimate when measurements are unavailable. The estimate is not a hard memory cap. Admit only when effective available RAM, less outstanding reservations not yet reflected in measurements and the new estimate, remains above 1 GB (1,000,000,000 bytes). Reconcile reservations with observed usage to avoid charging running memory twice; expire abandoned launch reservations only after reconciling whether an instance actually started. Storage admission applies the same ownership/reconciliation principle with the 5 GiB reserve.

Use Stage 3's waiting/suspended lifecycle to release idle deputy execution capacity at a safe checkpoint. A ready child can then run even when there is capacity for only one subordinate instance. Persist child completion before releasing its instance, then wake its parent through the central inbox. Reserve wake-up opportunities so repeated spawning cannot indefinitely starve completed-result processing. Do not treat temporary allocation waits as provider or agent failures.

Separate the feasibility check from admission. A queued assignment may rely on reclaiming its requesting deputy's measured/releasable execution footprint, but not on imaginary free RAM, deletion of retained files, or an unrelated agent stopping. Release-dependent acceptance records that dependency and consumes no execution lease. Only confirmed suspension/release followed by a fresh atomic reservation permits launch; the RAM floor and storage reserve apply to that actual admission without exception.

If another scan takes the released capacity, retain the same queued assignment under the fair scheduler, without duplicating the child or falling back locally. Use bounded scheduling rechecks, not model polling. If the release fails, feasibility disappears, or bounded allocation recovery is exhausted, mark the assignment blocked/failed with a reason and queue a durable notification to its parent; reacquire parent execution when capacity permits. Parent cancellation cancels its unstarted children. Do not leave accepted work hidden in an indefinite pending state.

### Cross-host environment recovery

Try retained same-host resume first. To move to another selected eligible host, stop/confirm the old execution, acquire lifecycle protection for its retained state, transfer the workspace and retained root-filesystem changes or reconstruct a verified equivalent, and validate compatibility before dispatch. Preserve the stable agent identity while creating a new execution generation. If files, installed tooling, or platform requirements cannot be restored, return a recoverability failure and retain the original state; do not silently launch a clean substitute or fall back locally.

Token/resource pacing remains best effort. Low-space warnings and admission decisions do not guarantee future workload size. Crossing the reserve stops new admission rather than immediately terminating running research. An actual storage failure triggers recovery or graceful stop of affected work.

For storage exhaustion after admission, stop accepting writes that cannot be made durable, preserve the last committed state, and retain existing active-use/publication protections. Failed checkpoints and uploads never make an environment cleanup-eligible. Retry interrupted publication/checkpoint operations with their original identities after space returns. If the failure also prevents persisting a new warning, emit it through available live channels and reconcile from prior durable state on recovery; do not pretend a failure marker was saved. Unrelated branches continue only while their required storage and canonical operations remain usable. Exhaustion of the leader's shared database may pause all dependent canonical work without declaring successful completion or discarding private environments.

### Retention

| State/data | Policy |
| --- | --- |
| Active or paused scan | Retain private environments |
| Completed subagent | Stop execution; preserve context and files for reuse |
| Ended scan | Retain private environments for seven days by default |
| Unuploaded/unacknowledged outputs | Protect from automatic deletion; show Needs attention |
| Published artifacts and records | Retain under their explicit workspace/record lifecycle |
| Unreferenced caches | Eligible for cleanup |
| User-kept environment | Exempt from timed cleanup until released |

Provide configurable retention, **Keep**, and **Delete now** controls. Closing Desktop does not end a scan. Low storage does not silently evict paused work or run broad Docker pruning. Normal scan completion attempts a final checkpoint and publication before cleanup eligibility.

### Atomic retention and deletion lifecycle

Start the seven-day clock from a persisted scan-ended event: normal completion, explicit user end, or terminal failure after recovery has been exhausted. Pausing, disconnecting, completing one assignment, or stopping one child does not end the scan. Unconfirmed executions remain protected from cleanup even if the scan has ended.

The authoritative lifecycle record includes a generation, retention deadline, Keep flag, active-use/restore references, publication dependencies, and deletion state. Resume and Keep acquire protection or update the generation transactionally. Cleanup claims only the unchanged, eligible generation with no protected dependencies. After a deletion claim wins, resume/Keep must report deletion in progress rather than partially restoring an environment; if resume/Keep wins first, cleanup must fail its claim. Reopening a retained ended scan clears the old end deadline before execution is allocated.

Record physical deletion steps durably and make them idempotent. A crash during cleanup resumes deletion of that claimed generation only. Reference counts for published evidence and shared snapshots/images are separate from private-environment retention. Accepted `swarm_plugin_resources` manifests also pin their content-addressed blobs while an attempt or retained workspace can still need them; resource transfers and restore must participate in the same protected-use lifecycle. Removing a private environment cannot remove bytes still referenced by durable records or another environment.

Delete now uses the same ownership checks. It cannot silently discard unpublished/unacknowledged outputs or delete an active, restoring, or termination-unconfirmed environment. Present the concrete blocker; explicit discard of the identified pending outputs requires a separate destructive confirmation. Reconcile lost publication acknowledgements by stable operation/artifact identity before declaring outputs missing or allowing deletion. Acknowledged published artifacts keep their independent lifecycle even after explicit private-state deletion.

### Acceptance tests

- Concurrent spawn requests cannot reserve the same capacity.
- When no eligible placement can retain more than 1 GB of available RAM even after the requesting parent's identified safe release, a spawn receives a resource denial; genuine storage insufficiency also denies it.
- Storage calculations include pending image/input allocations.
- Unknown capacity produces an actionable placement failure.
- No fallback occurs during preparation, execution, or recovery.
- Completed deputies release running resources while retaining resumability.
- Fake-clock tests cover seven-day expiry, paused scans, Keep, and protected uploads.
- Shared caches remain while referenced; unrelated Docker resources remain untouched.
- Admission tests distinguish host RAM from execution-domain limits, account for estimated instance footprints, and avoid double-counting measured running usage.
- Competing leaders reserve through the same owning host; a crash between reservation and launch neither leaks capacity indefinitely nor releases a live instance's reservation incorrectly.
- Start with one running deputy occupying the only subordinate slot and no child assignment: its first spawn returns queued, the deputy checkpoints and suspends, admission rechecks capacity, its worker runs and completes, and the deputy resumes to consume the result.
- Multiple waiting deputies make progress without duplicate wake-ups or indefinite result-processing starvation.
- Cross-host recovery restores workspace edits and installed tooling on a compatible selected host; incompatible/lost retained state produces an explicit failure without a clean substitute or local fallback.
- Prepared plugin bundles survive interrupted download, host restart, retained-workspace resume, and cross-host restore; cleanup cannot remove their blobs while a consumer or restore reference is protected, and releasing them cannot delete shared evidence bytes.
- Resume versus expiry and Keep versus cleanup races have one atomic winner; the loser receives the documented state and never observes partial restore.
- Cleanup restart after a crash completes only the claimed generation and preserves durable evidence/shared references.
- Lost upload acknowledgements are reconciled before deletion; pending outputs remain protected and explicit discard cannot remove published artifacts.
- Ending, pausing, reopening, stopping a child, and terminal failure set or preserve retention deadlines according to the documented lifecycle.
- Parent suspension/checkpoint failure never launches a queued child against unreleased capacity; bounded recovery makes the blocked result visible without duplicating assignments.
- Another scan acquiring released capacity leaves the same child queued fairly; later capacity permits one launch, while cancellation removes the unstarted assignment and no local fallback occurs.
- RAM/storage insufficient even after the identified parent release is denied rather than queued; retained filesystem bytes are not treated as reclaimable just because execution suspends.
- Fill worker checkpoint storage after admission, then restore space: the last committed checkpoint survives, unpublished files are protected, and recovery never reports a failed checkpoint as saved.
- Fill leader artifact storage and database storage separately during publication/commit, including a lost response; verify no false acknowledgement, no dangling durable reference, no duplicate retry, and no cleanup of unacknowledged outputs.
- Restore storage and resume publication with the original operation IDs; unaffected branches continue where dependencies remain available, while a shared canonical-store failure pauses dependent work explicitly.

### Stage 7 review regression requirements

- An accepted release-dependent child remains in the owning host's queue when a competing scan takes the yielded slot; normal maintenance admits it once, then the deputy resumes.
- Replaying an old checkpoint acknowledgement after the next reservation exists must preserve that reservation, its reference, and its environment generation/state.
- Persist destination and exact export-source intent before requesting export. Failed writes and lost export replies remain protected until destination cancellation is confirmed; normal maintenance after leader restart releases the pin and fences delayed export requests.
- Retry transient BeginRestore, ReadArchive, WriteArchive, and FinishRestore transport failures with the same allocation, instance, bundle, and offsets. Bound retries and total transfer time; renew only live unstarted reservations, never revive expired or running allocations.
- Editing a configuration after enqueue cannot prepare a different revision under the old reservation. Reconcile the unstarted reservation and report the mismatch before creating a container.
- A remote prepared/canceled allocation with no execution attempt still receives scan lifecycle updates and expires through normal owning-host maintenance.
- A→B→A restoration uses a fresh protected instance on return, preserves the latest workspace/root tooling, and remains possible when an older copy on that host is a deletion tombstone. Retries preserve the chosen new instance.

### Stage 7 implementation progress (2026-11-26)

Implemented and validated. Stage 8 has not started.

- Protocol 2.40 saves destination selections and optional retention settings. Ordinary/No Subagents requests reject these settings. Empty selections use native workers; selected scans use authenticated owning-host allocation and the peer supervisor.
- Allocation, specification, lifecycle, cancellation, and discard journals provide atomic reservations, immutable retry intent, scan/branch fairness, result wakeups, bounded waits, retained-use generations, and idempotent deletion. Queued work has no execution lease. Launch binding and backend recovery share the instance lock; no deadline alone releases potentially live execution.
- Actual execution-domain RAM and backing storage feed admission. Memory reservations subtract measured usage; pending shared image bytes are charged once. Leader artifact storage is separately reserved on the canonical filesystem. The default floor is strictly more than 1 GB RAM and the default disk reserve is 5 GiB. Native/canonical incremental storage estimates are 256 MiB; Docker estimates reserve 2 GiB image growth and 2 GiB private growth plus inputs and restore copies. Estimates remain best effort, not workload limits.
- Selected deputies checkpoint/suspend before a release-dependent child runs. Central inbox completion wakes the retained parent. Allocation exhaustion and failed release become durable blocked notices; unstarted cancellation fences late requests. Source restore protection is released only after destination preparation or confirmed cancellation.
- Cross-host transfer captures workspace, runtime/plugin state, and the installed root filesystem into immutable verified archives. Interrupted uploads resume at the original offset. Compatibility and retained-state failures never create a clean replacement. Host-authenticated stop proof permits publication recovery before central completion without prematurely ending the local turn.
- Retention starts from persisted scan end and clears on reopen. Keep, Delete now, and separately confirmed pending-output discard use generation checks. Physical cleanup resumes the claimed container/volume/private directory only; private deletion leaves a tombstone and preserves published evidence and shared snapshot/plugin/image storage.
- Host APIs provide Stage 7 controls; Desktop presentation remains Stage 9. Hyper-V, native Arm64 acceptance, and non-WSL Docker Desktop measurement remain outside the validated backend boundary. Existing Stage 4/5 storage-fault tests continue to cover canonical upload/commit failures and original-identity retries; Stage 7 adds retained checkpoint-write failure protection and retry.

Acceptance mapping (paths below are relative to `beale-v2/apps/beale-app-server/beale-harness`):

| Acceptance area | Permanent coverage and result |
| --- | --- |
| Atomic admission across leaders; RAM floor; execution-domain limits; measured-usage reconciliation; unknown/stale capacity | `beale-storage/tests/fleet_allocation.rs` concurrency and capacity cases; real `docker_measures_execution_domain_and_backing_storage` — passed |
| Pending images, private inputs, shared disk/canonical reservations, retained bytes after suspension | Allocation ledger tests for shared image keys, storage floors, parent release, and canonical storage reservations — passed |
| Queued parent release, competing scans, cancellation, fair branches and result wakeups | Allocation ledger tests for release-only feasibility, failed checkpoints, competing scans, fair wakeups, bounded waits, and cancellation before a lost enqueue — passed |
| One subordinate slot from the first spawn through worker completion and parent reuse | `hierarchy_allocation_tests.rs::docker_one_slot_deputy_yields_worker_completes_and_parent_resumes` uses the production allocator, peer supervisor, and background maintenance; exactly one worker execution and two deputy generations — passed |
| No local fallback and immutable placement/retry identity | Ordinary-request boundary tests, selected-destination one-slot integration, full-spec retry tests, and existing Docker readiness/preparation failure regressions — passed |
| Reservation/launch crash boundary and stale maintenance snapshots | Ledger expiration never releases a potentially live execution; Docker restore regression reloads the bound execution from a stale pre-launch snapshot and retains checkpoint protection — passed |
| Cross-host private edits, installed tooling, and prepared plugin state | `beale-fleet/src/allocation/tests.rs::docker_retained_restore_checkpoint_protection_and_claimed_cleanup` uses separate source/destination host databases and actual Docker environments; verifies all three state classes after transfer — passed |
| Incompatible/missing state, interrupted transfer, retry identity | Same Docker test rejects incompatible architecture and incomplete archives, replays chunks idempotently, verifies complete archive digests, and repeats restore without creating a new instance — passed |
| Seven-day expiry, Keep, pending output protection, reopen, deletion claims/restart | Fake-clock allocation tests plus independent-connection concurrent Keep/reopen versus cleanup races and deletion journal replay — passed |
| Disconnect, completion, exhausted leader failure, and ended-scan admission | Hierarchy closeout/local-tree assertions plus lifecycle revision tests verify deadlines, no deadline on disconnect, terminal end, and no admission before an explicit newer reopen — passed |
| Pending output discard and independent evidence/shared storage | Exact unexpired discard-preview tests and `swarm_files.rs::deleting_private_environment_preserves_shared_snapshot_and_published_evidence`; Docker cleanup retains the shared imported image — passed |
| Plugin resource pins, interrupted transfer and retained private edits | Existing plugin resource transfer/replay tests and private-edit retention tests rerun; Docker cross-host restore preserves the prepared resource tree — passed |
| Post-admission checkpoint/storage failure and recovery | Docker checkpoint-write obstruction retains pending protection, publishes no false manifest, and succeeds on original-identity retry; Stage 4 checkpoint write/commit fault regressions rerun — passed |
| Leader artifact/database failure, lost acknowledgements, original-ID retry and unaffected work | Existing `publication_faults_keep_pending_bytes_and_retry_identity`, `record_commit_failure_leaves_accepted_bytes_recoverable_without_dangling_refs`, gateway receipt/commit fault tests, and normal background publication/checkpoint recovery regressions rerun — passed |
| Confirmed remote termination before publication versus central completion | Peer termination-proof tests reject live/foreign attempts while preserving central ownership; the real one-slot test verifies checkpoint acceptance does not prematurely end the turn — passed |

Final validation:

- Windows contracts/storage/Fleet/Core gate: **240 passed** (Contracts 41, Storage 135, Fleet 41, Core 22 plus one worker-entry integration). Three opt-in/subprocess fixtures were skipped in this aggregate gate.
- Windows Sessions: **207 passed**, 13 opt-in/subprocess fixtures skipped. Independent Swarms-only Sessions: **164 passed**; disabled Sessions: **48 passed**. Disabled Core/Sessions all-target Clippy passed.
- Native Linux storage/Fleet/Sessions gate: **379 passed** (Storage 138, Fleet 41, Sessions 200); opt-in Docker/subprocess fixtures remain separate. All-target Core/Storage/Fleet/Sessions Clippy passed with warnings denied.
- Real Docker: all **seven existing Stage 6 session regressions passed**, and all **three new Stage 7 tests passed** (execution-domain capacity, retained restore/checkpoint/cleanup, one-slot scheduling). The restore test was rerun after adding stale-maintenance coverage. Cross-host restore uses two isolated owning-host stores on one physical Docker daemon; it is not a two-physical-machine interoperability claim.
- Windows affected-crate all-target Clippy, workspace all-feature/all-target check (including Desktop's shared request constructors), formatting, whitespace checks, and inspection for real research material/machine-local data passed. Tests used synthetic providers and fixtures; no live provider or research target was used.
- Stage 7 controls are authenticated host APIs. The later Desktop UI, provider pacing/accounting, Hyper-V, native Arm64 validation, and non-WSL Docker Desktop measurement were not implemented. Existing verified AMD64 portable worker binaries were reused because this stage changes host allocation/storage orchestration rather than guest execution.

### Stage 7 review fixes

All seven review findings are fixed with permanent regression coverage. The acceptance gates below passed; Stage 8 remains unstarted.

| Finding | Resolution and permanent regression |
| --- | --- |
| Accepted child blocked by competing capacity | Placement retains a reachable capacity-denied destination as a queue candidate; `docker_accepted_child_queues_when_competitor_takes_yielded_slot` runs production scheduling, peer supervision, and maintenance with competing capacity and exactly one child execution. |
| Old checkpoint acknowledgement releases newer reservation | Match the bound execution and treat completed acknowledgements as receipts; the Docker retained-restore test replays an old acknowledgement after the next reservation, and `old_stop_receipt_cannot_modify_reused_environment` covers both reserved and started successors. |
| Export protection has no durable owner after failure | Atomically journal destination plus source target before export. Protocol 2.41 adds `CancelExport`; a tombstone scoped to the handoff ID under the instance lock prevents delayed export requests from reacquiring a released pin while allowing a new handoff ID to transfer the same retained execution. `failed_export_and_lost_response_are_reconciled_after_restart_by_normal_maintenance` covers failure/lost reply and restart; the Docker test adds an actual full-export write obstruction and retry. |
| Transient restore error immediately blocks work | Replay each transport operation at most three times with the same identity/offset, with a 30-minute overall transfer bound and periodically renewed 11-minute unstarted transfer reservations. `restore_retries_lost_chunk_acknowledgement_with_same_identity_and_offset` injects failures at begin, read, acknowledged write, and finish; ledger tests reject renewal after expiry/start. |
| Preparation uses the latest configuration | Preparation checks the reserved revision before its durable preparation intent and any Docker mutation. `changed_configuration_is_rejected_before_reserved_instance_preparation` edits the image between enqueue and preparation and verifies reconciliation without instance creation. |
| Scan end misses remote allocations without attempts | Lifecycle host discovery includes durable placement/export intents. `docker_unclaimed_remote_allocation_receives_scan_end_and_expires` uses a separate host database, cancels after real preparation but before claim, ends the scan, and verifies automatic physical cleanup. |
| Return to an earlier host is rejected | Explicit restore chooses a new instance; same-host reuse selects the newest retained allocation. The Docker retained-restore test now covers A→B→A, latest workspace/root edits, immutable retry identity, an older deleted copy, and a new transfer after canceling an earlier handoff. |

The new `swarm-placement-exports` and `fleet-export-cancellations` journals are additive schema version 1. Rebuild participating hosts together for protocol 2.41. No provider pacing or Stage 8 behavior is included.

Review-fix validation (supersedes the original Stage 7 counts above):

- Windows contracts/storage/Fleet/Core: **244 passed**, 3 opt-in/subprocess fixtures skipped (Contracts 42, Storage 137, Fleet 42, Core 23 including its worker-entry test).
- Windows Sessions all features: **209 passed**, 15 opt-in/subprocess fixtures skipped. Independent Swarms-only: **166 passed**, 15 skipped; disabled Sessions: **48 passed**.
- Native Linux storage/Fleet/Sessions: **384 passed**, 18 opt-in/subprocess fixtures skipped (Storage 140, Fleet 42, Sessions 202). The final suite includes the expanded transport fault and handoff-ID cancellation tests.
- Real Docker: **12 distinct cases passed**: seven existing Stage 6 session cases plus five Stage 7 cases (capacity, retained checkpoint/export/A→B→A recovery, original one-slot scheduling, competing-slot scheduling, and unclaimed remote retention). The retained-restore case was rerun after scoping cancellation to individual handoff IDs and verifies a new transfer after cancellation. These fixtures use separate host stores on one physical Docker daemon; no two-physical-host interoperability claim is made.
- Windows and Linux affected-crate all-target Clippy passed with warnings denied. Disabled Core/Sessions Clippy and workspace all-feature/all-target checking, including Desktop, passed. Formatting, whitespace, and task-file inspection for real research material/machine-local data passed.
- Tests use synthetic providers and research fixtures. Native Arm64, non-WSL Docker Desktop capacity measurement, Hyper-V, later Desktop controls, and Stage 8 remain outside this implementation. The existing verified AMD64 portable guest runtime was reused; these fixes change host orchestration and persistence.


### Stage 7 follow-up review fixes

All five remaining review findings are fixed with permanent regression coverage. Acceptance reruns passed. Stage 8 remains unstarted.

| Finding | Fix and permanent coverage |
| --- | --- |
| Retry overlaps the original restore/export operation | Typed host lock contention becomes protocol 2.42 `Busy`, preserving the same request through an 11-minute operation deadline. Transport failures remain limited to three attempts; terminal rejection remains terminal and the overall restore limit remains 30 minutes. `lost_restore_connection_waits_for_original_owner_and_busy_wait_is_bounded` loses the first connection while an owner continues, exercises the real host Busy response, verifies completion under the same reservation, and verifies bounded exhaustion without releasing ownership. |
| Configuration drift poisons later retries | Same-host selection skips only confirmed unbound reservations with neither an instance nor a preparation intent; real retained or uncertain state remains protected. `changed_configuration_is_rejected_before_reserved_instance_preparation` now verifies a fresh reservation at the new revision; the real Docker retained-restore case now starts with revision rejection and successful preparation retry. |
| Partial exports survive deletion | `fleet-allocation-archives` version 1 journals exact instance ownership before archive writes. Cleanup removes owned partial components even when no manifest was published, retaining other owners' data. `interrupted_exports_are_deleted_by_owned_journal_without_manifest` verifies journal reopen, idempotent cleanup, and unrelated-file preservation; the real Docker export obstruction asserts ownership was persisted before failure. |
| Resume commits an unchecked alternative host | `swarm-placement` version 2 distinguishes provisional export routing from finalized placement. Read-only feasibility checks use the actual retained bundle and skip unavailable/incompatible hosts before committing a destination. Enqueue retries cannot change a finalized route. `restore_selection_skips_unavailable_or_incompatible_hosts_and_pins_selected_route` covers three hosts, restart, changed inventory, and cancellation before and after route selection, including a late enqueue after cancellation. |
| An old host prevents cleanup of newer intents | Per-intent failures are collected while independent cancellations continue. Selection cancellation is durable before routing can finalize. `maintenance_cancels_current_handoff_despite_unavailable_historical_host` covers both missing pairing and unreachable historical host, using normal maintenance to cancel the current allocation and release its source pin, including repeated reconciliation. |

The new Busy reply requires protocol 2.42 hosts. Rebuild participating hosts and clients together. No provider pacing or Stage 8 work is included.

Validation after these fixes:

- Windows host acceptance (`beale-contracts`, `beale-storage`, `beale-fleet`, and `beale-core`, all features): **246 passed, 3 ignored**. The strict allocation-reply contract rejects unknown Busy fields.
- Windows Sessions acceptance (all features): **212 passed, 15 ignored**, including all new placement and normal-maintenance regressions.
- Native Linux acceptance (`beale-storage`, `beale-fleet`, and `beale-sessions`, all features): **388 passed, 18 ignored**.
- Optional-feature isolation: Swarms-only Sessions **169 passed, 15 ignored**; disabled-feature Sessions **48 passed**. Disabled Core/Sessions Clippy also passed.
- Explicit real Docker acceptance: **12 passed** (2 Fleet and 10 Sessions), including configuration-revision retry, retained cross-host restore, failed-export ownership, one-slot deputy progress, competing allocation, and remote retention cleanup. Ignored test counts above include these opt-in cases; remaining platform/provider-specific cases were not run.
- Windows workspace all-feature/all-target compilation, affected-crate Windows and Linux Clippy, formatting, and diff checks passed. Changed source, tests, and documentation were inspected for machine-local or live research data; fixtures remain synthetic.

These results supersede the earlier Stage 7 validation counts. Docker cross-host coverage uses separate owning-host databases on one physical daemon; native Arm64, macOS Docker, and Hyper-V remain outside the tested boundary. No live provider or research target was used. The existing verified AMD64 guest runtime was reused because these fixes change host orchestration, protocol handling, and persistence.


### Stage 7 reservation-expiry follow-up

The remaining review finding is fixed: leader publication-storage reservations now hold an operation-owned file lock from admission through durable execution binding. Normal maintenance acquires the same lock and reloads state before reclaiming an expired reservation. Live queueing, preparation, and Busy restore waits retain their storage even after the initial five-minute deadline. Task cancellation or process exit releases the owner; expired unbound reservations remain reclaimable after restart. Remote transfer-lease renewal and all existing bounded waits remain unchanged. No protocol or schema change is required.

Permanent regression coverage:

- `live_busy_restore_keeps_expired_canonical_reservation_through_binding`: the production host returns Busy while its original instance owner remains active; normal lifecycle maintenance runs after the persisted leader deadline expires, preserves the reservation, and the same restore retries successfully and binds storage to execution.
- `abandoned_canonical_operation_expires_after_owner_drop_and_restart`: abort an owning task, preserve the unexpired reservation, then expire it and reopen the leader. Repeated normal maintenance reclaims the abandoned reservation and its retention record while preserving a separately owned expired reservation; dropping that owner allows cleanup too. A canceled reservation cannot be renewed.
- The existing complete restore transport-retry regression now retains the canonical owner throughout transfer.

Acceptance reruns passed:

- Windows contracts/storage/Fleet/Core: **246 passed, 3 ignored**; Sessions all features: **214 passed, 15 ignored**. The focused placement gate passed all **7** non-Docker cases, including both new reservation regressions.
- Native Linux storage/Fleet/Sessions: **390 passed, 18 ignored**, including the new ownership and cleanup cases.
- Optional-feature isolation: Swarms-only Sessions **171 passed, 15 ignored**; disabled Sessions **48 passed**. Disabled Core/Sessions all-target Clippy passed.
- All **5 Stage 7 real Docker cases passed**: execution-domain capacity, retained restore/checkpoint/cleanup, unclaimed remote retention, competing-slot scheduling, and one-slot deputy/worker/resume. The seven Stage 6 Docker cases were not repeated for this host-only reservation fix; their previous results remain recorded above.
- Windows and Linux Sessions all-target Clippy, Windows workspace all-feature/all-target checking, formatting, diff checks, and inspection of changed code/tests/documentation for machine-local or live research data passed.

The existing verified AMD64 portable runtime was reused. Docker coverage still uses separate owning-host stores on one physical daemon; native Arm64, macOS Docker, and Hyper-V remain outside the validated boundary. No live provider or research target was used. Stage 8 remains unstarted.

### Stage 7 lock-contention follow-up

The remaining review finding is fixed: canonical reservation admission retries maintenance/live-owner contention every 50 ms within the existing five-minute admission budget. Timeout or a stop request while waiting returns pending without canceling or blocking another placement's work. Placement propagates that pending outcome instead of entering failure cleanup. After acquiring the lock, admission reloads the reservation and defers if its previous owner has already bound execution. Releasing an unbound owner permits reuse of the original reservation identity. No protocol or schema change is required.

Permanent regression coverage:

- `canonical_admission_retries_maintenance_contention_with_same_reservation`: a deterministic short lock hold forces admission to wait and then succeed; repeated admissions also race the real normal reconciliation function. The allocation remains unchanged, with one reservation and no cancellation.
- `canonical_live_owner_contention_is_bounded_and_never_cancels_or_rebinds`: bounded contention and a stop arriving during the wait preserve the original owner's allocation. Releasing an unbound owner permits same-identity reuse; binding execution before releasing the lock makes a waiting contender defer without cancellation, rebinding, or duplicate allocation.
- Existing live-recovery and abandoned-operation expiry regressions retain their ownership guards under the pending-aware admission result.

Acceptance reruns passed:

- Windows contracts/storage/Fleet/Core: **246 passed, 3 ignored**; Sessions all features: **216 passed, 15 ignored**, including both final contention regressions and the preceding expiry/recovery cases.
- Native Linux storage/Fleet/Sessions: **392 passed, 18 ignored**.
- Optional-feature isolation: Swarms-only Sessions **173 passed, 15 ignored** on the full rerun; disabled Sessions **48 passed**. Disabled Core/Sessions all-target Clippy passed.
- The initial concurrent Swarms-only run had one failure in the existing `worker_spawn_receipt_failure_rolls_back_assignment_and_both_dispatch_receipts` test (two agents instead of three). It passed in isolation and on the full rerun without source changes. Its spawn path uses native resource admission rather than the changed canonical reservation path; the cause of that initial failure was not established.
- All **5 Stage 7 real Docker cases passed**: execution-domain capacity, retained restore/checkpoint/cleanup, unclaimed remote retention, competing-slot scheduling, and one-slot deputy/worker/resume. The seven Stage 6 Docker cases were not repeated for this host-only contention fix.
- Windows and Linux Sessions all-target Clippy, Windows workspace all-feature/all-target checking, formatting, diff checks, and inspection of changed source/tests/documentation for machine-local or live research data passed.

The existing verified AMD64 guest runtime was reused. The previously documented single-physical-daemon, platform, and synthetic-provider validation limits remain unchanged. No Stage 8 work is included.

## Stage 8: Provider accounting and best-effort pacing

Enforce pacing at the leader's inference router.

### Implementation

- Configure per-scan targets per provider/account shared by leaders, deputies, workers, and attributable supporting calls, including auto-review.
- Coordinate account usage across participating scans so each does not independently assume the full available allowance.
- Express subscription targets as percentage points of a named allowance window per interval, not a percentage of the remaining allowance.
- Express API targets as currency or provider credits per interval.
- Estimate spend from available usage/pricing when actual spend is unavailable and label estimates explicitly.
- Delay new inference calls when ahead of target while allowing active calls and tools to finish.
- Handle resets, stale data, external account usage, and session warnings.
- Continue on a best-effort basis when measurements are missing. Missing pricing is unknown, never zero.
- Keep measurement and pacing host-owned so they continue with Desktop closed.

Coordination can account for participating Beale scans; unrelated account activity remains observable only through provider measurements and must not be presented as precisely attributable. Pacing is not a guaranteed spending cap.

### Account identity and accounting ownership

Use the Stage 1 opaque account-budget group to distinguish billing/allowance ownership from provider, model, API key, or OAuth token. Map a provider's stable account/organization identity to that internal group where reliable identity is available. Credential refresh does not create a new group. If the provider cannot establish shared account identity, require an explicit host-side grouping to coordinate accounts across hosts; otherwise use a visibly limited local group and warn that cross-host accounting is unavailable. Never infer identity by sharing or logging credentials, and never merge all accounts for a provider into one bucket.

Each coordinated group has one explicitly designated trusted Fleet app-server as its accounting authority, a persisted owner generation, and registered participating leaders. All participating scans obtain pacing admission/reservations there and report usage with stable event IDs. Keep credentials and actual provider inference on the scan leader as already agreed; the accounting authority stores only the minimal accounting/coordination metadata needed for pacing, not replicated research records or provider credentials.

The authority respects each scan's target and reconciles participating usage against observed account allowances. A group's ownership change requires an explicit handover/reconciliation with a new generation; partitioned peers do not independently elect competing authorities. Host-side configuration exposes grouping and authority state without exposing account identifiers to models.

When the authority is temporarily unreachable, retain queued usage events and the last acknowledged rate allocation, apply conservative local pacing, and surface degraded coordination. A participant with no acknowledged allocation uses its scan target with an explicit warning that shared-account coordination is unavailable. This remains best-effort continuation, not a claim that disconnected leaders enforce a combined cap. Reconnect deduplicates events and reconciles outstanding reservations before returning to coordinated pacing.

### Admission, accounting, and timeout boundaries

Distinguish waiting for pacing admission from an in-flight provider call. Start provider response timeouts only at actual dispatch; pacing waits remain cancelable and do not exhaust provider retry budgets or count as failed agent progress. Record every physical provider attempt under a unique attempt ID and account for known usage even if its research result was lost. Redelivery of the same usage event is deduplicated; a genuine retry is a new attempt and may incur new cost. Unknown charges remain uncertain instead of becoming zero.

Heartbeats, cancellations, result acknowledgements, and artifact recovery do not require inference admission. Supporting inference, including auto-review, remains counted but receives fair scheduling so a waiting research request cannot monopolize the budget needed to finish another call's review. Dispatch must not hold orchestration or storage locks while waiting for admission. Persist scheduler state so restarts preserve accounting and do not replay an unbounded backlog as a burst.

### Acceptance tests

- Fake-clock tests verify pacing, allowance resets, and delayed admission.
- Mixed-provider scans use separate accounting.
- Multiple scans share account information without multiplying allowances.
- Supporting calls are counted once.
- Missing pricing, stale usage, and unavailable probes generate warnings without false guarantees.
- Pacing does not terminate in-flight work or deadlock coordination/recovery.
- Restart preserves sufficient accounting state to avoid an unrestricted burst.
- Two distinct accounts on one provider remain separate; credential refresh preserves the same account-budget group.
- Two leaders sharing an account coordinate through one authority; an ownership-generation change rejects stale-owner admissions.
- Missing stable account identity produces explicit limited-accounting state instead of silently merging or treating linked accounts as independent full allowances.
- Authority loss continues under documented degraded pacing with a warning; reconnect deduplicates queued events and reconciles reservations without double-charging.
- Duplicate usage-event delivery is counted once, while genuine retried provider attempts are separately accounted for and unknown costs remain uncertain.
- A long pacing wait does not trigger a provider timeout, retry, agent failure, or unnecessary instance replacement; cancellation remains responsive.
- Supporting review calls progress without holding shared locks, while heartbeats and completion/publication acknowledgements remain live independently of inference admission.

### Stage 8 progress and validation (2026-11-26)

Implemented and validated within the boundaries below. Stage 9 is not started.

- Protocol 2.43 adds typed host accounting requests, saved per-scan pacing targets, and durable session warnings. Provider/account bindings use stable host-only identity where available, explicit grouping where configured, and a visibly limited local group otherwise.
- A host-owned accounting authority persists per-scan and aggregate debt, alternating research/supporting admission, physical-attempt usage, uncertain charges, and generation-fenced handover. Every participant must drain its local outbox before the authority can freeze/export a reconciled generation.
- Leader, routed worker, auto-review/Guardian, and attributable memory inference use the same physical-dispatch boundary. Admission waits precede response timeouts; cancellation and worker heartbeats remain independent. Plugin/runbook review waits precede shared mutation locks.
- Remote-authority loss retains usage and acknowledged allocations. Background maintenance reconciles events and reservations without re-dispatching inference. Restart preserves debt and marks interrupted attempts uncertain.
- Subscription probes retain named-window basis points and reset observations. Configured model prices produce labeled currency/credit estimates; missing prices and measurements remain unknown with warnings.

Implementation defaults: the aggregate group ceiling is the largest active participating scan rate, while each scan retains its own ceiling. Acknowledged disconnected allocations divide that rate among participating scans and never exceed the current scan target. Unknown initial per-call cost uses one sixtieth of the target, rounded up to one unit; later known usage reconciles the debt. Pending admission tickets refresh while waiting and expire after 60 seconds without refresh. Provider observations become stale after 60 seconds; probes are attempted at most every 30 seconds per local group. Accounting outboxes retry through five-second host maintenance. These are conservative best-effort heuristics, not spending guarantees.

Permanent acceptance coverage:

| Acceptance boundary | Regression evidence |
| --- | --- |
| Clock, reset, debt, and fairness | Storage `provider_accounting`: delayed admission, named-window reset/external usage, restart debt, multiple scans/leaders, supporting-call alternation, cancellation and abandoned tickets. |
| Provider/account separation and identity | Separate providers and stable account IDs; refresh preserves grouping; explicit grouping overrides limited local identity; stale handover generations are rejected. |
| Charges and unavailable measurements | Pricing/cache estimates, wrong-unit/missing-price unknown charges, stale observations, distinct physical retries, duplicate-event deduplication, interrupted streamed usage, and restart uncertainty. |
| Authority outage and recovery | Two-host HTTP fixtures with and without acknowledged allocations, conservative local settlement, dropped response bodies, normal background-maintenance reconciliation, and coordinated admission after reconnect. |
| Explicit handover | All registered participants drain, lost admission acknowledgements remain uncertain, frozen snapshots preserve debt, malformed transfers are rejected, and transfer history is bounded. |
| Dispatch and supporting work | Fake 90-second admission wait outside a 45-second review response timeout; cancellation before dispatch; active calls finish; concurrent host operations; background-session attribution. |
| Session and worker boundaries | Pacing-target protobuf validation/round-trip; ordinary local mode rejects Swarm pacing; inner plugin/runbook review cancellation; independent worker heartbeat and transport cancellation; existing publication and recovery suites. |

Acceptance results:

- Windows affected-crate suite: **521 passed, 18 ignored, zero failures**. Linux/WSL affected-crate suite: **514 passed, 18 ignored, zero failures**. Both run `cargo test -p beale-contracts -p beale-storage -p beale-providers -p beale-safety -p beale-sessions -p beale-core -p beale-memories --all-features -- --test-threads=4`; Linux uses `scripts/test-runtime-linux.sh` with isolated home and provider CLIs blocked. Ignored fixtures retain their existing opt-in/subprocess boundaries.
- Focused accounting coverage includes **12 storage/scheduler tests and 11 provider/authority tests**, plus session contract, review cancellation, and transport tests in those broader gates.
- Swarms-only Sessions: **176 passed, 15 ignored**. No-feature Sessions: **49 passed**. No-feature Core/Sessions all-target Clippy with warnings denied passed.
- `cargo check --workspace --all-features --all-targets`, affected-crate all-feature/all-target Clippy with `-D warnings`, and `cargo fmt --all -- --check` passed. The final test-fixture lint correction was followed by another **11/11 provider pacing tests on both Windows and Linux**. Changed source, fixtures, and documentation were inspected for private research material and machine-local data; no such material was introduced.

An intermediate concurrent Windows run failed the existing provider-recovery fixture's accepted-spawn assertion. Its isolated retry, the Swarms-only suite, and the final full gate passed; no production recovery workaround was added. The earlier Windows future-stack overflow was corrected by boxing provider dispatch/admission scopes, and the existing full hierarchy execution/resume regression passes without increasing its stack limit.

Validation boundaries: tests use synthetic providers, accounts, workspaces, and loopback peers. No live provider or research target is used. Provider CLIs do not expose every internal upstream attempt or usage measurement; Beale accounts for host invocations and warns about uncertain usage. Status reports the latest admission coordination mode, while session warnings retain earlier incidents. Status/handover includes the latest 128 reservations and an omitted-history count; complete earlier history stays on the former authority, while scheduling debt and per-scan deadlines transfer. Docker/VM execution and macOS are not revalidated by this stage; the Stage 6/7 isolation and platform limits remain unchanged. Stage 9 UI controls are not implemented.


### Stage 8 review fixes (2026-11-26)

All four findings are fixed with permanent regression coverage, and the acceptance gates below pass. Stage 9 remains unstarted.

- Aggregate and per-scan settlement now use separate rates. Each granted reservation retains its aggregate target for delayed/retried settlement; unpaced scans sharing a paced account contribute their known usage to that account's debt. External subscription-usage observations also settle against the active aggregate rate when scan targets differ.
- Outbox persistence and known degraded-local settlement commit atomically. Cancellation, response timeout, and restart recovery retain streamed charges and update local debt before another admission; maintenance retries remain idempotent while the authority is unavailable.
- Cancellation persists a tombstone before acknowledgement even when admission has not arrived. Duplicate cancellation is safe, changed identities are rejected, and delayed admission cannot recreate the reservation or obstruct drainage.
- Handover imports retain local model prices and merge locally held closed history omitted from the bounded transfer. Tests exercise two hosts, distinct prices, more than 128 events, ownership changes in both directions, stale-owner rejection, and preserved debt/omitted-history counts.

Protocol 2.44 adds the reservation's aggregate target to the typed accounting transfer. Rebuild participating hosts together. No SQL schema migration or Stage 9 controls are introduced.

Validation results:

- Added 13 permanent regression tests (11 storage and two provider tests); the accounting suites now contain 23 storage and 13 provider tests. Coverage includes delayed and duplicate settlement, target expiry, unpaced reconciliation, atomic outbox rollback/retry, cancellation before admission, timeout and cancellation after streamed usage, restart/background maintenance with the authority unavailable, external allowance observations, and two-host handover in both directions.
- Full all-feature acceptance suites for contracts, storage, providers, safety, sessions, core, and memories: Windows **534 passed, zero failed, 18 ignored**; Linux/WSL **527 passed, zero failed, 18 ignored**. Existing opt-in/subprocess ignores remain unchanged.
- Sessions with only Swarms enabled: **176 passed, zero failed, 15 ignored**. Sessions without default features: **49 passed, zero failed**.
- `cargo fmt --all -- --check`, `cargo check --workspace --all-features --all-targets`, affected-crate all-feature/all-target Clippy, and core/sessions no-default-feature Clippy passed with warnings denied.
- Changed source, fixtures, tests, and documentation were checked for live research material, credentials, and machine-local data; fixtures remain synthetic. Targeted whitespace checks passed.

Validation uses synthetic providers/accounts and loopback peers, including normal background maintenance. No live provider, real research target, Docker/VM execution, or macOS validation was added; earlier platform and isolation validation boundaries remain unchanged. Stage 9 has not started.

### Stage 8 follow-up fixes (2026-11-26)

All three remaining accounting findings are fixed with permanent regression coverage. The acceptance gates below pass; Stage 9 remains unstarted.

- Reconciliation initializes missing or expired per-scan deadlines at reconciliation time while retaining future debt. Already admitted attempts and duplicate usage deliveries keep their original deadlines. The regression covers missing, expired, and future deadlines with both absent and queued reservations, reopen/replay, and admission exactly at the resulting deadline.
- Unpaced calls validate model prices against the acknowledged shared-account allocation. Both streamed and final usage retain unknown cost for mismatched currency/credit units or subscription windows. Provider regressions exercise coordinated and disconnected execution, completion and cancellation, wrong units, matching currency/credit prices, and the resulting account/local debt.
- Admission and reconciliation use the same group-unit validation before ledger mutation. Regression cases reject currency, credit, and subscription mismatches without changing persisted state, then verify ordinary admission and a corrected retry still succeed. Cancellation tombstones fence rejected requests without contaminating the account's units.

Four permanent regression tests were added (two storage and two provider tests with parameterized cases). No protocol or SQL schema change is required; protocol remains 2.44.

Validation results:

- Focused accounting suites: **25 storage tests and 15 provider tests passed**. All four new regression tests also passed in the final full Windows and Linux gates.
- Final all-feature contracts/storage/providers/safety/sessions/core/memories acceptance: Windows **538 passed, zero failed, 18 ignored**; Linux/WSL **531 passed, zero failed, 18 ignored**. Existing opt-in/subprocess ignores are unchanged. Linux uses the isolated-home wrapper with provider CLIs disabled.
- Optional-feature isolation: Swarms-only Sessions **176 passed, zero failed, 15 ignored**; no-default-feature Sessions **49 passed, zero failed**. No-default-feature Core/Sessions all-target Clippy passed with warnings denied.
- Workspace all-feature/all-target checking, affected-crate all-feature/all-target Clippy with warnings denied, and `cargo fmt --all -- --check` passed. Storage/provider Clippy was repeated after the final regression assertions.
- Changed source, tests, and documentation were inspected for live research material, credentials, and machine-local data; fixtures remain synthetic. Targeted whitespace checks passed. README and changelog describe the corrected behavior.

Validation uses synthetic providers/accounts, fake clocks, and loopback peers. No live provider or research target was used. Docker/VM execution and macOS were not revalidated; prior platform and isolation boundaries remain unchanged. Stage 9 has not started.

### Stage 8 first-use authority-outage fix (2026-11-27)

Implemented the conservative unknown-cost option for the final review finding. Without either a scan target or an acknowledged allocation, a local model price does not establish the shared account's units. Completed and streamed/interrupted usage retains token counts, unknown cost, and uncertainty. Restart and background reconciliation preserve those values and apply the existing unknown-cost heuristic once. Subsequent calls can produce estimates after receiving an allocation whose units match their prices. Historical unknown events are not retroactively repriced. No protocol or database migration is required; protocol remains 2.44.

Permanent coverage adds two provider regressions for completed and interrupted/restarted first-use calls. Both exercise a real synthetic HTTP authority, normal background-maintenance reconnect, duplicate delivery, exact conservative debt, preserved token counts, and subsequent coordinated admission. Cases cover currency mismatch, credit mismatch, subscription units, and matching currency becoming usable after allocation. The pricing unit test now explicitly rejects absent account units; existing known-charge retry/stream tests establish a matching target.

Validation results:

- Focused provider accounting: **17 passed**, including both new reconnect regressions. The full gates retain all **25 storage accounting regressions**.
- All-feature contracts/storage/providers/safety/sessions/core/memories acceptance: Windows **540 passed, zero failed, 18 ignored**; Linux/WSL **533 passed, zero failed, 18 ignored**. Both new tests passed on both platforms. Existing opt-in/subprocess ignores remain unchanged; Linux uses the isolated-home wrapper with provider CLIs disabled.
- Optional-feature isolation: Swarms-only Sessions **176 passed, zero failed, 15 ignored**; no-default-feature Sessions **49 passed, zero failed**. No-default-feature Core/Sessions all-target Clippy passed with warnings denied.
- Workspace all-feature/all-target checking, affected-crate all-feature/all-target Clippy with warnings denied, and formatting passed. Changed source, tests, README, changelog, and plan were inspected for private research or machine-local data; fixtures remain synthetic and targeted whitespace checks passed.

No live provider or research target was used. Docker/VM execution and macOS were not revalidated; the earlier platform and isolation boundaries remain unchanged. Stage 9 has not started.

### Stage 8 shared-allowance admission fix (2026-11-27)

New unpaced research and supporting calls now honor fresh exhausted-window observations for their shared account. Admission resolves the window from the request, active aggregate target, or retained non-canceled accounting history, so expiration of the active rate lease does not bypass a still-fresh observation. Existing inference may finish; queued calls remain uncharged until admission. The existing freshness threshold, best-effort warnings, named-window matching, and minimum retry interval are unchanged. No protocol or database migration is required; protocol remains 2.44.

Three permanent storage regressions cover research and supporting calls waiting until reset, active and expired rate leases, reopening while queued, admission exactly at reset, idempotent grant retries, completion of existing inference, and stale/unrelated observations retaining their existing behavior.

Validation results:

- Focused storage accounting: **28 passed**, including all three new regressions. The full gates also retain all **17 provider accounting regressions**, including first-use outage recovery.
- All-feature contracts/storage/providers/safety/sessions/core/memories acceptance: Windows **543 passed, zero failed, 18 ignored**; Linux/WSL **536 passed, zero failed, 18 ignored**. All three new tests passed on both platforms. Existing opt-in/subprocess ignores remain unchanged; Linux uses the isolated-home wrapper with provider CLIs disabled.
- Optional-feature isolation: Swarms-only Sessions **176 passed, zero failed, 15 ignored**; no-default-feature Sessions **49 passed, zero failed**. No-default-feature Core/Sessions all-target Clippy passed with warnings denied.
- Workspace all-feature/all-target checking, affected-crate all-feature/all-target Clippy with warnings denied, and formatting passed. Changed source, tests, README, changelog, and plan were inspected for private research and machine-local data; fixtures remain synthetic and targeted whitespace checks passed.

No live provider or research target was used. Docker/VM execution and macOS were not revalidated; earlier platform and isolation boundaries remain unchanged. Stage 9 has not started.

### Stage 8 handover identity fix (2026-11-27)

Protocol 2.45 adds a durable, typed account denomination to the accounting snapshot: currency, credits, or a named subscription window. Accepted admissions (including queued calls) and reconciliation establish it; cancellation receipts alone do not. Unit validation and shared-allowance admission use this field independently of the bounded reservation list. Handover retains it even after every unit-bearing reservation is omitted, validates its syntax and agreement with retained history, and rejects attempts to erase or change an importing host's established unit. The field stores no rate and does not reactivate expired pacing allocations. Rebuild participating hosts together; no SQL schema migration is required.

Four permanent storage regressions cover unpaced research and supporting calls across a JSON handover with more than 128 reservations, a fresh exhausted observation, a new authority with no old history, reopen, generation fencing, waiting until reset, and idempotent admission. Additional cases cover currency/credit/window identity validation, rejection without state mutation, conflicting admission/reconciliation after history is omitted, cancellation receipts, and identity established by a queued first call.

Validation results:

- Focused storage accounting: **32 passed**, including the four new regression tests. The full gates retain all **17 provider accounting regressions**, including first-use outage recovery.
- All-feature contracts/storage/providers/safety/sessions/core/memories acceptance: Windows **547 passed, zero failed, 18 ignored**; Linux/WSL **540 passed, zero failed, 18 ignored**. Both unpaced handover regressions and identity-validation cases passed on both platforms. Existing opt-in/subprocess ignores remain unchanged; Linux uses the isolated-home wrapper with provider CLIs disabled.
- Optional-feature isolation: Swarms-only Sessions **176 passed, zero failed, 15 ignored**; no-default-feature Sessions **49 passed, zero failed**. No-default-feature Core/Sessions all-target Clippy passed with warnings denied.
- Workspace all-feature/all-target checking, affected-crate all-feature/all-target Clippy with warnings denied, and formatting passed. Changed contracts, source, tests, README, changelog, and plan were inspected for private research and machine-local data; fixtures remain synthetic and targeted whitespace checks passed.

No live provider or research target was used. Docker/VM execution and macOS were not revalidated; earlier platform and isolation boundaries remain unchanged. Participating hosts must be rebuilt together for protocol 2.45. Stage 9 has not started.

## Stage 9: Product UI and research-tool preferences

Expose implemented behavior without requiring users to understand runtime internals.

### Implementation

- Rename Graphs to Swarms throughout feature selection, New Scan, settings, and session views.
- Render leader/deputy/worker relationships, descendant status, handoffs, pending work, resumable agents, and resource/pacing warnings.
- Preserve profile model choices and display the new bundled definitions.
- Add the New Scan **VMs / Containers** field, short description, multi-select destination dropdown, and plus button immediately to its left.
- Enable that destination field only for a new scan using Swarms. With No Subagents or Swarms disabled, show local-only execution and disable the destination selector. Switching a draft to No Subagents visibly clears its destinations and explains that the new scan will run locally; switching back does not silently restore them.
- Validate saved drafts and submitted requests before execution. A saved No Subagents draft with destinations must show a validation error and require correction; never silently discard the selection while launching. If Swarms is disabled while a new swarm draft is open, fail new-scan submission with a clear message rather than changing the draft's mode and running locally.
- Label the Swarms feature toggle as applying to new scans. Existing running/paused scans display their retained swarm configuration, remain resumable with that configuration, and keep the ordinary End scan control for explicitly stopping their hierarchy. A new scan copied from an existing swarm still checks current new-scan eligibility.
- Add subscription and API pacing inputs with provider/account and unit context.
- Rename Fleet's Virtual Machines view to **VMs / Containers**.
- Use tabs **All VMs / Containers**, **VMs**, and **Containers**.
- Place **Add Container** to the right of the tab group using existing Add Finding/Add Surface styling.
- Accept an image name without an instance-count field. Explain that more instances may be allocated as research requires.
- Distinguish saved container configurations from allocated instances and expose them through connected Fleet hosts.
- Use an appropriately licensed Docker icon from an MIT-licensed icon source and a package icon instead of the VM tag for containers; verify asset licensing and required attribution.
- Display storage use, retention deadlines, preparation failures, and protected environments.
- Replace the two research-tool allowance fields with mutually exclusive **Allow**, **Recommend**, and **Priority** choices applying locally and remotely. These provide guidance, not a new execution permission system.
- Interpret Allow as permission guidance to install/use, Recommend as preferred but optional, and Priority as the top choice for a relevant task.
- Apply platform relevance using the actual execution environment rather than only the leader's OS.
- Show the active leader-only rule and its swarm-definition revision, explain why global toggles do not change it, and direct instruction edits to a new definition revision for future scans.
- Show waiting/suspended agents, queued wake requests, termination-unconfirmed executions, and outcome-unknown commands without labeling them as ordinary completed work.
- Expose host-side account grouping/accounting-authority configuration and degraded pacing state, without displaying raw credentials or putting account identifiers in model context.
- Make Delete now blockers and explicit pending-output discard clear; show deletion-in-progress when a cleanup claim has already won against resume or Keep.

### Acceptance tests

- New Scan correctly submits multiple destinations and both pacing modes.
- New scans created with Swarms disabled or No Subagents preserve ordinary local research behavior; existing swarm scans keep their saved execution mode.
- Container registration has no instance-count requirement.
- Fleet filters distinguish configurations, instances, and future VM entries correctly.
- Preference changes produce consistent local and remote guidance.
- Reconnected Desktop reconstructs current hierarchy and warnings.
- Layout, keyboard interaction, empty states, long image names, and failure states receive visual verification.
- Scoped leader rules are absent in No Subagents and cannot be inadvertently disabled by unrelated global-rule controls; definition edits do not alter active snapshots.
- Suspended/waking, termination-unconfirmed, unknown command outcome, and degraded-accounting states survive a Desktop reconnect.
- Resume/Keep versus cleanup displays the winning lifecycle state; explicit pending-output discard describes exactly what will be lost and leaves published evidence intact.
- No Subagents with selected destinations cannot be submitted; mode changes visibly clear the draft selection, and invalid saved drafts block Start until corrected.
- Disable Swarms after destination selection but before submission: reject the new swarm request without starting local or remote execution. Switching intentionally to No Subagents presents local-only execution before Start.
- Disable Swarms with active children, then with a suspended deputy: existing scans retain the leader rule, handoffs, and destinations, and continue/resume without mode conversion. A newly created/copied scan obeys the disabled setting.
- Loading an existing swarm into a build lacking support shows an unsupported-configuration error, not a local single-agent fallback.
- Queued spawn status shows that an assignment exists but an instance has not started; release failure or allocation exhaustion displays the retained blocked/failed result.

- Stop during delayed peer discovery/authentication prevents submission; cancellation during POST is replayed after host acceptance.
- Switching provider/model or host invalidates account resolution for binding and pacing actions; old asynchronous responses cannot overwrite the current selection.
- While the first provider call is blocked, hierarchy/wake changes and pacing warnings reach Desktop before completion without clearing live output or pending prompts. Malformed status frames and stale session/generation updates are rejected.
- A rejected retention mutation or lost response reloads authoritative inventory and shows the cleanup winner. Older refreshes cannot replace that result; reload failures preserve the mutation error separately.
- Native-only allocation inventory leaves the Containers tab empty, while All VMs / Containers retains native lifecycle controls and an explicit no-isolation label.

### Progress — implemented and validated

Stage 9 exposes the earlier runtime stages in Desktop. Stage 10 remains unstarted.

- New Scan retains profile/model and bundled-definition selection and adds multiple saved image destinations, image registration, and API currency/provider-credit/subscription pacing. Client validation and host admission both reject invalid local-only drafts and disabled new Swarms; explicit No Subagents selection clears destinations/pacing visibly. Existing scan submissions keep using their host-owned snapshot.
- Fleet now presents All VMs / Containers, VMs, and Containers. Saved configurations are separate from allocations and retained environments, with host identity, preparation failures, host-volume storage use, allocation storage reservations, deadlines, protection blockers, Keep/Delete now, and exact pending-output discard previews. Hyper-V remains discoverable but is not offered as a supported Swarm allocation destination.
- The authenticated inventory and trusted peer-registration route connect these controls to host allocation/accounting. Selected execution hosts and explicit accounting authorities use existing Fleet pairings. Provider credentials never cross these routes. The UI exposes provider/account route resolution, explicit budget groups, authority/participants, limited identity, and degraded coordination. Existing authority replacement still uses the host's drained handover protocol; the UI does not bypass it.
- Session detail includes a typed, host-authored overview derived from durable state. Desktop validates it and shows saved definition revision and leader-only rule, hierarchy, pending assignments/wakes, resumability, and uncertain execution/command states. Uncertain work cannot be presented as ordinary completion. Existing handoff and recovery commentary remain visible after reconnect. Global toggles do not remove saved rules or convert existing scans to local mode.
- Research Tools uses one Allow/Recommend/Priority preference per tool for local and remote guidance. Allow is the default. Catalog settings include all platforms; guidance filters at the actual execution environment. Protocol 2.46 and Fleet schema 3 replace the old allowance fields; existing settings reset to Allow. Participating hosts and Desktop must be rebuilt together.
- The Docker mark is Tabler's MIT-licensed brand asset with its full license included and README attribution; container rows use the package icon. Related READMEs and changelog describe only the implemented Stage 9 behavior.

Acceptance coverage:

| Boundary | Permanent coverage |
| --- | --- |
| Multi-destination and API/subscription draft submission | Desktop `multiple_destinations_and_both_pacing_modes_roundtrip_submission`, decimal-unit validation, and existing host admission/pacing contracts |
| Disabled feature, invalid saved local draft, intentional mode change | Desktop `drafts_reject_invalid_saved_modes_and_feature_disable_without_conversion`, destination interaction tests, and existing hierarchy feature-disable/resume tests |
| Keyboard, narrow layout, long images, empty/failure states | Desktop destination keyboard regression, minimum-window pacing-form traversal, existing title/tab and form layout tests, plus offline dark/light visual fixtures |
| Configurations versus instances; registration without counts | Desktop registration/filter regression; host `client_inventory_reconstructs_allocations_separately_from_configurations`; existing configuration endpoint tests |
| Preference persistence and actual execution OS | Fleet preference migration/reopen and HTTP tests, `preferences_are_mutually_exclusive_and_follow_execution_os`, local session prompt test, and cross-OS managed-worker prompt regression |
| Reconnect, scoped rule, parent/agent status, uncertainty | Extended `local_tree_executes_and_resumes_after_feature_disable` reopens the host snapshot and compares its overview/rule; Desktop status regression and malformed/foreign/duplicate/dangling-parent overview rejection |
| Cleanup winner and explicit output loss | Desktop cleanup-claim/preview regression; existing Stage 7 host retention race, generation-fence, exact-discard, and published-evidence preservation tests |
| Queued/blocked allocation, unsupported build, degraded accounting | Current host-authored statuses in the UI; existing allocation/recovery/accounting cases run in the broad gates; feature-disabled session tests reject unsupported saved Swarms without execution fallback |
| Pairing authorization | `execution_peer_registration_is_host_only_and_never_echoes_pairing_material` verifies host versus worker audience, endpoint validation, persistence, and secret-free replies |

Validation results (synthetic fixtures only):

- `cargo fmt --all -- --check` and workspace/all-target/all-feature check pass.
- Clippy with warnings denied passes for Desktop, contracts, Fleet, and sessions, including all targets/features.
- Desktop acceptance suite: **278 passed**, no failures or ignored tests.
- Windows contracts/storage/providers/safety/sessions/core/memories/Fleet gate: **591 passed, 20 ignored**, no failures.
- Isolated Linux/WSL gate for the same crates: **584 passed, 20 ignored**, no failures.
- Sessions with no default features: **49 passed**. Swarms-only sessions: **176 passed, 15 ignored**. No failures.
- Offline screenshots checked in dark/light at 1440×960 and 840×600, with the full pacing form additionally checked at 1440×1600. Fixtures cover destination selection, local-only mode, pacing, container registration/configurations, retention blockers, uncertain session status, empty Fleet, and tool preferences. Keyboard traversal verifies that the long form scrolls into view. Visual review corrected light-theme icon contrast, narrow guidance wrapping, and empty-state wording.
- The Windows recovery-exhaustion fixture now waits for child startup before injecting renewal failures, eliminating a process-start race while retaining the three-failure bound and independent-branch assertion. Runtime recovery behavior is unchanged. Existing tray/child-process tests require normal Windows desktop access; sandbox restrictions were resolved by rerunning the gate with that access.
- Changed source, fixtures, and documentation were checked for private research material, credentials, and machine-local data; fixtures remain synthetic. `git diff --check` passes.

Validation boundaries: no live provider, real research target, live Docker/VM workload, or macOS run was used for Stage 9. Existing Docker/Hyper-V/Arm64 platform limits remain unchanged. The ignored tests are existing opt-in provider/Docker cases and executable worker fixtures. Screenshots are offline rendering checks, not claims of a live multi-host end-to-end run. Host grouping uses opaque identifiers and existing paired-host configuration; disconnected authorities still produce the Stage 8 best-effort degradation semantics. Storage UI distinguishes measured host-volume usage from per-allocation reservations and does not claim per-container filesystem measurements. Stage 10's integrated validation matrix remains pending.

### Stage 9 review fixes — five findings addressed

- **Cancellation during preflight:** Desktop retains cancellation ownership while discovering peers and authenticating, prevents submission after cancellation, and replays a cancellation received during POST once the host accepts the turn. Permanent regression: `cancellation_during_peer_discovery_prevents_submission_and_covers_post_race` covers delayed discovery, no dispatch after cancellation, and the acceptance race.
- **Stale accounting context:** Resolved bindings are fenced by host, provider/model, and request generation. Save binding and Add pacing require that current resolution; changing a group requires saving/resolving it first. Permanent regression: `resolved_accounts_are_fenced_by_provider_host_and_request_generation` covers provider/host changes, stale responses, current responses, and unsaved groups.
- **Live status:** Protocol 2.47 adds bounded, deduplicated `SwarmUpdated` snapshots during active turns. Hierarchy and accounting warnings arrive before turn completion, including the first turn. Desktop validates these snapshots and preserves live output, pending prompts, and agent selection. Permanent regressions: `live_status_stream_reports_changes_while_provider_is_blocked`, `live_swarm_updates_preserve_output_and_reject_old_sessions_and_generations`, and `live_swarm_status_uses_validated_stream_detail` cover blocked-provider updates, warnings/queued wakes, teardown, client state, and malformed wire data.
- **Retention reconciliation:** Mutations reload host inventory after success or failure, including lost acknowledgements. Refresh generations and queued refreshes prevent older inventory from overwriting the cleanup winner; mutation and refresh errors remain distinct. Permanent regressions: `retention_reloads_cleanup_winner_after_rejection_and_lost_response` races against the durable allocation store and verifies deletion-in-progress controls; `mutation_refresh_is_ordered_and_keeps_both_failure_messages` covers refresh ordering and outages.
- **Native/container distinction:** Native allocations and retained copies remain manageable in All VMs / Containers with an explicit no-OS-isolation label. Containers excludes them and shows its empty state when only native work exists. Permanent regression: `container_filter_excludes_native_allocations_but_all_retains_native_controls` and `native_only_inventory_keeps_container_empty_state` cover native/Docker allocations, retention controls, tab filtering, and the native-only empty state.

Review-fix validation results (synthetic fixtures only):

- Desktop: **286 passed**, no failures or ignored tests; eight new permanent regressions across the five findings.
- Windows contracts/storage/providers/safety/sessions/core/memories/Fleet gate: **592 passed, 20 ignored**, no failures.
- Isolated Linux/WSL gate for the same crates: **585 passed, 20 ignored**, no failures.
- Sessions without default features: **49 passed**. Swarms-only sessions: **177 passed, 15 ignored**. The added host regression passes in both full gates and the Swarms-only configuration.
- Workspace/all-target/all-feature check, formatting check, and all-target/all-feature Clippy for Desktop/contracts/Fleet/sessions with warnings denied pass. Desktop production build passes.
- Offline Fleet and live-status fixtures were rendered and inspected in dark/light at 840×600; warnings, preparation failures, and controls remain readable. Permanent interaction/layout tests cover native filtering, empty state, and retention winner controls.
- Source, test fixtures, and documentation were inspected for private research material, credentials, and machine-local data; new fixtures remain synthetic. `git diff --check` passes.

The existing ignored provider/Docker cases and executable worker fixtures remain opt-in. No live provider, real target, live Docker/VM workload, or macOS validation was performed. Protocol 2.47 requires rebuilding Desktop and participating hosts together; no storage migration was added. Stage 10 remains unstarted.

### Stage 9 second review fixes — three findings addressed

- **Consistent live snapshots:** Live, final Swarm, and reconnect detail projections read a pinned, read-only SQLite WAL snapshot. Its transcript, totals, model breakdown, duration, warnings, and hierarchy share the same database version while writers remain free to commit. Snapshot handles validate existing feature schemas without migrating them; Runbook summary access uses savepoints that compose with the pinned transaction. Permanent coverage: `runbook_summary_reads_compose_with_pinned_host_snapshot`, `read_snapshot_pins_commits_and_cannot_write_or_migrate` and `live_and_reconnected_details_read_consistent_usage_during_commits`.
- **Committed versus in-flight events:** Protocol 2.48 adds live-event turn numbers and the highest committed transcript turn. The leader relay tags text, reasoning, tool, and usage events. Desktop retires only committed events and prompts, preserves a later streaming turn and its timer/selection, separates identical segment indices from different turns, and rejects late events or older committed snapshots. Permanent coverage: `live_swarm_commit_retires_only_committed_turn_events_and_usage`, `committed_and_live_turn_boundaries_roundtrip`, and the extended blocked-provider stream regression, which checks both successive leader turn identities.
- **Stop replay:** A Stop received during POST runs alongside response consumption. An already-finished response (404 from Stop) does not discard the completed frame. Transient Stop failures receive up to three attempts with individual three-second deadlines; exhaustion keeps the stream connected and restores the UI's retry control. Permanent real-HTTP regression: `replayed_stop_preserves_completion_and_retries_without_dropping_stream` covers late Stop, transient recovery, and exhausted automatic retries followed by successful explicit retry.

Validation completed on 2026-11-27:

- Desktop all-features suite: **288 passed**.
- Nine-crate host acceptance gate (`beale-contracts`, `beale-storage`, `beale-providers`, `beale-safety`, `beale-sessions`, `beale-core`, `beale-memories`, `beale-fleet`, and `beale-runbooks`), all features: **Windows 612 passed / 20 ignored; Linux via WSL 604 passed / 20 ignored**. No failures. The existing ignored tests retain their opt-in environment requirements.
- Sessions feature configurations: **49 passed** with no default features; **178 passed / 15 ignored** with only `swarms` enabled.
- Workspace all-target/all-feature check, formatting check, and all-target/all-feature Clippy with warnings denied passed. Clippy covered Desktop, contracts, storage, Fleet, sessions, and Runbooks. `git diff --check` passed.
- Six new permanent tests and the extended blocked-provider regression cover these findings. Changed source, fixtures, and documentation were checked for live research material and machine-local data; fixtures remain synthetic.

Protocol 2.48 requires rebuilding the app-server and Desktop together; these fixes require no database migration. These runs used deterministic providers and synthetic HTTP services, not live providers, Docker/VM execution, or macOS. No new visual capture was required for these state/transport fixes; Desktop rendering and layout regressions passed in its suite. Stage 10 remains unstarted.

## Stage 10: Integrated validation and documentation

Use synthetic workspaces and deterministic provider substitutes for default tests.

### Integrated acceptance scenarios

1. **Ordinary research:** new scans created with Swarms disabled or No Subagents run locally without swarm tools, leader review, or Docker requirements; nonempty destination selections are rejected before execution.
2. **Basic:** two Research Deputies investigate separate synthetic areas, delegate workers, exchange artifacts through parents, and resume a completed worker.
3. **VDH VVS:** a candidate reaches independent Validation, returns for missing evidence, and completes without losing provenance.
4. **OAI 948:** multiple work items progress concurrently through ranking, discovery, validation, reporting, patching, and independent patch checking.
5. **Distributed execution:** Linux containers on connected hosts run their own loops while inference, findings, and durable state remain on the leader.
6. **Failure recovery:** interrupt Desktop, provider responses, worker transport, artifact upload, and the leader process; verify bounded recovery and no duplicate work.
7. **Capacity and retention:** exhaust admission headroom, complete agents, resume them, end scans, advance retention time, and verify protected outputs survive.
8. **Ownership and routing boundaries:** harness-managed workspace paths and artifact routing keep concurrent agents' private copies separate; scoped protocol dispatch prevents control of non-direct descendants and does not distribute host-control/provider credentials. This does not claim that arbitrary local same-user processes cannot access other host paths.
9. **Worker identity:** attempted parent/scan impersonation, host-administration access, and replay from revoked execution attempts fail without changing canonical state.
10. **Constrained capacity:** begin with a deputy occupying the only subordinate slot and no child; accept its first child as queued, persist the response, suspend the parent, and admit the child only after fresh capacity checks. Exercise failed suspension and another scan winning capacity, then verify bounded recovery, fair progress, and no duplicate child or local fallback.
11. **Unknown execution outcome:** partition a running experiment, lose its completion response, and reconnect or request replacement; preserve uncertainty, confirm managed-process termination, and avoid blind command replay.
12. **Cleanup races:** race expiry with resume, Keep, and upload acknowledgement; interrupt deletion and restart the host; preserve active generations and published evidence.
13. **Shared-account pacing:** coordinate two leaders on one account and a separate account on the same provider, refresh credentials, interrupt the accounting authority, and replay usage events; verify separation, warnings, deduplication, and responsive cancellation.
14. **Cross-platform files:** transfer Windows-origin inputs into Linux workers and exercise executable metadata, links, repository worktrees, case collisions, and capture-time mutation; preserve supported semantics and reject unsupported ones explicitly.
15. **Mode and feature settings:** reject No Subagents plus destinations, make draft mode changes visible, and disable Swarms during active/suspended scans. Existing scans continue/resume from their snapshots; new scans obey the toggle and never silently switch execution location.
16. **Storage failure after admission:** inject worker checkpoint, leader artifact-store, and leader database exhaustion independently; preserve committed state and unpublished outputs, emit no false acknowledgement, restore space, and reconcile using original operation IDs. Check unaffected-branch progress and explicit pausing where shared storage is unavailable.

### Verification gates

Run the required Rust checks from `beale-v2/` when implementation begins:

```text
cargo fmt --all -- --check
cargo check -p <affected-crate>
cargo clippy -p <affected-crate> --all-targets -- -D warnings
cargo test -p <affected-crate>
```

Broaden checks for shared-contract changes. Test Swarms-enabled and disabled builds separately; an all-features build is insufficient. Keep isolated contract, persistence, policy, and scheduling tests below the integration boundary. Use hosted integration tests only when real worker, transport, Docker, or session composition is essential. Keep live-provider and real-target tests opt-in.

### Documentation and completion

Update relevant READMEs and root `CHANGELOG.md` under the appropriate Beale and app-server entries as implemented stages change product behavior. Document the fresh v2 contract, Docker compatibility boundaries, resource/pacing limits, retained storage, and the operator-managed execution boundary. Beale remains the trusted harness; containers and VMs must not be described as providing guarantees beyond their actual configuration.

Inspect changed source, fixtures, snapshots, and documentation for live research material and machine-local data before the final test gate. Use only synthetic research fixtures and permitted public integration identifiers.

Implementation is complete when these scenarios pass and documented behavior matches the product, including failure and recovery paths. Stage 10 validation does not certify untested platforms or deployment environments.

### Stage 10 progress — integrated validation and documentation

The initial four acceptance tests comprise three component-composition tests for tool routing, durable hierarchy, private files and workflow transitions, plus ordinary research through the session engine and a synthetic HTTP provider. Component fixtures explicitly dispatch recipients and advance durable handoff status; they do not prove inbox consumption or scheduler delivery. The review fixes below add separate scripted-provider scheduling coverage and real process/storage/container validation.

- `basic_two_deputies_handoff_artifacts_and_resume_completed_worker`: two separate Research Deputies each delegate a worker; publication records the permitted parent handoff path and the fixture explicitly materializes evidence at the recipient; a completed worker resumes with a new execution generation and retained private files, and the leader workspace stays unchanged.
- `vdh_missing_evidence_returns_and_preserves_provenance_after_reopen`: an independent reviewer receives a candidate, returns a missing-evidence request, receives supplemental evidence, and returns an assessment. Reopening canonical storage preserves the original handoff and producer/attempt provenance.
- `oai_concurrent_items_reach_independent_patch_review_without_stage_barrier`: one work item remains blocked while a second advances through ranking, discovery, validation, reporting, patch generation, and independent patch checking. A canonical synthetic finding seeds remediation. Distinct patch and regression-test files are published, materialized by an independent checker, and executed against a synthetic input that fails before patching and passes afterward. Both assessments preserve the exact submitted patch/test IDs. Component assertions store results for Patch Generation and Reporter, then advance the first item. This proves independent work-item transitions and actual patch checking; it is not a single autonomous all-stage provider run.
- `ordinary_scans_remain_local_without_swarm_tools_or_leader_rules`: both disabled Swarms and enabled Swarms with No Subagents run ordinary research without swarm tools or leader rules; destinations are rejected before another provider call.

The Docker constrained-capacity fixtures now honor the documented optional `BEALE_DOCKER_CLI` override and otherwise use `docker`, matching the other container fixtures. The first explicit Docker run exposed an unconditional environment-variable unwrap in these two fixtures, before any worker/provider execution. No production behavior changed.

The root v2, app-server, Desktop, and Swarms READMEs now describe the implemented routing, allocation, pacing, and retention boundaries. Stale claims that these remain later stages were removed. The initial Stage 10 implementation remained on protocol **2.48** without a storage migration or new model-facing contract. The final-review blocker fixes below introduce protocol **2.49** and its handoff tool contract. The subsequent operator-transcript fix adds host schema **2** as recorded below; the handoff wire contract remains 2.49.

### Stage 10 acceptance matrix

Paths below are relative to `beale-v2/`. Host tests reside under `apps/beale-app-server/beale-harness/`; Desktop tests reside under `apps/beale-desktop-app/src/`; feature tests reside under `extensions/features/`.

| Scenario | Permanent acceptance evidence | Exercised boundary and remaining limits |
| --- | --- | --- |
| 1. Ordinary research | New `ordinary_scans_remain_local_without_swarm_tools_or_leader_rules`; `beale-swarms/tests/workflows.rs::local_selection_needs_no_swarm_tables_and_rejects_fleet_destinations` | Real session engine plus synthetic HTTP inference; no Docker or remote fallback. |
| 2. Basic | New `research_workflow_tests.rs::basic_two_deputies_handoff_artifacts_and_resume_completed_worker`; `hierarchy_tests.rs::local_tree_executes_and_resumes_after_feature_disable` | Component workflow plus `hierarchy_workflow_tests.rs::bundled_handoffs_flow_through_parent_turns_scheduler_and_resumed_worker`: actual model turns forward inbox envelopes through parents, schedule the recipient, read accepted bytes and resume the source worker. Scripted providers learn IDs only from their tool replies/inboxes. |
| 3. VDH VVS | New `vdh_missing_evidence_returns_and_preserves_provenance_after_reopen`; `workflows.rs::all_forward_return_and_independent_patch_routes_are_durable_and_parent_mediated` | Component coverage exercises independent identities, missing-evidence return and immutable provenance across reopen. The new bundled scheduler regression separately exercises VDH candidate delivery through source deputy, leader, destination deputy and scheduled independent worker; the complete missing-evidence cycle is now also exercised by `vdh_missing_evidence_cycle_runs_through_scheduler_with_provenance`, using scheduled independent reviewers and canonical handoff lifecycle updates. |
| 4. OAI 948 | New `oai_concurrent_items_reach_independent_patch_review_without_stage_barrier`; `workers_inherit_saved_deputy_stage_on_execution_and_resume` | Component coverage advances pending items, creates a canonical finding, executes a real patch/test pair and preserves exact artifact identity in independent assessment. The new bundled scheduler regression exercises OAI discovery-to-validation inbox delivery. `oai_concurrent_full_cycles_run_through_scheduler_and_independent_patch_checks` now schedules both work items through all stages, including canonical findings and actual independent patch/test execution. UI-driven distributed validation remains unavailable. |
| 5. Distributed execution | `docker_alpine_and_ubuntu_research_artifacts_metadata_and_retained_reuse`; `peer_host_runs_assigned_worker_once_and_preserves_leader_owned_results`; `worker_binary_uses_leader_inference_and_private_spool_without_guest_host_services` | Real AMD64 Linux containers/native workers and authenticated synthetic leader/peer transport; separate logical hosts on one test machine, not separate physical Tailscale hosts. |
| 6. Failure recovery | `client_disconnect_during_managed_research_preserves_completion_on_reconnect`; `provider_recovery_preserves_exchange_and_deduplicates_tool_replies`; `peer_snapshot_transfer_recovers_partition_and_rejects_foreign_or_revoked_reads`; `coordinator_restart_respects_live_owner_and_atomically_stops_lost_hierarchy`; `abandoned_inference_recovers_after_process_death_without_redispatch_and_warns_once`; Docker cancellation/recovery tests | Disconnects, bounded provider retry, transport faults, process death, durable restart reconstruction and background maintenance. `leader_restart.rs` now terminates the actual leader during an active hierarchy and verifies normal startup maintenance records recovery once without provider redispatch. Its opt-in Windows variant also kills/restarts Desktop and retrieves the recovered session over HTTP. Visual reconnect confirmation is separately blocked by the computer-use runtime. |
| 7. Capacity and retention | `docker_retained_restore_checkpoint_protection_and_claimed_cleanup`; `docker_unclaimed_remote_allocation_receives_scan_end_and_expires`; `fleet_allocation.rs` retention/reservation tests | Real retained container/volume restore and cleanup plus deterministic RAM/storage/time pressure. Admission uses deterministic resource pressure. A separate disposable tmpfs test exercises actual filesystem exhaustion; hardware-memory exhaustion is not tested. |
| 8. Ownership/routing | Basic composition test; `native_tools_route_to_private_copies_and_materialize_only_explicitly`; `direct_child_authority_is_enforced_in_transactions`; `worker_gateway_rejects_identity_escalation_host_control_and_canceled_reconnect` | Separate private copies, parent-scoped dispatch, centrally accepted evidence and credential boundaries; no same-user OS isolation claim. |
| 9. Worker identity | `authoritative_dispatch_rejects_impersonation_expiry_revocation_and_bypassed_parent`; `worker_gateway_rejects_expired_and_revoked_credentials_without_branch_stop`; `host_endpoint_rejects_worker_authority_and_unbounded_or_rebound_requests` | Transactional storage and real HTTP gateway rejection without canonical mutation. |
| 10. Constrained capacity | `docker_one_slot_deputy_yields_worker_completes_and_parent_resumes`; `docker_accepted_child_queues_when_competitor_takes_yielded_slot`; `failed_parent_checkpoint_blocks_even_when_ram_has_become_free` | Real Docker parent yield/child/resume and competing scan, with injected one-slot capacity and failed-checkpoint coverage. |
| 11. Unknown outcome | `running_command_partition_requires_termination_and_parent_reconciliation`; `assigned_loop_retains_uncertain_effect_after_outcome_write_failure_without_reexecution`; `peer_supervisor_retains_uncertain_execution_and_never_relaunches` | Real managed commands with transport/checkpoint faults, confirmed termination and no blind effect replay. |
| 12. Cleanup races | `concurrent_keep_or_resume_and_cleanup_have_one_winner`; `cleanup_and_keep_race_has_one_winner_and_deletion_resumes_exact_generation`; `failed_publication_and_execution_protections_cannot_be_cleared_by_generic_release`; Docker retained cleanup test | Independent DB connections, cleanup claim/reopen, generation fencing and actual Docker lifecycle; published artifacts stay outside deleted private environments. |
| 13. Shared-account pacing | `two_hosts_share_http_authority_and_reconnect_flushes_degraded_attempts`; `two_scans_and_leaders_share_allowance_and_supporting_calls_alternate`; first-use outage, allowance-reset, and bounded-handover regressions in `beale-providers`/`beale-storage` | Two logical HTTP hosts, stable identity across refresh, distinct accounts, fake-clock debt/reset, outages, deduplicated final/interrupted usage and cancellation. No real provider-account quota measurement. |
| 14. Cross-platform files | `windows_git_link_and_executable_metadata_survive_simulated_linux_transfer`; `linked_git_worktrees_become_self_contained_without_host_pointers`; `changing_sources_retry_boundedly_and_only_seal_verified_inputs`; Docker metadata test | `docker_windows_git_worktree_reaches_linux_worker_with_links_and_history` now captures an actual Windows linked worktree, transfers it through the scoped gateway into a real Linux worker, and verifies links, executable metadata, self-contained history and retained reuse. Collision/mutation rejection remains in focused storage tests. |
| 15. Mode/settings | New ordinary-research test; `local_tree_executes_and_resumes_after_feature_disable`; `drafts_reject_invalid_saved_modes_and_feature_disable_without_conversion`; `unsupported_hierarchy_never_falls_back_to_local_or_rewrites_saved_state` | Host and Desktop boundaries, including isolated compiled-feature configurations. Stage 9 offline visual evidence is retained. Real Desktop process restart now passes; a new interactive Desktop distributed session is blocked by computer-use runtime initialization failure. |
| 16. Storage failure | `checkpoint_exhaustion_preserves_last_durable_state_and_same_retry_identity`; `canonical_findings_and_handoffs_wait_for_artifact_and_record_commits`; `failure_receipt_and_database_failures_remain_unacknowledged_until_recovery`; background checkpoint/publication recovery regressions | Synthetic branch/recovery coverage is supplemented by real SQLite `SQLITE_FULL` at worker checkpoint/outcome and leader record commit, plus actual ENOSPC on a disposable 1 MiB Linux tmpfs at worker checkpoint and leader artifact writes. Original operation IDs recover after capacity restoration; no false acceptance, prior checkpoint loss, or unrelated-branch failure. |

### Stage 10 initial verification results

Validation run on **2026-11-27**, using synthetic workspaces, synthetic credentials, deterministic HTTP providers, isolated Linux configuration, and the rebuilt current-source AMD64 worker:

| Gate | Result |
| --- | --- |
| Windows `cargo test --workspace --all-features` | **1,006 passed; 21 ignored; 0 failed**, including all 288 Desktop tests and the four new acceptance tests |
| Linux `bash scripts/test-runtime-linux.sh test --workspace --exclude beale-desktop-app --all-features` | **722 passed; 22 ignored; 0 failed** |
| Sessions `--no-default-features` | **49 passed; 0 ignored; 0 failed** |
| Sessions `--no-default-features --features swarms` | **182 passed; 15 ignored; 0 failed** |
| Real AMD64 Linux Docker, Windows owning host | **12 cases passed across runs**: 10 on the first run; both constrained-capacity cases passed after correcting their optional-CLI fixture setup. The initial 2 setup failures are not counted as passes |
| Windows opt-in local HTTP WebFetch test | **1 passed**, run separately with normal loopback access |
| Workspace check, all targets/features | Passed |
| Windows workspace Clippy, all targets/features, warnings denied | Passed; affected Sessions Clippy passed again after the Docker fixture correction |
| Linux workspace Clippy excluding Desktop, all targets/features, warnings denied | Passed before the fixture-only optional-CLI correction |
| `beale-core` checks with no defaults and with only `swarms` | Both passed |
| Formatting (including the included workflow-test file), whitespace, source/fixture/documentation audit | Passed; no live research content or machine-local identities added |
| Static AMD64 musl worker build/package | Passed; Docker validation uses this rebuilt bundle, not the earlier-stage worker |

The two constrained-capacity reruns exercised the corrected default with no `BEALE_DOCKER_CLI` override. No production bug was identified by these failures.

These initial workspace runs replace the narrower earlier-stage crate gates for that checkpoint; review-fix validation is recorded separately below. The default ignored counts still include Docker tests run separately, subprocess entry fixtures exercised by their parent tests, and two live-provider tests not run. Windows also ignores the local WebFetch fixture in its default gate; its separate explicit run passed. Linux Desktop rendering was excluded, while Windows Desktop's full suite ran. At this initial checkpoint, no new screenshots or interactive Desktop session were used. The later review-fix process and automation attempts are recorded below.

To repeat the explicit container suite after packaging the current worker, set `BEALE_CONTAINER_RUNTIME` and `BEALE_DOCKER_TEST_GATEWAY` as described in the app-server README, then run:

```text
cargo test -p beale-fleet -p beale-sessions --all-features docker_ -- --ignored --test-threads=1
```

The Docker CLI override is optional. Gate logs are local ignored artifacts under `beale-v2/target/stage10-*.log`; this plan retains the results and test mapping without committing machine-specific logs.

### Stage 10 review fixes and additional validation

Both review findings are addressed:

- **Scheduling and delivery evidence:** `hierarchy_workflow_tests.rs::bundled_handoffs_flow_through_parent_turns_scheduler_and_resumed_worker` runs the real session engine against a scripted HTTP provider for Basic, VDH VVS, and OAI 948. Each source worker publishes evidence to its deputy; the deputy consumes its inbox and forwards an accepted envelope to the leader; the leader forwards it to the destination deputy, which schedules an independent recipient. The recipient materializes and reads the accepted bytes and reports through its parent. A follow-up traverses the same parent hierarchy and resumes the completed source worker with retained files. Scripted provider actions use only model inputs/tool replies, never storage reads or direct fixture delivery. Read-only final assertions check handoff artifact identity, completed agents and the second source-worker attempt. The older workflow tests are explicitly documented as component tests; their direct status transitions do not stand in for scheduling coverage.
- **Real patch evidence:** the OAI component test creates and reads a canonical synthetic finding instead of using a nonexistent identifier. The builder publishes separate patch and test files. An independent checker materializes both, confirms the unpatched input fails, applies the patch, runs the test, and verifies the resulting bytes. Assessments preserve exactly the submitted patch/test artifact IDs and publish the independent checker’s actual before/after output with checker provenance. This exposed omitted handling of `patch_refs` and `test_refs`: host attachment resolution, canonical accepted-artifact validation, and remote worker upload traversal now handle both fields. `patch_and_test_references_publish_validate_and_preserve_exact_identity` covers accepted bytes, producer identity, replay and malformed/missing references. `assigned_worker_publishes_patch_and_test_handoff_references_before_parent_delivery` covers actual assigned-worker inference, scoped HTTP upload, accepted parent handoff, provenance and replay without another provider call.

Additional permanent coverage closes environment-feasible gaps:

- **Real process death:** `beale-core/tests/leader_restart.rs::killed_leader_process_recovers_through_normal_startup_without_provider_redispatch` launches the actual host with an isolated profile and deterministic provider, drops the client connection, kills an active leader process, and restarts the host twice. Normal startup maintenance records one recovery, requests descendant stops and does not redispatch the provider. The opt-in Windows `desktop_and_leader_process_restart_preserves_recovered_session` additionally terminates/relaunches the built Desktop executable and verifies the recovered session over authenticated HTTP. It checks process survival and transport-visible state, not UI rendering.
- **Actual storage exhaustion:** `sqlite_full_preserves_worker_checkpoint_and_unacknowledged_outcome_for_retry` and `sqlite_full_keeps_accepted_bytes_and_retries_canonical_record_identity` assert SQLite's real `DiskFull` error under page limits, rollback, prior durable state and same-identity retries after restoring capacity. `enospc_preserves_artifact_and_worker_checkpoint_until_same_operation_retry` fills only a disposable 1 MiB tmpfs, asserts OS ENOSPC, preserves the last checkpoint and unacknowledged publication, allows a separate storage branch to progress, removes its filler, and recovers both original operation IDs across reopen.
- **Combined platform transfer:** `docker_windows_git_worktree_reaches_linux_worker_with_links_and_history` captures a real Windows linked worktree with Git executable/link metadata, moves the original repository and worktree, and sends the captured snapshot through the scoped gateway into a real Alpine worker. The worker verifies executable/link semantics, self-contained branch/history and retained reuse. Git installation belongs only to this disposable test image setup; the product still requires Git for repository snapshots.

Protocol remains **2.48**; these fixes need no schema migration. READMEs and the root changelog document the artifact-reference fix and regression boundaries. All new tracked fixtures use synthetic research data.

### Stage 10 review-fix verification results

Validation completed on **2026-11-27**, using synthetic fixtures and isolated host profiles:

| Gate | Result |
| --- | --- |
| Windows `cargo test --workspace --all-features` | **1,012 passed; 23 ignored; 0 failed**, including all 288 Desktop tests |
| Linux/WSL workspace, all features, excluding Desktop | **728 passed; 23 ignored; 0 failed**; the OAI component test passed again after strengthening its published assessment evidence |
| Sessions without default features | **49 passed; 0 failed** |
| Sessions with only `swarms` | **183 passed; 16 ignored; 0 failed** |
| Real AMD64 Docker suite | **13 passed; 0 failed**, including the combined Windows Git-worktree transfer and both constrained-capacity scenarios; the worktree case passed again with the final rebuilt worker after the remote-upload fix |
| Actual Desktop/leader process restart, opt-in Windows | **1 passed**, rerun against the final host code; process/HTTP evidence only |
| Actual ENOSPC on isolated Linux tmpfs, opt-in | **1 passed**; SQLite-full regressions also pass in both workspace gates |
| Opt-in Windows loopback WebFetch | **1 passed** |
| Workspace all-target/all-feature check and Windows/Linux Clippy | Passed with warnings denied; Sessions Clippy rerun after the final assessment-evidence assertions |
| Core feature-disabled and Swarms-only checks | Both passed |
| Static AMD64 musl worker/package | Rebuilt successfully with both host and remote-worker artifact-reference fixes |
| Formatting, included workflow-file formatting, whitespace and fixture/data audit | Passed; fixtures remain synthetic and no machine-local identities or credentials were added |

The default ignored counts include separately exercised Docker, Desktop-process and ENOSPC cases where applicable, subprocess entry fixtures exercised by their parent tests, and unavailable live-provider cases. They are not counted as default-gate passes. The final gates supersede transient fixture failures encountered while adding the tests (Windows test-stack size, an artifact-count assumption, and a retained test database handle); those fixture defects were corrected. No production fallback, compatibility shim, protocol bump or storage migration was introduced.

### Stage 10 validation limits and follow-up

- **Unavailable platform coverage:** native macOS and Arm64 execution require matching hosts/toolchains/runtime bundles. AMD64 Docker results do not establish those platforms. Hyper-V/Tart allocation and Windows-container support remain outside this plan.
- **Blocked interactive validation:** an isolated Desktop profile was launched, but both computer-use entry points failed before window discovery with `failed to write kernel assets: The system cannot find the path specified (os error 3)`, including after retry/reset. A UI-driven distributed bundled-workflow run and visual reconnect confirmation remain untested. Process-lifecycle/HTTP assertions do not substitute for those observations.
- **Unavailable deployment validation:** separate approved physical paired hosts and live-provider credentials were not supplied. No live research targets were used. Native macOS/Arm64 and physical multi-host Fleet validation remain untested.
- **Storage boundary:** actual ENOSPC was exercised in bounded disposable tmpfs and actual SQLITE_FULL through SQLite page limits. No host disk was filled and no physical storage-device or hardware-memory exhaustion is claimed.
- **Docker scope:** the earlier Stage 10 review-validation revision was rebuilt as a static AMD64 musl worker and packaged with the existing static Bash/license. Tests own their synthetic containers/volumes and use a test-only host-gateway mapping. No arbitrary existing user container or workspace is used.
- **Ignored tests:** ordinary gate counts include opt-in Docker/provider cases and subprocess entry fixtures. Docker cases run separately; parent supervisor tests invoke the subprocess fixtures. An ignored entry is never counted as a standalone pass.
- **Completion boundary:** the stage checkbox remains open for the blocked interactive run and the unavailable deployment/platform validation above. Bundled workflow coverage now includes complete scripted scheduler-driven VDH and OAI cycles in addition to component tests; a single UI-driven autonomous all-stage distributed workflow is not claimed. Completed automated and process/container checks are listed separately from those limits.


### Stage 10 final-review blocker fixes

- **Durable runtime lifecycle:** protocol 2.49 adds `swarm_handoff` to the role-specific tool contract and canonical assigned-worker dispatch. Canonical ownership gates forwarding, completion, blocking and cancellation. Inbox delivery and lifecycle updates share the existing mutation transaction/receipt. Explicit typed completion preserves assessment and accepted evidence; inference ending alone cannot fabricate a completed handoff. Temporary recovery retains ownership, exhausted recovery blocks work, explicit retries restore eligible retained work as detailed in the custody/retry fixes below, and branch stop/scan end cancel unfinished handoffs without overwriting completed results.
- **Lossless forwarding:** forward the saved envelope by handoff ID through immediate parents and permitted direct children. Optional `swarm_spawn_subagent.handoff_id` assigns the envelope atomically before a fresh recipient can run. The original envelope, producer and artifact identities remain unchanged; plain chat and task size limits remain bounded. Same-operation replay returns the saved acknowledgement without another inbox message or child.
- **Permanent regressions:** strict contract decoding; a greater-than-48-KiB envelope across forwarding, atomic spawn, authority rejection and reopened-store replay; failed managed-artifact completion; bounded failure/retry, successful completion, branch cancellation and scan-end cancellation. The bundled scheduler test now forwards an envelope over 16 KiB unchanged, completes its durable handoff, and still resumes the source worker with retained files.
- **Feasible integrated coverage:** new VDH scheduler acceptance performs the initial candidate, independent inconclusive assessment, missing-evidence return, supplemental publication, fresh independent validation and supported return assessment. New OAI scheduler acceptance lets item B complete ranking, discovery, validation, canonical reporting, patch generation and independent before/after patch checks while item A remains blocked, then resumes A through the same complete cycle. Scripts act only on model inputs and tool replies; fixture-side dispatch and status transitions do not advance these workflows. Assertions inspect canonical handoff results, original observations and exact patch/test identities afterward.
- **Compatibility:** rebuild Desktop and all participating app-servers/workers for protocol 2.49. Existing durable envelope fields and receipts are used; no database schema migration or compatibility fallback is introduced.

### Stage 10 final-review verification results

Completed on **2026-11-27**, with synthetic inputs and deterministic HTTP providers:

| Gate | Result |
| --- | --- |
| Windows `cargo test --workspace --all-features` | **1,017 passed; 23 ignored; 0 failed** |
| Linux/WSL `cargo test --workspace --exclude beale-desktop-app --all-features`, isolated home | **733 passed; 23 ignored; 0 failed** |
| Sessions without default features | **49 passed; 0 failed** |
| Sessions with only `swarms` | **184 passed; 16 ignored; 0 failed** |
| Workspace all-target/all-feature check | Passed, including Desktop and core |
| Windows workspace and Linux workspace excluding Desktop Clippy, all targets/features, warnings denied | Passed; final affected-crate Windows Clippy also passed |
| Core without default features and with only `swarms` | Both checks passed |
| Formatting, whitespace and synthetic-fixture audit | Passed |

Five new permanent tests cover strict handoff contracts, transactional lifecycle/large-envelope forwarding, bounded recovery and cancellation, the complete VDH cycle, and concurrent complete OAI cycles. The existing bundled scheduler regression now exercises lossless forwarding above the plain-message limit and asserts durable completion. Storage coverage also checks same-operation spawn replay, rejection of replacement while a blocked owner is still active, failed artifact acceptance and reopened-store replay. The full scheduler cases pass on Windows and Linux, including actual shell patch/test execution and canonical assigned-worker operations.

The final gates supersede development failures caused by the OAI fixture not enabling Findings and old tool-count assertions; both were corrected. The subsequent custody/retry review identified two additional handoff issues; their fixes and fresh verification are recorded below.

**Validation boundaries:** interactive Desktop automation/visual reconnect remains blocked by the previously recorded computer-use initialization failure. Native macOS/Arm64, separate physical paired Fleet hosts and live-provider validation remain unavailable here. Default ignored tests are not counted as passes. Earlier opt-in Docker, disposable-tmpfs ENOSPC, Desktop-process and WebFetch results remain recorded above. They were not repeated at that revision; fresh runs and a rebuilt static container package for the custody/retry fixes are recorded below. The current fixes were validated through Windows/Linux native assigned-worker loops and canonical transport. Rebuild participating binaries for protocol 2.49 before deployment. Stage 10's overall checkbox remains open for the separately documented environment validation, rather than asserting those unperformed runs passed.


### Stage 10 handoff custody and retry fixes

- **Former-owner input:** executable handoff inbox entries are tracked separately from result/status mail. Forwarding or replacement removes the previous owner's executable envelope in the same transaction as custody transfer. Subsequent prompts include an authoritative custody correction, including after database reopen and when earlier context was already checkpointed. Saved context and upstream results remain intact. Completion/cancellation also retire executable input.
- **Per-item recovery:** executor exhaustion and an unfinished inference turn mark only newly blocked owned handoffs as eligible for an explicit parent/operator retry. Claiming unrelated work does not change those handoffs or explicitly blocked items to Assigned. Blocking removes executable inbox input; an explicit recovery retry redelivers the immutable accepted envelope. A model-explicit block requires a per-item forwarding/completion decision; forwarding to the same inactive recipient is supported. Receipt replay remains atomic and does not duplicate deliveries.
- **Permanent regression coverage:** replacement is tested with both unread input and acknowledged, retained context; later parent and operator messages cannot replay the retired assignment. A deputy explicitly blocks A, receives B, exhausts recovery on B, and retries B without reactivating A; a subsequent explicit per-item retry can reactivate A alone. Another case receives unrelated B after executor exhaustion on A, then retries A through the operator path and completes both. Existing large-envelope, authority, result-notification, replay, stop and scan-end tests remain in the acceptance gate.
- **Compatibility boundary:** no new wire/model tool contract is introduced; participating binaries remain on protocol 2.49. Host-local durable supervisor metadata now distinguishes executable deliveries, recovery-eligible items and retired custody. Retired development sessions remain unsupported; no compatibility reader or conversion is added.


### Stage 10 custody/retry verification — 2026-11-27

All fixtures use synthetic inputs and deterministic providers. Three permanent storage regressions were added for the two reported issues; they include multiple items on one deputy, explicit per-item retry, database reopen, unread and acknowledged input, and operator-input visibility.

| Gate | Result |
| --- | --- |
| Windows workspace, all features | **1,020 passed; 23 ignored; 0 failed** |
| Linux/WSL workspace excluding Desktop, all features, isolated home | **736 passed; 23 ignored; 0 failed** |
| Final affected storage suite, all features | **178 passed; 1 ignored; 0 failed** |
| Sessions without default features | **49 passed; 0 failed** |
| Sessions with only Swarms | **184 passed; 16 ignored; 0 failed** |
| Workspace check, all targets/features | Passed |
| Windows workspace Clippy plus final affected-storage Clippy, warnings denied | Passed |
| Linux workspace excluding Desktop Clippy, all targets/features, warnings denied | Passed |
| Core disabled and Swarms-only builds | Both checks passed |
| Desktop and leader process restart, rebuilt matching binaries | **1 passed**; process lifecycle and recovered HTTP session assertions |
| Windows loopback WebFetch opt-in | **1 passed** |
| Actual ENOSPC in disposable, network-disabled 1 MiB tmpfs | **1 passed**; no host disk exhaustion |
| Fresh static AMD64 worker package and Docker acceptance | **13 passed; 0 failed**; final static-PIE binary and package manifest/hashes verified |
| Formatting, whitespace and synthetic-fixture/document audit | Passed |

The first Windows/Linux broad runs overlapped each other and the static build; each hit the unchanged OAI workflow timeout. The synthetic Windows fixture had completed 13 of 14 handoffs and was executing the last worker. Serial platform reruns passed the full workflow and all broad gates without increasing the timeout. The final operator-input classification adjustment also passed the complete affected-storage suite and Linux workspace gate.

**Remaining validation limits:** interactive Desktop distributed-workflow and visual reconnect validation remains unperformed after the previously recorded computer-use initialization failure; interactive automation was not retried in this fix pass. Native macOS/Arm64 execution, separate physical paired Fleet hosts, and live-provider validation require environments or credentials not supplied here. Process-restart assertions do not establish interactive UI behavior, and AMD64 Docker does not establish unsupported platforms. Ignored tests are not treated as passes; opt-ins and parent-invoked subprocess fixtures are accounted for separately. Stage 10's overall checkbox remains open for these validation boundaries.


The fresh Docker run passed both Fleet allocation cases and all eleven Sessions cases, including Alpine/Ubuntu execution, accepted artifacts, retained reuse, cancellation/lease expiry/daemon-loss recovery, orphan cleanup, minimal-image incompatibility, startup/PATH preservation, readiness retry, Windows-origin Git worktrees, abandoned allocation expiry, and both constrained-capacity scheduler cases. The earlier feasible opt-in gaps are now closed for this revision: Docker/static packaging, bounded ENOSPC, Desktop/leader process restart and loopback WebFetch were rerun successfully.

**Outcome:** both reported handoff implementation blockers are fixed, their permanent regressions pass, and no requested feasible acceptance check remains pending. The remaining open Stage 10 items are the explicitly unperformed environment-dependent validation above; this does not claim interactive, native macOS/Arm64, physical multi-host or live-provider validation.

### Stage 10 operator-message transcript fix

The final review found one operator-transcript blocker with two triggers: a retired-custody notice replaced visible operator text with the rendered inbox JSON, and a recovery delivery preceding an operator retry classified the whole turn as internal. Both paths now retain the original operator text independently of model input. Claim batching consumes at most one operator message, preserves later messages for subsequent turns, and derives operator metadata after prompt-size trimming. The model still receives the complete bounded inbox, assignment and custody context.

Local session completion, coordinator first-turn interruption and assigned-worker reconciliation persist the original operator text and its visible classification. Full model input is stored separately and used when local or assigned-worker history is reconstructed, so the transcript correction does not discard handoff context on a later turn. Host schema **2** adds nullable `session_turns.model_prompt`; the migration preserves existing transcript rows and is idempotent. Protocol **2.49** remains unchanged. Rebuild participating hosts/workers; earlier development in-flight turn formats remain unsupported without conversion.

Permanent regressions:

- `retired_custody_preserves_exact_operator_transcript_after_reopen`: exact whitespace/Unicode text in returned and reopened transcripts after custody retirement; two queued operator messages remain separate turns; full custody input survives later inference. Also covers first-turn coordinator interruption and reopening its stored transcript.
- `recovery_delivery_preserves_visible_operator_transcript_after_reopen`: the same transcript, visibility, queued-message, retained-context and interruption assertions when an executor-blocked handoff is redelivered before the operator retry.
- `worker_reconciliation_preserves_operator_transcript_and_model_input_after_reopen`: mixed parent/operator mail becomes exact visible operator text after failed-worker reconciliation, fresh database reopen and resumed worker bootstrap; model input retains the parent delivery.
- `model_prompt_migration_preserves_existing_transcript_and_is_idempotent`: upgrades host schema 1 with an existing transcript to schema 2 twice without rewriting the original text or fabricating model input.

### Stage 10 operator-message acceptance results

Earlier tables describe their respective revisions and are not current-revision passes. The following results use this operator-message fix:

| Check | Result |
| --- | --- |
| Focused transcript, reopen and migration tests | **9 passed; 0 failed** (including the four new permanent regressions) |
| Windows workspace, all features | **1,024 passed; 23 ignored; 0 failed** |
| Linux/WSL workspace excluding Desktop, all features | **740 passed; 23 ignored; 0 failed** |
| Bundled Basic, VDH and OAI scheduler workflows | Passed in both workspace gates |
| Sessions with features disabled | **49 passed; 0 failed** |
| Sessions with only Swarms | **187 passed; 16 ignored; 0 failed** |
| Workspace all-target/all-feature check | Passed |
| Windows and Linux workspace all-target/all-feature Clippy | Passed with warnings denied |
| Core feature-disabled and Swarms-only checks | Both passed |
| Bounded Linux tmpfs ENOSPC opt-in | **1 passed** using the current Linux test binary |
| Fresh static AMD64 worker/package | Rebuilt successfully; static ELF verified and packaged worker/Bash/license sizes and SHA-256 verified |
| Real AMD64 Docker opt-ins | **13 passed; 0 failed** (2 Fleet and 11 Sessions cases), using the freshly rebuilt package |
| Desktop/leader process restart opt-in | **1 passed** against rebuilt executables; process/HTTP coverage only |
| Loopback WebFetch opt-in | **1 passed** |

Formatting, whitespace and the changed-source/test/documentation synthetic-data audit passed. Current gate logs are under `beale-v2/target/stage10-transcript-*.log`; these ignored local artifacts are not required runtime inputs. The Docker run covers Alpine/Ubuntu execution, artifact and metadata transfer, retained reuse, cancellation/lease expiry/daemon-loss recovery, orphan cleanup, image compatibility and startup/PATH behavior, readiness retries, Windows-origin Git worktrees, abandoned allocation expiry and both constrained-capacity scheduler cases.

**Outcome:** both triggers of the reported operator-message blocker are fixed, with permanent transcript/reopen regressions and retained-model-input coverage. All requested feasible automated acceptance gates were rerun successfully on this revision; no such gate remains pending. This does not claim the interactive, platform or deployment validation listed below.

### Stage 10 operator-message validation limits

- **Interactive validation remains unperformed:** the UI-driven distributed workflow and visual reconnect checks were previously blocked by computer-use initialization failure. Interactive automation was not retried in this fix pass. The successful rebuilt Desktop/leader process test verifies process lifecycle and HTTP state, not visual behavior.
- **Unavailable platform/deployment validation:** native macOS and Arm64 hosts/toolchains, separate approved physical paired Fleet hosts, and live-provider credentials are not supplied. Linux/WSL and AMD64 Docker tests do not establish those environments. No live research targets were used.
- **Storage scope:** ENOSPC uses a disposable bounded tmpfs; SQLite-full cases use synthetic database limits. Hardware-memory exhaustion and physical storage-device failures are not claimed.
- **Ignored entries:** opt-in tests and subprocess fixture entry points remain ignored by ordinary workspace gates. Separately run opt-ins are reported independently; ignored entries are never counted as passes.
- **Stage completion:** Stage 10's overall checkbox remains open for the unperformed interactive and unavailable platform/deployment validation. This operator-message fix does not expand the platform scope to Hyper-V, Tart or Windows containers.


### Stage 10 existing-session interruption fix

The final operator-message review found that coordinator recovery saved an interruption transcript only when the session did not exist. A claimed operator follow-up after custody retirement or recovery redelivery therefore remained invisible after reopening an existing session, even though recovery had finished and stopped the leader.

Recovery now preserves the claimed operator text and full model input in new and existing sessions. The `swarm-local-workers` schema advances from **2 to 3**, adding nullable `swarm_local_coordinators.committed_turn`. A host-owned turn extension records the attempt-to-transcript binding inside normal transcript persistence. Recovery appends an explicit interruption notice only for an uncommitted attempt; a committed turn keeps its original answer and user text. Recovery insertion, the binding, hierarchy stop, fences and completion flag share one transaction. Failed persistence leaves the recovery pending without a partial transcript or false completion. Protocol **2.49** and host schema **2** remain unchanged. Rebuild participating hosts/workers; earlier development in-flight executions remain unsupported across rebuilds.

Permanent regressions:

- `existing_session_custody_interruption_and_duplicate_recovery_preserve_transcript` and `existing_session_redelivery_interruption_and_duplicate_recovery_preserve_transcript` each begin with an actual completed session-engine turn. Both cover interruption before saving the follow-up, interruption after its successful save but before supervisor checkpoint completion, and exhaustion of transcript-binding writes without repeated inference. They verify exact operator text and full model input after reopening, unchanged prior history, preservation of completed answers, one interruption lifecycle event, and no duplicate transcript after repeated normal recovery sweeps and explicit replay.
- Each interruption scenario injects failure at the final recovery-completion write and verifies that transcript and hierarchy changes roll back before successful retry through normal coordinator recovery with its ownership lock.
- `coordinator_transcript_binding_migration_preserves_completed_intents` upgrades schema 2 twice, retaining completed intent data and leaving its new binding empty. Existing first-turn, worker-reconciliation, host-schema migration and handoff regressions remain in place.

### Stage 10 interruption acceptance results

Prior tables describe earlier revisions, not new passes for this fix. Current-revision results:

| Check | Result |
| --- | --- |
| Focused transcript/reopen, interruption, duplicate-recovery and migration tests | **12 passed; 0 failed** |
| Windows workspace, all features | **1,027 passed; 23 ignored; 0 failed** |
| Windows workspace all-target/all-feature check and Clippy | Passed with warnings denied |
| Linux/WSL workspace excluding Desktop, all features | **743 passed; 23 ignored; 0 failed** |
| Linux all-target/all-feature Clippy | Passed with warnings denied |
| Bundled Basic, VDH and OAI scheduler workflows | Passed in both workspace gates |
| Sessions with features disabled | **49 passed; 0 failed** |
| Sessions with only Swarms | **189 passed; 16 ignored; 0 failed** |
| Core disabled and Swarms-only checks | Both passed |
| Fresh static AMD64 worker/package | Rebuilt; static ELF and packaged worker/Bash/license sizes and SHA-256 verified |
| Real AMD64 Docker tests | **13 passed; 0 failed** (2 Fleet and 11 Sessions), using the fresh worker package |
| Actual ENOSPC in bounded Linux tmpfs | **1 passed** |
| Desktop/leader process restart | **1 passed** against rebuilt matching executables; process/HTTP coverage only |
| Loopback WebFetch | **1 passed** |

Formatting, whitespace and the changed-source/test/documentation synthetic-data audit passed. Current logs are under `beale-v2/target/stage10-interruption-*.log`. The Docker suite includes Alpine/Ubuntu execution, retained reuse, artifact/metadata transfer, cancellation and reconnect recovery, image/startup compatibility, readiness retry, orphan cleanup, Windows-origin Git worktrees, abandoned allocation expiry, and both constrained-capacity scheduler cases. The migration fixture's initial missing parent row was corrected before the recorded passing gates; no production fallback was added.

**Outcome:** the reported existing-session interruption blocker is fixed for custody retirement and recovery redelivery. Permanent regressions cover interruption before and after transcript commit, duplicate recovery, failed persistence followed by successful retry, preserved completed answers, and the schema migration. All requested feasible automated acceptance checks were rerun successfully on this revision; none remain pending. The remaining Stage 10 limits are the unperformed interactive and unavailable environment-dependent validation below.

### Stage 10 interruption validation limits

- **Interactive validation remains unperformed:** the UI-driven distributed workflow and visual reconnect checks were previously blocked by computer-use initialization failure. Interactive automation was not retried in this fix pass. Process/HTTP restart assertions do not establish visual behavior.
- **Unavailable platform/deployment validation:** native macOS and Arm64 execution require matching hosts/toolchains; separate approved physical paired Fleet hosts and live-provider credentials were not supplied. No live research targets were used. Linux/WSL and AMD64 Docker results do not establish those environments.
- **Storage boundary:** actual ENOSPC uses disposable bounded tmpfs, and SQLite-full cases use bounded synthetic database limits. No host disk was filled; hardware-memory exhaustion and physical storage-device failures are not claimed.
- **Ignored entries:** opt-ins and subprocess fixture entry points remain ignored by default gates. Independently executed opt-ins are reported separately; ignored entries are not counted as passes.
- **Completion boundary:** Stage 10's overall checkbox stays open for the unperformed interactive and unavailable platform/deployment validation. Hyper-V, Tart and Windows-container support remain outside this plan.


### Stage 10 checkpoint-exhaustion fix

The final interruption review found a remaining gap in supervisor handling: after transcript persistence exhausted its retries, the supervisor stopped the attempt and completed its coordinator recovery record without preserving the operator turn. The earlier rejected-write tests invoked recovery directly before this supervisor handling and therefore did not cover the production failure sequence.

Coordinator completion now preserves an uncommitted transcript before marking recovery complete, in the same transaction. If persistence remains unavailable, the stopped attempt stays on the recovery queue. Ordinary background maintenance repairs the transcript of stopped attempts without changing their settled lifecycle, repeating inference, or stopping a replacement generation. Already-committed turns retain their original answers and model input. Recovery notices describe an unsaved coordinator transcript without incorrectly asserting that a later generation is stopped. For the current exhausted generation, they retain the bounded local recovery failure explanation. Host schema **2**, `swarm-local-workers` schema **3**, and protocol **2.49** remain unchanged.

Permanent regressions `normal_supervisor_custody_checkpoint_exhaustion_preserves_transcript` and `normal_supervisor_redelivery_checkpoint_exhaustion_preserves_transcript` start with an existing engine-persisted session and exercise the normal hierarchy supervisor. Both inject transcript-binding write failure, verify bounded persistence retries with one inference, and check that supervisor completion leaves the stopped attempt pending. They run ordinary automation maintenance while the fault persists, restore storage, wait for automatic recovery, reopen the transcript, and verify exact operator text, preserved full model input and prior history, one recovery notice, unchanged settled lifecycle, and no duplicate turns after further maintenance and explicit replay. Existing interruption-before/after-commit, atomic rollback, migration, live-owner and replacement-generation regressions remain in place.

### Stage 10 checkpoint-exhaustion acceptance results

Earlier acceptance tables record earlier revisions and are not new passes for this fix. Current logs use `beale-v2/target/stage10-checkpoint-*.log`.

| Check | Result |
| --- | --- |
| Focused transcript/reopen, normal-supervisor exhaustion, interruption and migration tests | **14 passed; 0 failed** |
| Local scheduler/provider recovery regression gate | **10 passed; 0 failed** |
| Windows workspace, all features, four test threads | **1,029 passed; 23 ignored; 0 failed** |
| Windows workspace all-target/all-feature check and Clippy | Both passed; Clippy with warnings denied |
| Linux/WSL workspace excluding Desktop, all features | **745 passed; 23 ignored; 0 failed** |
| Linux all-target/all-feature Clippy | Passed with warnings denied |
| Sessions with features disabled | **49 passed; 0 failed** |
| Sessions with only Swarms | **191 passed; 16 ignored; 0 failed** |
| Core disabled and Swarms-only checks | Both passed |
| Final static AMD64 worker/package | Rebuilt; static ELF and packaged worker/Bash/license sizes and SHA-256 verified |
| Real AMD64 Docker tests using the final worker package | **13 passed; 0 failed** (2 Fleet and 11 Sessions) |
| Actual ENOSPC in bounded Linux tmpfs | **1 passed** |
| Desktop/leader process restart | **1 passed** against rebuilt matching executables; process/HTTP coverage only |
| Loopback WebFetch | **1 passed** |

The initial broad gates caught a recovery-notice regression in the existing provider-exhaustion tests; the notice was corrected before the final gates. An additional Windows concurrent run encountered a retryable loopback transport error in `memory_rejections_replay_without_masking_storage_failures`; it passed independently without a code change, and the final Windows workspace gate passed with four test threads. Initial logs remain separately identified; failed runs are not counted as passes. The final runtime bundle is `beale-v2/target/stage10-checkpoint-runtime-final`.

Formatting, whitespace and the changed-source/test/documentation synthetic-data audit passed. The bundled Basic, VDH and OAI scheduler workflows passed in both workspace gates. Docker coverage includes Alpine/Ubuntu execution, retained reuse, artifacts and metadata, cancellation/reconnect, startup compatibility, readiness retries, orphan cleanup, Windows-origin Git worktrees, abandoned allocation expiry, and both constrained-capacity scheduler scenarios.

**Outcome:** the checkpoint-exhaustion blocker is fixed for custody retirement and recovery redelivery, with permanent regressions through the normal supervisor and background maintenance. All feasible automated acceptance checks were rerun successfully on the final revision; none remain pending. The remaining Stage 10 validation limits are listed below.


### Stage 10 checkpoint-exhaustion validation limits

- **Interactive validation is currently blocked:** computer-use initialization was retried for this change and failed with `failed to write kernel assets: The system cannot find the path specified. (os error 3)`. UI-driven distributed workflows and visual reconnect remain unperformed. Process/HTTP restart coverage does not establish visual behavior.
- **Unavailable platform/deployment validation:** native macOS and Arm64 execution require matching hosts/toolchains. Separate approved physical Fleet hosts and live-provider credentials were not supplied. No live research targets were used.
- **Storage boundary:** actual ENOSPC is confined to a disposable bounded Linux tmpfs; SQLite-full coverage uses bounded synthetic database limits. Hardware-memory exhaustion and physical storage-device failures are not claimed.
- **Ignored entries:** opt-ins and subprocess fixtures remain ignored in default gates; independently executed opt-ins are reported separately and ignored entries are not counted as passes.
- **Completion boundary:** the Stage 10 overall checkbox remains open for the blocked interactive and unavailable platform/deployment validation. Hyper-V, Tart and Windows-container support remain outside scope.


### Stage 10 terminal-finalization fix

The final checkpoint-exhaustion review found that a failed coordinator-completion write returned before normal scan finalization. Maintenance later repaired the transcript and cleared recovery but never persisted the scan end or started retention. The previous normal-supervisor tests preserved that incomplete state in their equality assertions.

Terminal coordinator completion and maintenance recovery now use the same canonical scan-end transaction as explicit scan end. Transcript repair, terminal finalization and the recovery-complete flag commit together. Failed transcript or scan-end writes leave the entire recovery pending for retry. Finalization starts the configured retention clock, cancels unfinished handoffs, revokes worker authority and preserves termination uncertainty and all environment cleanup protections. A historical attempt cannot end a newer leader generation; an already-completed record is a no-op after reopening. Normal successful-turn scheduling remains unchanged. Host schema **2**, `swarm-local-workers` schema **3**, and protocol **2.49** remain unchanged.

Permanent coverage:

- Both `normal_supervisor_custody_checkpoint_exhaustion_preserves_transcript` and `normal_supervisor_redelivery_checkpoint_exhaustion_preserves_transcript` now inject transcript failure followed by scan-end failure under ordinary background maintenance. They verify rollback and successful retry, exact reopened transcript/model input, a single persisted end clock and deadline generation, and unchanged deadlines after repeated maintenance and explicit replay. Before expiry, cleanup is denied. After expiry, an otherwise eligible environment can complete the canonical cleanup lifecycle; Keep, active references, pending publication and unconfirmed termination each independently prevent cleanup. These assertions exercise canonical cleanup transitions; the separate Docker gate exercises physical cleanup.
- `coordinator_terminal_completion_rolls_back_and_cannot_end_a_reopened_scan` covers failed finalization through supervisor completion, successful retry, duplicate completion/recovery, and replay after reopening without a newer attempt.
- The existing live-replacement regression now also replays completion of the recovered historical attempt, verifying that the replacement scan remains unended and unchanged. Existing live-owner, credential/fencing rollback, committed-answer and migration regressions remain in place.

### Stage 10 terminal-finalization acceptance results

Earlier acceptance tables describe earlier revisions. Current logs use `beale-v2/target/stage10-finalization-*.log`.

| Check | Result |
| --- | --- |
| Focused transcript, normal-supervisor recovery, retention and migration tests | **14 passed; 0 failed** |
| Coordinator ownership, atomic finalization, replacement and reopen replay | **4 passed; 0 failed** |
| Local scheduler/provider recovery gate | **10 passed; 0 failed** |
| Canonical lifecycle and retention foundation suite | **59 passed; 1 ignored; 0 failed** |
| Windows workspace, all features, four test threads | **1,030 passed; 23 ignored; 0 failed** |
| Windows all-target/all-feature check and Clippy | Both passed; Clippy with warnings denied |
| Linux/WSL workspace excluding Desktop, all features, four test threads | **746 passed; 23 ignored; 0 failed** |
| Linux all-target/all-feature Clippy | Passed with warnings denied |
| Sessions with features disabled | **49 passed; 0 failed** |
| Sessions with only Swarms | **192 passed; 16 ignored; 0 failed** |
| Core disabled and Swarms-only checks | Both passed |
| Rebuilt static AMD64 worker/package | Static ELF verified; worker/Bash/license sizes and SHA-256 verified |
| Real AMD64 Docker acceptance | **13 passed; 0 failed** (2 Fleet and 11 Sessions) |
| Bounded tmpfs ENOSPC | **1 passed** |
| Desktop/leader process restart | **1 passed** against rebuilt matching executables; process/HTTP coverage only |
| Loopback WebFetch | **1 passed** |

Formatting, whitespace and the changed-source/test/documentation synthetic-data audit passed. The bundled Basic, VDH and OAI scheduler workflows passed in both workspace gates. All 13 Docker tests used the rebuilt worker package at `beale-v2/target/stage10-finalization-runtime`; they cover Alpine/Ubuntu execution, retained reuse and cleanup, artifacts and metadata, cancellation/reconnect, startup compatibility, readiness retry, orphan cleanup, Windows-origin Git worktrees, abandoned allocation expiry and both constrained-capacity scheduler scenarios. No acceptance gate failed in this fix pass.

**Outcome:** terminal finalization is fixed for custody retirement and recovery redelivery. Both normal-supervisor regressions verify atomic rollback and retry, retention deadlines and independent cleanup protections. Coordinator regressions also protect reopened scans and newer generations from completion replay. All feasible automated acceptance checks were rerun successfully on this revision; none remain pending. The remaining validation limits are listed below.

### Stage 10 terminal-finalization validation limits

- Interactive distributed-workflow and visual reconnect checks remain blocked: computer-use initialization was retried during this pass and again failed with `failed to write kernel assets: The system cannot find the path specified. (os error 3)`. Process/HTTP restart assertions do not establish visual behavior.
- Native macOS and Arm64 execution, separate approved physical Fleet hosts and live-provider validation require hosts/toolchains or credentials that were not supplied. No live research targets are used.
- Actual ENOSPC is bounded to disposable Linux tmpfs; SQLite-full tests use bounded synthetic database limits. Hardware-memory exhaustion and physical storage-device failures are not claimed.
- Ignored opt-ins and subprocess fixture entry points are not counted as passes; independently executed opt-ins are reported separately.
- Stage 10 remains open for blocked interactive and unavailable platform/deployment validation. Hyper-V, Tart and Windows containers remain outside scope.

### Stage 10 successful-completion finalization fix

The final terminal-finalization review found an adjacent successful-completion gap: the coordinator recovery record could be cleared before the supervisor's separate scan-end write. Owner loss after the successful checkpoint, or a failed end write, could leave a completed scan without an end clock or retention deadline.

`ScanState::ready_to_end` now supplies the same terminal-state predicate to the supervisor and the coordinator completion/recovery transaction. Successful completion requires a completed leader, all executions stopped, and no queued or assigned work. Scan end and retention commit before the recovery record is cleared. A failed finalization write leaves recovery pending; background maintenance preserves the committed answer and finishes finalization without another provider call. Existing newer-generation and completed-record replay guards remain in place. Host schema **2**, `swarm-local-workers` schema **3**, and protocol **2.49** are unchanged.

Permanent regressions:

- `completed_coordinator_owner_loss_finalizes_retention_without_reinference` persists an actual engine answer and successful checkpoint, releases the saved owner lock at the interruption boundary before coordinator completion, and starts a fresh runtime's normal background maintenance. It models owner loss at that precise durable boundary; it is not an additional physical process-kill test.
- `completed_coordinator_end_write_failure_retries_through_normal_maintenance` runs the normal hierarchy supervisor with a rejected scan-end write. It verifies that the saved answer/checkpoint do not clear recovery, a failed maintenance sweep rolls back finalization, and restoring storage allows automatic retry.
- Both start from an existing persisted session, preserve exact operator text, answer and prior history after reopening storage, retain the Completed lifecycle, assert one inference for the follow-up, and verify a single retention clock/deadline generation after repeated maintenance and explicit replay. Cleanup fails before expiry, succeeds at expiry for the eligible environment, and remains blocked by Keep. The existing normal-supervisor checkpoint-exhaustion tests separately cover active references, pending publications and unconfirmed termination.

### Stage 10 successful-completion acceptance results

Earlier tables describe earlier revisions. Current logs use `beale-v2/target/stage10-completion-*.log`.

| Check | Result |
| --- | --- |
| New successful-completion regressions | **2 passed; 0 failed** |
| Windows workspace, all features, four test threads | **1,032 passed; 23 ignored; 0 failed** |
| Windows workspace all-target/all-feature check and Clippy | Both passed; Clippy with warnings denied |
| Linux/WSL workspace excluding Desktop, all features, four test threads | **748 passed; 23 ignored; 0 failed** |
| Linux all-target/all-feature Clippy | Passed with warnings denied |
| Sessions with features disabled | **49 passed; 0 failed** |
| Sessions with only Swarms | **194 passed; 16 ignored; 0 failed** |
| Core disabled and Swarms-only checks | Both passed |
| Rebuilt static AMD64 worker/package | Static ELF verified; worker/Bash/license sizes and SHA-256 verified |
| Real AMD64 Docker acceptance | **13 passed; 0 failed** (2 Fleet and 11 Sessions) |
| Bounded tmpfs ENOSPC | **1 passed** |
| Desktop/leader process restart | **1 passed** against rebuilt matching executables; process/HTTP coverage only |
| Loopback WebFetch | **1 passed** |

Formatting, whitespace and the changed-source/test/documentation synthetic-data audit passed. Both workspace gates include the new successful-completion tests, the earlier normal-supervisor retention/cleanup regressions, coordinator rollback/replay/replacement tests, and the bundled Basic, VDH and OAI scheduler workflows. The current static worker bundle is `beale-v2/target/stage10-completion-runtime`.

**Outcome:** successful-completion finalization is fixed for owner loss after a saved checkpoint and for failed scan-end writes. The permanent regressions verify automatic retry without re-inference, transcript preservation, one retention clock and cleanup protection. All feasible automated acceptance checks were rerun successfully on this revision; none remain pending. Docker acceptance used the rebuilt current-source worker. Remaining validation limits are listed below.

### Stage 10 successful-completion validation limits

- Interactive distributed-workflow and visual reconnect checks remain blocked: computer-use initialization was retried during this pass and again failed with `failed to write kernel assets: The system cannot find the path specified. (os error 3)`. Process/HTTP restart assertions do not establish visual behavior.
- Native macOS and Arm64 execution, separate approved physical Fleet hosts and live-provider validation require hosts/toolchains or credentials that were not supplied. No live research targets are used.
- Actual ENOSPC is bounded to disposable Linux tmpfs; SQLite-full tests use bounded synthetic database limits. Hardware-memory exhaustion and physical storage-device failures are not claimed.
- Ignored opt-ins and subprocess fixture entry points are not counted as passes; independently executed opt-ins are reported separately.
- Stage 10 remains open for blocked interactive and unavailable platform/deployment validation. Hyper-V, Tart and Windows containers remain outside scope.

### Stage 10 final review and implementation completion

**Implementation complete.** The final review found no remaining implementation blockers or blocking regression-coverage gaps in successful-completion finalization. Scan end commits before coordinator recovery clears; failed writes remain retryable. Coverage verifies preserved answers, stable retention deadlines, cleanup protections, and replay safety for reopened scans and newer generations. The review made no source changes.

Checks rerun during the final review:

| Check | Result |
| --- | --- |
| Coordinator tests, including both successful-completion regressions | **13 passed; 0 failed** |
| Transcript tests, including both normal-supervisor checkpoint-exhaustion regressions | **10 passed; 0 failed** |
| Canonical lifecycle and retention foundation suite | **59 passed; 1 ignored; 0 failed** |

Review logs use `beale-v2/target/stage10-completion-review-*.log`. The owner-loss regression models the precise saved-checkpoint boundary by releasing execution ownership; the separate process-restart test supplies the previously recorded process/HTTP coverage.

**Feasible checks not repeated during the final review:** the full Windows/Linux suites, Docker, feature isolation, Clippy, ENOSPC, process restart and WebFetch. Their preceding successful runs cover the unchanged implementation and remain recorded in the successful-completion acceptance table: **1,032 Windows tests, 748 Linux tests and 13 Docker tests**, plus the other listed checks. These are prior passes, not new review runs. No feasible automated acceptance check remains unperformed for this implementation revision.

Completion tracking:

- [x] Stage 10 implementation and documentation complete within the agreed scope.
- [x] Final implementation review and focused regression checks passed.
- [x] Feasible automated acceptance checks passed on the final implementation revision.
- [ ] Interactive distributed-workflow and visual reconnect validation — blocked by the recorded computer-use initialization failure; not retried during the final review.
- [ ] Native macOS execution validation — matching host/toolchain unavailable.
- [ ] Native Arm64 execution validation — matching host/toolchain unavailable.
- [ ] Separate physical Fleet-host interoperability validation — approved paired hosts unavailable.
- [ ] Live-provider validation — credentials not supplied; no live research targets used.

The overall Stage 10 checkbox remains open until the outstanding validation is completed or its scope is explicitly revised. Hardware-memory exhaustion and physical storage-device failures remain untested; bounded tmpfs ENOSPC and synthetic SQLite-full tests do not establish those boundaries. Hyper-V, Tart and Windows containers remain outside scope. This documentation update records the final review and does not claim additional test runs or new implementation work.
