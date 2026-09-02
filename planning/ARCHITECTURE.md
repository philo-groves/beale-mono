# Architecture

app-server is a general-purpose research agent built around Pi's native agent loop. It puts useful workspace context, durable knowledge, and tools in front of the selected model, then lets the model plan and decide when the work is complete.

## Core Ownership

- The model owns investigation planning, decomposition, tool use, collaboration, and completion.
- app-server owns context compilation, tool execution, durable knowledge, storage, orchestration, live events, and flow captures.
- A host such as Beale owns workspace setup, authorization recording, repository references, researcher interaction, and presentation.

app-server does not maintain an outer goal tree, generated subgoals, a triager, or completion gates.

## Runtime Flow

1. The app-server host supplies a research request and structured authorized workspace context.
2. app-server selects concise relevant knowledge, leads, and findings from the unified SQLite database.
3. app-server compiles projected workspace identity, source references, selected knowledge and claims, and skills into model context. Database paths and storage layout remain runtime-only.
4. Pi runs the selected model with research tools and collaboration tools.
   app-server retries a model turn when the provider reports a retryable transient failure before emitting substantive output; partial turns and non-retryable failures remain terminal.
5. Research tool observations are appended to the operational event stream. The model explicitly saves reusable knowledge or advances evidence-backed claims.
6. The model may spawn bounded child sessions, communicate with them, and incorporate their results.
7. app-server returns the root response and writes a schema-v4 flow capture containing the root result, child sessions, tools, compiled context, and operational storage metadata.

## Context

The compiled workspace context is guidance, not a repository permission fence. It contains:

- the user's request;
- structured authorization and scope metadata supplied by the host;
- known repositories and materialized source paths;
- bounded session, workspace, and relevant or linked subject memory with stable ids, evidence references, relationships, and revisions; and
- selected skills.

Pi presents the tools themselves through their typed definitions. app-server enforces permissions and budgets in lifecycle hooks instead of restating that policy as prompt prose. The compiled-context event records concise summaries of the tools actually available for inspection by host interfaces.

Repository paths help the model discover likely source. A repository may include bounded nested content roots when a host checkout wraps the actual project directory. A repository need not be known before research begins, and the same user-global checkout may be referenced by multiple workspaces.

## Native Agent And Subagents

Subagent orchestration sits behind a provider-neutral runtime factory shared by the Pi, Claude, and ZCode executor paths. Collaboration settings select one of two implementations. Simple is the compatibility default; Advanced is opt-in.

The root session is `/root`. In Simple mode it receives Pi research tools plus direct collaboration tools including:

- `spawn_agent`
- `send_message`
- `followup_task`
- `interrupt_agent`
- `list_agents`
- `wait_agent`

Simple mode retains direct spawning, bounded inheritance, lead/child messaging, follow-ups, interruption, waiting, and durable channel participation.

Advanced mode replaces direct spawning and peer messaging with lead-only `delegate_batch`, `work_status`, `steer_work`, `cancel_work`, and `wait_for_work` controls. It permits one exploration wave of at least two disjoint scouts followed by at most one verification wave containing an independent falsifier or verifier. Verification cannot start until exploration is terminal and must cite the candidate evidence or work it challenges. Workers receive fresh context plus explicit task packets, have no delegation or peer-messaging tools, and can only read or contribute to durable channels. Terminal prose is projected into a typed result with conclusions, observations, evidence references, negative results, uncertainty, contradictions, next experiments, and schema-completeness state. The lead remains the sole synthesis owner. Focused, Balanced, and Deep intensity allow two, four, and six active evidence workers respectively.

Each child receives an opaque id and canonical path such as `/root/parser_review`. Both modes limit children to one level.

`spawn_agent` accepts `fork_turns: "all"`, `"none"`, or a positive integer string. The runtime removes the unresolved spawning tool call from inherited history. Full-history children inherit the parent's model and reasoning effort. Partial-history or fresh children can select another model available from the active provider and a supported effort.

In Simple mode, messages use per-session mailboxes. `send_message` queues without starting an idle turn. `followup_task` starts another turn for an idle non-root child or reaches a running child at a message boundary. Advanced steering and cancellation wrap those lifecycle operations behind lead-only work-ledger controls. Interrupting a child aborts only its active turn and keeps its session available. Completion and failure notifications are delivered to the parent conversation.

Children share the active workspace, research tool registry, governance, storage, memory tier context, and cancellation boundary. Delegation cannot broaden authorization or permissions. A host stop control aborts the root and every pending or running child; interrupted child states are emitted before the process exits. Model streams that produce no response content for three minutes are retried so one silent provider request cannot indefinitely hold the tree open.

Collaboration calls use the same requested/observed research-event envelope as executable tools, but they remain coordination operations rather than research action classes. Events carry the calling agent identity and preserve normalized inputs, results, and failures for live host rendering and durable replay.

## Durable Knowledge

app-server uses the host-compatible SQLite database as the source of truth. Durable research state has three complementary projections:

- **Knowledge memory** stores reusable facts, entities, boundaries, constraints, references, methods, and trajectories in a concise typed graph.
- **Research claims** use one stable identity. A proposed or refuted claim appears as a Lead; direct evidence promotes that same row and ID into the Findings view.
- **Campaign tracks** organize questions, experiments, observations, next actions, claims, runbooks, and reports across sessions without becoming another claim store.

The model-facing memory tools search, read, save, correct, and link knowledge. `lead.create`, `lead.list`, `finding.list`, and `finding.transition` operate the canonical claim ledger. Claims have independent projection, maturity, freshness, workflow, classification, components, evidence, and revision dimensions.

Knowledge records share the user-global SQLite database and are tiered by:

- session id for work useful only to the current session;
- workspace for knowledge reusable across sessions on one research target; and
- subject for knowledge reusable across workspaces owned by the same subject.

Transcripts, narration, and bulk tool output are operational data, not durable knowledge. Large outputs remain artifact files referenced by concise graph nodes.

Runbooks are a separate workspace-scoped artifact family for reusable multi-step procedures. SQLite stores their ownership, lifecycle, artifact identity, and optimistic revision. Valid Jupyter `nbformat 4` files store ordered markdown/code cells and bounded recorded results under `~/.beale/artifacts/runbooks/<workspace-id>/`. Runbooks are portable documents, not an alternate executor; all commands run through the normal research tools.

The agent searches knowledge, leads, and findings early and when research crosses system boundaries. In the Security profile, a suspected vulnerability starts as a Lead, an evidence-backed isolated flaw is classified `security.primitive`, and an end-to-end result that composes component claims into demonstrated impact is classified `security.chain`. In the Mathematics profile, conjecture, theorem, and counterexample are claim classifications rather than memory-node types. Reusable flow endpoints, invariants, mitigations, references, techniques, and trajectories remain knowledge. Routine narration is not durable state.

Claim transitions are append-audited and revision-checked. Direct evidence is required to promote a Lead to a Finding; reproduction requires a successful durable runbook execution; verification requires independent evidence; reporting and disclosure require matching durable report or disclosure references. A composite verified claim must cite component claim IDs. Legacy hypothesis, primitive, chain, conjecture, theorem, counterexample, and finding rows remain in SQLite for audit but migrate idempotently to stable claim identities and are hidden from the knowledge projection.

Memory queries are tokenized and relevance-ranked across ids, types, content, assets, tags, and evidence. An exact node id embedded in a broader natural-language query remains directly retrievable.

Research attention is product policy, not profile ontology. Lead, maturity, staleness, reporting, and closure state determine session heat and mobile notifications; profiles only define domain semantics and claim classifications. Operators may customize the product-wide attention palette without changing claim urgency.

## Research Tools

Research tools expose concrete capabilities such as repository search, bounded file reads, structural code intelligence, analysis, experiments, runbook artifacts, storage inspection, memory access, skills, and configured MCP servers.

Each research tool has a typed schema, action class, side-effect profile, required permissions, and structured result. app-server enforces tool governance in lifecycle hooks. Collaboration tools are orchestration primitives and remain available when a research-call budget is exhausted.

Tool-backed observations may promote or advance claims. Model or child prose alone does not become evidence.

## Storage And Capture

The default durable surfaces are:

- `~/.beale/memory.sqlite` for cross-workspace operational state and tiered knowledge; and
- `~/.beale/artifacts/` for files, raw outputs, logs, generated material, and reproducible scripts.

The shared SQLite database uses an append-only, component-scoped migration ledger. app-server owns the `app_server_core` sequence and adopts the idempotent graph baseline for databases created before the ledger existed.

Schema-v4 flow captures summarize the request, root result, child session tree, model calls, tool events, compiled context with selected graph knowledge, and storage manifest. Child metadata includes path, parent, lifecycle state, model, effort, inheritance mode, timestamps, result, errors, and usage.

## Trust Boundary

OpenAI credentials remain in the host credential layer. Host-provided authorization is recorded once and inherited by child sessions. app-server treats external tool content as untrusted input and does not interpret delegation as authorization expansion.

Isolation is an operator and host choice. Allowlisted local experiments are auditable tools, not a security sandbox.

## Current Limits

- Child sessions use models from the root session's active provider.
- Child depth and concurrency are runtime defaults rather than Beale settings.
- Custom role definitions and agent instruction files are not implemented.
- Configured MCP support currently targets stdio servers.
- Tree-sitter code intelligence is structural assistance, not full semantic or taint analysis.
- Integration health checks are deterministic tests plus bounded real sessions, not a portable live-model CI job.
