use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Datelike, Months, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

mod schema;
/// The SQLite binding features use for their own tables and turn hooks.
pub use rusqlite;
pub use schema::{HOST_MIGRATIONS, HOST_OWNER};

#[derive(Clone)]
pub struct HostDatabase(Arc<Mutex<Connection>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionWorkspaceScope {
    pub all_workspaces: bool,
    pub workspace_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomPermissionRuleRow {
    pub id: String,
    pub stage: i32,
    pub title: String,
    pub instruction: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub created_at_ms: i64,
    pub last_message_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationJobRow {
    pub id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub title: String,
    pub repeat_preset: String,
    pub next_run_at_ms: i64,
    pub last_run_at_ms: i64,
    pub canceled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationOptions {
    pub reasoning_effort: i32,
    pub goal_mode: bool,
    pub goal_max_turns: u64,
    pub goal_max_duration_minutes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationRunRow {
    pub id: String,
    pub job_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub status: i32,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationClaim {
    pub run_id: String,
    pub job_id: String,
    pub workspace_id: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub prompt: String,
    pub options: AutomationOptions,
}

pub fn automation_next_run(preset: &str, anchor_ms: i64, occurrence: u32) -> Result<i64> {
    ensure!(occurrence > 0, "invalid automation occurrence");
    let interval: i64 = match preset {
        "minute" => 60_000,
        "hour" => 3_600_000,
        "day" => 86_400_000,
        "week" => 604_800_000,
        "month" => {
            let date = DateTime::<Utc>::from_timestamp_millis(anchor_ms)
                .context("invalid automation time")?;
            return date
                .checked_add_months(Months::new(occurrence))
                .map(|next| next.timestamp_millis())
                .context("automation time overflow");
        }
        _ => bail!("invalid automation repeat preset"),
    };
    anchor_ms
        .checked_add(
            interval
                .checked_mul(i64::from(occurrence))
                .context("automation time overflow")?,
        )
        .context("automation time overflow")
}

fn next_automation_after(
    preset: &str,
    anchor_ms: i64,
    minimum: u32,
    now_ms: i64,
) -> Result<(u32, i64)> {
    let mut occurrence = minimum.max(1);
    if preset == "month" {
        let anchor = DateTime::<Utc>::from_timestamp_millis(anchor_ms)
            .context("invalid automation anchor")?;
        let now = DateTime::<Utc>::from_timestamp_millis(now_ms).context("invalid current time")?;
        let months = (now.year() - anchor.year()) * 12 + now.month() as i32 - anchor.month() as i32;
        occurrence = occurrence.max(u32::try_from(months.max(0))?);
    } else {
        let interval = automation_next_run(preset, 0, 1)?;
        if now_ms >= anchor_ms {
            let elapsed = now_ms
                .checked_sub(anchor_ms)
                .context("automation time overflow")?;
            occurrence = occurrence.max(u32::try_from(elapsed / interval + 1)?);
        }
    }
    loop {
        let due = automation_next_run(preset, anchor_ms, occurrence)?;
        if due > now_ms {
            return Ok((occurrence, due));
        }
        occurrence = occurrence
            .checked_add(1)
            .context("automation occurrence overflow")?;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRow {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub model: String,
    pub turn_count: i64,
    pub created_at_ms: i64,
    pub archived_at_ms: i64,
    pub quick_chat: bool,
    pub last_message_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnRow {
    pub number: i64,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub prompt: String,
    pub answer: String,
    pub duration_ms: i64,
    pub internal_prompt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildTurnRow {
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub result: String,
    pub is_error: bool,
    pub messages_json: String,
    pub reasoning_json: String,
    pub completed_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionChildRow {
    pub parent_role: String,
    pub turn: ChildTurnRow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionChildSummaryRow {
    pub parent_role: String,
    pub agent_id: String,
    pub role: String,
    pub task: String,
    pub is_error: bool,
    pub completed_at_ms: i64,
}

impl From<&SessionChildRow> for SessionChildSummaryRow {
    fn from(row: &SessionChildRow) -> Self {
        Self {
            parent_role: row.parent_role.clone(),
            agent_id: row.turn.agent_id.clone(),
            role: row.turn.role.clone(),
            task: row.turn.task.clone(),
            is_error: row.turn.is_error,
            completed_at_ms: row.turn.completed_at_ms,
        }
    }
}

fn session_child_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionChildRow> {
    Ok(SessionChildRow {
        parent_role: row.get(0)?,
        turn: ChildTurnRow {
            agent_id: row.get(1)?,
            role: row.get(2)?,
            task: row.get(3)?,
            result: row.get(4)?,
            is_error: row.get(5)?,
            messages_json: row.get(6)?,
            reasoning_json: row.get(7)?,
            completed_at_ms: row.get(8)?,
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildAssistantMessageRow {
    pub parent_turn: i64,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsageRow {
    pub agent_id: String,
    pub role: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub work_elapsed_ms: Option<i64>,
    pub duration_ms: i64,
    pub observed_at_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionUsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub measured_calls: i64,
    pub total_calls: i64,
    pub cache_read_tokens: i64,
    pub cache_input_tokens: i64,
    pub cache_measured_calls: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionUsageSnapshotRow {
    pub work_duration_ms: u64,
    pub totals: SessionUsageTotals,
    pub main_totals: SessionUsageTotals,
    pub subagent_totals: SessionUsageTotals,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelUsageRow {
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub role: String,
    pub totals: SessionUsageTotals,
}

fn is_top_level_role(role: &str) -> bool {
    matches!(
        role,
        "main"
            | "discovery_harness"
            | "validation_system"
            | "discovery"
            | "validation"
            | "reporter"
            | "patch_generation"
    )
}

fn add_usage_totals(total: &mut SessionUsageTotals, delta: &SessionUsageTotals) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.measured_calls = total.measured_calls.saturating_add(delta.measured_calls);
    total.total_calls = total.total_calls.saturating_add(delta.total_calls);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(delta.cache_read_tokens);
    total.cache_input_tokens = total
        .cache_input_tokens
        .saturating_add(delta.cache_input_tokens);
    total.cache_measured_calls = total
        .cache_measured_calls
        .saturating_add(delta.cache_measured_calls);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallRow {
    pub id: String,
    pub turn_number: i64,
    pub ordinal: i64,
    pub name: String,
    pub summary: String,
    pub arguments: String,
    pub output: String,
    pub is_error: bool,
    pub agent_role: String,
    /// Unix milliseconds when the call finished; zero for calls saved before schema 49.
    pub created_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallSummaryRow {
    pub id: String,
    pub turn_number: i64,
    pub ordinal: i64,
    pub name: String,
    pub summary: String,
    pub is_error: bool,
    pub agent_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasoningSummaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
    pub agent_role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentaryRow {
    pub turn_number: i64,
    pub ordinal: i64,
    pub after_tool_count: i64,
    pub text: String,
    pub agent_role: String,
}

pub struct TurnActivity<'a> {
    pub tools: &'a [ToolCallRow],
    pub reasoning: &'a [ReasoningSummaryRow],
    pub commentary: &'a [CommentaryRow],
}

#[derive(Default)]
pub struct TurnMetadata<'a> {
    pub provider_context_json: Option<&'a str>,
    /// Top-level role of a peer agent session turn; `None` for ordinary turns.
    pub agent_role: Option<&'a str>,
    pub model_usage: Option<&'a [ModelUsageRow]>,
    pub child_turns: &'a [ChildTurnRow],
    /// Feature rows committed in the same transaction as the turn.
    pub extensions: &'a [&'a dyn TurnExtension],
}

/// The committed turn a feature extension writes against.
pub struct CommittedTurn<'a> {
    pub session: &'a SessionRow,
    pub turn_number: i64,
    pub new_session: bool,
    pub agent_role: Option<&'a str>,
    pub child_turns: &'a [ChildTurnRow],
}

/// Feature-owned rows saved atomically with a turn, such as Goal Mode state,
/// graph configuration, or a memory extraction job. Extensions run after the
/// host rows are written and before the transaction commits; an error rolls
/// back the whole turn.
pub trait TurnExtension {
    fn commit(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        turn: &CommittedTurn<'_>,
    ) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct SessionWithTurns {
    pub session: SessionRow,
    pub turns: Vec<TurnRow>,
}

impl HostDatabase {
    pub fn protect_database_file(root: &Path, path: &Path) -> Result<()> {
        private_root(root)?;
        ensure!(
            path.parent() == Some(root),
            "database path must be beneath app-server root"
        );
        #[cfg(windows)]
        restrict_windows_acl(path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .context("restrict database file")?;
        }
        Ok(())
    }

    pub fn open(root: &Path) -> Result<Self> {
        private_root(root)?;
        let path = root.join("state.sqlite");
        let mut connection = Connection::open(&path).context("open host database")?;
        #[cfg(windows)]
        restrict_windows_acl(&path, false)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .context("restrict host database")?;
        }
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        schema::reject_legacy(&connection)?;
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")?;
        schema::ensure_registry(&connection)?;
        schema::apply(&mut connection, schema::HOST_OWNER, schema::HOST_MIGRATIONS)?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub fn session_model_usage(&self, session_id: &str) -> Result<Vec<ModelUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT agent_id, role, provider, method, model, input_tokens, output_tokens,
                        duration_ms, observed_at_ms, cache_read_tokens, work_elapsed_ms
                 FROM session_model_usage WHERE session_id = ?1
                 ORDER BY turn_number, ordinal LIMIT 2001",
            )?;
            let rows = statement
                .query_map([session_id], |row| {
                    Ok(ModelUsageRow {
                        agent_id: row.get(0)?,
                        role: row.get(1)?,
                        provider: row.get(2)?,
                        method: row.get(3)?,
                        model: row.get(4)?,
                        input_tokens: row.get(5)?,
                        output_tokens: row.get(6)?,
                        duration_ms: row.get(7)?,
                        observed_at_ms: row.get(8)?,
                        cache_read_tokens: row.get(9)?,
                        work_elapsed_ms: row.get(10)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 2000, "too many session model usage rows");
            Ok(rows)
        })
    }

    pub fn session_usage_totals(&self, session_id: &str) -> Result<SessionUsageTotals> {
        self.session_usage_totals_for_role(session_id, None)
    }

    pub fn session_usage_totals_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<SessionUsageTotals> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COALESCE(SUM(COALESCE(input_tokens, 0)), 0),
                        COALESCE(SUM(COALESCE(output_tokens, 0)), 0),
                        COUNT(CASE WHEN input_tokens IS NOT NULL AND output_tokens IS NOT NULL THEN 1 END),
                        COUNT(*),
                        COALESCE(SUM(COALESCE(cache_read_tokens, 0)), 0),
                        COALESCE(SUM(CASE WHEN cache_read_tokens IS NOT NULL THEN input_tokens ELSE 0 END), 0),
                        COUNT(cache_read_tokens)
                 FROM session_model_usage m JOIN session_turns t
                   ON t.session_id=m.session_id AND t.number=m.turn_number
                 WHERE m.session_id = ?1 AND (?2 IS NULL OR t.agent_role=?2)",
                params![session_id, role],
                |row| Ok(SessionUsageTotals {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    measured_calls: row.get(2)?,
                    total_calls: row.get(3)?,
                    cache_read_tokens: row.get(4)?,
                    cache_input_tokens: row.get(5)?,
                    cache_measured_calls: row.get(6)?,
                }),
            )?)
        })
    }

    pub fn session_model_totals(&self, session_id: &str) -> Result<Vec<SessionModelUsageRow>> {
        self.session_model_totals_for_role(session_id, None)
    }

    pub fn session_model_totals_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<Vec<SessionModelUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.provider, m.method, m.model, m.role,
                        SUM(COALESCE(m.input_tokens, 0)), SUM(COALESCE(m.output_tokens, 0)),
                        COUNT(CASE WHEN m.input_tokens IS NOT NULL AND m.output_tokens IS NOT NULL THEN 1 END), COUNT(*),
                        SUM(COALESCE(m.cache_read_tokens, 0)),
                        SUM(CASE WHEN m.cache_read_tokens IS NOT NULL THEN COALESCE(m.input_tokens, 0) ELSE 0 END),
                        COUNT(m.cache_read_tokens)
                 FROM session_model_usage m JOIN session_turns t
                   ON t.session_id=m.session_id AND t.number=m.turn_number
                 WHERE m.session_id = ?1 AND (?2 IS NULL OR t.agent_role=?2)
                 GROUP BY m.provider, m.method, m.model, m.role
                 ORDER BY m.provider, m.method, m.model, m.role LIMIT 1025",
            )?;
            let rows = statement.query_map(params![session_id, role], |row| Ok(SessionModelUsageRow {
                provider: row.get(0)?, method: row.get(1)?, model: row.get(2)?, role: row.get(3)?,
                totals: SessionUsageTotals {
                    input_tokens: row.get(4)?, output_tokens: row.get(5)?, measured_calls: row.get(6)?,
                    total_calls: row.get(7)?, cache_read_tokens: row.get(8)?, cache_input_tokens: row.get(9)?, cache_measured_calls: row.get(10)?,
                },
            }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 1024, "too many session model sources");
            Ok(rows)
        })
    }

    pub fn session_usage_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionUsageSnapshotRow>> {
        self.session_usage_snapshots_for_role(session_id, None)
    }

    pub fn session_usage_snapshots_for_role(
        &self,
        session_id: &str,
        role: Option<&str>,
    ) -> Result<Vec<SessionUsageSnapshotRow>> {
        const INTERVAL_MS: u64 = 300_000;
        const MAX_SNAPSHOTS: u64 = 2_048;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "WITH turn_work AS (
                     SELECT number, duration_ms,
                            SUM(duration_ms) OVER (ORDER BY number ROWS UNBOUNDED PRECEDING) - duration_ms AS prior_ms
                     FROM session_turns t
                     WHERE t.session_id = ?1 AND (?2 IS NULL OR t.agent_role=?2)
                 )
                 SELECT t.prior_ms, t.duration_ms, m.ordinal, m.input_tokens,
                        m.output_tokens, m.cache_read_tokens, m.work_elapsed_ms, m.role
                 FROM turn_work t LEFT JOIN session_model_usage m
                   ON m.session_id = ?1 AND m.turn_number = t.number
                 ORDER BY t.number, m.ordinal",
            )?;
            let rows = statement.query_map(params![session_id, role], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })?;
            // All three scopes share the same work clock and sampling boundaries.
            let mut changes = BTreeMap::<u64, [SessionUsageTotals; 3]>::new();
            let mut total_work_ms = 0_u64;
            for row in rows {
                let (prior, duration, ordinal, input, output, cached, elapsed, role) = row?;
                let prior = u64::try_from(prior)?;
                let duration = u64::try_from(duration)?;
                total_work_ms = total_work_ms.max(prior.saturating_add(duration));
                if ordinal.is_none() {
                    continue;
                }
                let offset = elapsed.map_or(duration, |value| value.max(0) as u64).min(duration);
                let work_ms = prior.saturating_add(offset);
                let bucket = if work_ms == 0 { 0 } else { (work_ms - 1) / INTERVAL_MS + 1 };
                let mut delta = SessionUsageTotals::default();
                delta.total_calls += 1;
                if let (Some(input), Some(output)) = (input, output) {
                    delta.input_tokens = delta.input_tokens.saturating_add(input);
                    delta.output_tokens = delta.output_tokens.saturating_add(output);
                    delta.measured_calls += 1;
                }
                if let (Some(input), Some(cached)) = (input, cached) {
                    delta.cache_read_tokens = delta.cache_read_tokens.saturating_add(cached);
                    delta.cache_input_tokens = delta.cache_input_tokens.saturating_add(input);
                    delta.cache_measured_calls += 1;
                }
                let scopes = changes.entry(bucket).or_default();
                add_usage_totals(&mut scopes[0], &delta);
                match role.as_deref() {
                    Some(role) if is_top_level_role(role) => add_usage_totals(&mut scopes[1], &delta),
                    Some(role) if !role.is_empty() => add_usage_totals(&mut scopes[2], &delta),
                    _ => {}
                }
            }
            let last_bucket = if total_work_ms == 0 { 0 } else { (total_work_ms - 1) / INTERVAL_MS + 1 };
            let first_bucket = last_bucket.saturating_sub(MAX_SNAPSHOTS - 1);
            let mut totals: [SessionUsageTotals; 3] = Default::default();
            for delta in changes.range(..first_bucket).map(|(_, delta)| delta) {
                for (total, delta) in totals.iter_mut().zip(delta) {
                    add_usage_totals(total, delta);
                }
            }
            let mut snapshots = Vec::with_capacity((last_bucket - first_bucket + 1) as usize);
            for bucket in first_bucket..=last_bucket {
                if let Some(delta) = changes.get(&bucket) {
                    for (total, delta) in totals.iter_mut().zip(delta) {
                        add_usage_totals(total, delta);
                    }
                }
                snapshots.push(SessionUsageSnapshotRow {
                    work_duration_ms: bucket.saturating_mul(INTERVAL_MS).min(total_work_ms),
                    totals: totals[0].clone(),
                    main_totals: totals[1].clone(),
                    subagent_totals: totals[2].clone(),
                });
            }
            Ok(snapshots)
        })
    }

    pub fn session_last_top_level_role(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT role FROM session_model_usage WHERE session_id = ?1
                 AND role IN ('main','discovery_harness','validation_system','discovery','validation','reporter','patch_generation')
                 ORDER BY turn_number DESC, ordinal DESC LIMIT 1",
                [session_id],
                |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    pub fn session_turn_role(&self, session_id: &str, turn_number: i64) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT role FROM session_model_usage WHERE session_id = ?1 AND turn_number = ?2
                 AND role IN ('main','discovery_harness','validation_system','discovery','validation','reporter','patch_generation')
                 ORDER BY ordinal DESC LIMIT 1",
                params![session_id, turn_number],
                |row| row.get(0),
            ).optional().map_err(Into::into)
        })
    }

    pub fn provider_context(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT context_json FROM session_provider_context
                     WHERE session_id = ?1 AND agent_role = ''",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn graph_provider_context(&self, session_id: &str, role: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT context_json FROM session_provider_context
                     WHERE session_id=?1 AND agent_role=?2",
                    params![session_id, role],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn automation_jobs(&self) -> Result<Vec<AutomationJobRow>> {
        self.with_connection(|db| {
            let mut query = db.prepare(
                "SELECT id, session_id, workspace_id, title, repeat_preset,
                        next_run_at_ms, last_run_at_ms, canceled
                 FROM automation_jobs ORDER BY canceled, next_run_at_ms, title",
            )?;
            let rows = query.query_map([], |row| {
                Ok(AutomationJobRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    title: row.get(3)?,
                    repeat_preset: row.get(4)?,
                    next_run_at_ms: row.get(5)?,
                    last_run_at_ms: row.get(6)?,
                    canceled: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn automation_runs(&self) -> Result<Vec<AutomationRunRow>> {
        self.with_connection(|db| {
            let mut query = db.prepare(
                "SELECT id, job_id, session_id, title, started_at_ms, completed_at_ms, status, error
                 FROM automation_runs ORDER BY status = 1 DESC, started_at_ms DESC LIMIT 1024",
            )?;
            let rows = query.query_map([], |row| {
                Ok(AutomationRunRow {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    session_id: row.get(2)?,
                    title: row.get(3)?,
                    started_at_ms: row.get(4)?,
                    completed_at_ms: row.get(5)?,
                    status: row.get(6)?,
                    error: row.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn add_automation_job(&self, session: &SessionRow, preset: &str) -> Result<()> {
        let next = automation_next_run(preset, session.last_message_at_ms, 1)?;
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            transaction.execute(
                "INSERT INTO automation_jobs
                 (id, session_id, workspace_id, title, repeat_preset, next_run_at_ms, last_run_at_ms)
                 VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                params![session.id, session.workspace_id, session.title, preset, next, session.last_message_at_ms],
            )?;
            insert_initial_automation_run(&transaction, session, AutomationOptions::default())?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn cancel_automation(&self, id: &str, series: bool) -> Result<bool> {
        self.with_connection(|db| {
            let current: Option<(String, i64, i64)> = db
                .query_row(
                    "SELECT a.repeat_preset, o.anchor_at_ms, a.next_occurrence
                     FROM automation_jobs a JOIN automation_job_options o ON o.job_id = a.id
                     WHERE a.id = ?1 AND a.canceled = 0",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((preset, anchor, occurrence)) = current else { return Ok(false) };
            if series {
                db.execute("UPDATE automation_jobs SET canceled = 1 WHERE id = ?1", [id])?;
            } else {
                let next_occurrence = occurrence.checked_add(1).context("automation occurrence overflow")?;
                let next = automation_next_run(&preset, anchor, u32::try_from(next_occurrence)?)?;
                db.execute(
                    "UPDATE automation_jobs SET next_run_at_ms = ?2, next_occurrence = ?3 WHERE id = ?1",
                    params![id, next, next_occurrence],
                )?;
            }
            Ok(true)
        })
    }

    pub fn automation_job_canceled(&self, id: &str) -> Result<bool> {
        self.with_connection(|db| {
            db.query_row(
                "SELECT canceled FROM automation_jobs WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
    }

    pub fn claim_due_automation(&self, now_ms: i64) -> Result<Option<AutomationClaim>> {
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            type Due = (String, String, String, i64, i64, String, i32, i32, String, String, i32, bool, i64, i64);
            let due: Option<Due> = transaction.query_row(
                "SELECT a.id, a.workspace_id, a.repeat_preset, a.next_occurrence, o.anchor_at_ms,
                        a.title, t.provider, t.method, t.model, t.prompt,
                        o.reasoning_effort, o.goal_mode, o.goal_max_turns, o.goal_max_duration_minutes
                 FROM automation_jobs a
                 JOIN automation_job_options o ON o.job_id = a.id
                 JOIN session_turns t ON t.session_id = a.session_id AND t.number = 1
                 WHERE a.canceled = 0 AND a.next_run_at_ms <= ?1
                   AND NOT EXISTS (SELECT 1 FROM automation_runs r WHERE r.job_id = a.id AND r.status = 1)
                 ORDER BY a.next_run_at_ms LIMIT 1",
                [now_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                    row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                    row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?)),
            ).optional()?;
            let Some((job_id, workspace_id, preset, occurrence, anchor, title, provider, method,
                model, prompt, effort, goal_mode, max_turns, max_minutes)) = due else { return Ok(None) };
            let next_minimum = u32::try_from(occurrence.checked_add(1).context("automation occurrence overflow")?)?;
            let (next_occurrence, next_due) = next_automation_after(&preset, anchor, next_minimum, now_ms)?;
            let goal_max_turns = u64::try_from(max_turns)?;
            let goal_max_duration_minutes = u64::try_from(max_minutes)?;
            let run_id = uuid::Uuid::new_v4().to_string();
            transaction.execute(
                "UPDATE automation_jobs SET next_occurrence = ?2, next_run_at_ms = ?3 WHERE id = ?1",
                params![job_id, next_occurrence, next_due],
            )?;
            transaction.execute(
                "INSERT INTO automation_runs(id, job_id, title, started_at_ms, status, lease_expires_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
                params![run_id, job_id, title, now_ms, now_ms.checked_add(30_000).context("lease overflow")?],
            )?;
            transaction.commit()?;
            Ok(Some(AutomationClaim {
                run_id, job_id, workspace_id, provider, method, model, prompt,
                options: AutomationOptions {
                    reasoning_effort: effort, goal_mode,
                    goal_max_turns,
                    goal_max_duration_minutes,
                },
            }))
        })
    }

    pub fn finish_automation_run(
        &self,
        run_id: &str,
        session_id: Option<&str>,
        status: i32,
        error: &str,
        finished_at_ms: i64,
    ) -> Result<bool> {
        ensure!(matches!(status, 2 | 3), "invalid automation finish status");
        ensure!(error.len() <= 512, "automation error is too long");
        self.with_connection(|db| {
            let transaction = db.transaction()?;
            let changed = transaction.execute(
                "UPDATE automation_runs SET session_id = ?2, completed_at_ms = ?3, status = ?4,
                 error = ?5, lease_expires_at_ms = 0
                 WHERE id = ?1 AND status = 1",
                params![run_id, session_id, finished_at_ms, status, error],
            )?;
            if changed == 1 && status == 2 {
                transaction.execute(
                    "UPDATE automation_jobs SET last_run_at_ms = ?2 WHERE id =
                     (SELECT job_id FROM automation_runs WHERE id = ?1)",
                    params![run_id, finished_at_ms],
                )?;
            }
            transaction.commit()?;
            Ok(changed == 1)
        })
    }

    pub fn interrupt_automation_runs(&self, at_ms: i64) -> Result<usize> {
        self.with_connection(|db| {
            Ok(db.execute(
                "UPDATE automation_runs SET status = 4, completed_at_ms = ?1,
             lease_expires_at_ms = 0,
             error = 'The app-server stopped before this occurrence completed.'
             WHERE status = 1 AND lease_expires_at_ms <= ?1",
                [at_ms],
            )?)
        })
    }

    pub fn renew_automation_lease(&self, run_id: &str, at_ms: i64) -> Result<bool> {
        let expires = at_ms.checked_add(30_000).context("lease overflow")?;
        self.with_connection(|db| {
            Ok(db.execute(
                "UPDATE automation_runs SET lease_expires_at_ms = ?2 WHERE id = ?1 AND status = 1",
                params![run_id, expires],
            )? == 1)
        })
    }

    /// Runs an action on the shared connection. Feature stores use this for
    /// the tables they own.
    pub fn with_connection<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("host database lock poisoned"))?;
        action(&mut connection)
    }

    pub fn installed_plugin_names(&self) -> Result<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT name FROM plugin_installs ORDER BY name")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn install_plugin(&self, name: &str, installed_at_ms: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "INSERT OR IGNORE INTO plugin_installs(name, installed_at_ms) VALUES (?1, ?2)",
                params![name, installed_at_ms],
            )? > 0)
        })
    }

    pub fn uninstall_plugin(&self, name: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM plugin_installs WHERE name = ?1", [name])? > 0)
        })
    }

    pub fn workspaces(&self) -> Result<Vec<WorkspaceRow>> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT id, name, directory FROM workspaces ORDER BY ordinal")?;
            let rows = statement.query_map([], |row| {
                Ok(WorkspaceRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    directory: row.get(2)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn replace_workspaces(&self, rows: &[WorkspaceRow]) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("DELETE FROM workspaces", [])?;
            for (ordinal, row) in rows.iter().enumerate() {
                transaction.execute(
                    "INSERT INTO workspaces(id, name, directory, ordinal) VALUES (?1, ?2, ?3, ?4)",
                    params![row.id, row.name, row.directory, i64::try_from(ordinal)?],
                )?;
            }
            transaction.execute(
                "DELETE FROM extension_workspace_scope WHERE workspace_id NOT IN (SELECT id FROM workspaces)",
                [],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn sessions(&self) -> Result<Vec<SessionRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions
                 ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn sessions_excluding(&self, workspace_id: &str) -> Result<Vec<SessionRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions
                 WHERE workspace_id != ?1 AND archived_at_ms IS NULL
                 ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([workspace_id], session_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn archive_session(&self, id: &str, archived_at_ms: i64) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE sessions SET archived_at_ms = ?2
                 WHERE id = ?1 AND workspace_id != 'internal:quick-chats'",
                params![id, archived_at_ms],
            )? > 0)
        })
    }

    pub fn restore_session(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE sessions SET archived_at_ms = NULL
                 WHERE id = ?1 AND workspace_id != 'internal:quick-chats'
                 AND archived_at_ms IS NOT NULL",
                [id],
            )? > 0)
        })
    }

    pub fn archive_items(&self) -> Result<Vec<ArchiveRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.id, COALESCE(q.workspace_id, s.workspace_id), s.title,
                        COALESCE((SELECT model FROM session_turns
                                  WHERE session_id = s.id ORDER BY number DESC LIMIT 1), ''),
                        (SELECT COUNT(*) FROM session_turns WHERE session_id = s.id),
                        s.created_at_ms, s.archived_at_ms,
                        s.workspace_id = 'internal:quick-chats', s.last_message_at_ms
                 FROM sessions s
                 LEFT JOIN quick_chat_contexts q ON q.session_id = s.id
                 WHERE s.archived_at_ms IS NOT NULL
                 ORDER BY s.archived_at_ms DESC, s.id DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(ArchiveRow {
                    id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    title: row.get(2)?,
                    model: row.get(3)?,
                    turn_count: row.get(4)?,
                    created_at_ms: row.get(5)?,
                    archived_at_ms: row.get(6)?,
                    quick_chat: row.get(7)?,
                    last_message_at_ms: row.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn archived_quick_chat_ids(&self) -> Result<Vec<String>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM sessions WHERE workspace_id = 'internal:quick-chats'
                 AND archived_at_ms IS NOT NULL ORDER BY created_at_ms DESC LIMIT 1025",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionWithTurns>> {
        self.with_connection(|connection| {
            let session = connection
                .query_row(
                    "SELECT id, workspace_id, title, created_at_ms, last_message_at_ms FROM sessions WHERE id = ?1",
                    [id],
                    session_from_row,
                )
                .optional()?;
            let Some(session) = session else {
                return Ok(None);
            };
            let mut statement = connection.prepare(
                "SELECT number, provider, method, model, prompt, answer, duration_ms, internal_prompt
                 FROM (SELECT number, provider, method, model, prompt, answer, duration_ms, internal_prompt
                       FROM session_turns WHERE session_id = ?1 ORDER BY number DESC LIMIT 64)
                 ORDER BY number",
            )?;
            let rows = statement.query_map([id], |row| {
                Ok(TurnRow {
                    number: row.get(0)?,
                    provider: row.get(1)?,
                    method: row.get(2)?,
                    model: row.get(3)?,
                    prompt: row.get(4)?,
                    answer: row.get(5)?,
                    duration_ms: row.get(6)?,
                    internal_prompt: row.get(7)?,
                })
            })?;
            Ok(Some(SessionWithTurns {
                session,
                turns: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            }))
        })
    }

    pub fn graph_agent_turns(&self, session_id: &str, role: &str) -> Result<Vec<TurnRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT number,provider,method,model,prompt,answer,duration_ms,internal_prompt
                 FROM (SELECT t.number,t.provider,t.method,t.model,t.prompt,t.answer,t.duration_ms,t.internal_prompt
                       FROM session_turns t
                       WHERE t.session_id=?1 AND t.agent_role=?2
                       ORDER BY t.number DESC LIMIT 64)
                 ORDER BY number",
            )?;
            statement.query_map(params![session_id,role], |row| Ok(TurnRow {
                number: row.get(0)?, provider: row.get(1)?, method: row.get(2)?,
                model: row.get(3)?, prompt: row.get(4)?, answer: row.get(5)?,
                duration_ms: row.get(6)?, internal_prompt: row.get(7)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn graph_recent_turns(&self, session_id: &str) -> Result<Vec<TurnRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT number,provider,method,model,prompt,answer,duration_ms,internal_prompt
                 FROM (SELECT t.number,t.provider,t.method,t.model,t.prompt,t.answer,t.duration_ms,t.internal_prompt,
                         ROW_NUMBER() OVER (PARTITION BY t.agent_role ORDER BY t.number DESC) AS role_ordinal
                       FROM session_turns t
                       WHERE t.session_id=?1 AND t.agent_role IS NOT NULL)
                 WHERE role_ordinal<=2 ORDER BY number",
            )?;
            statement
                .query_map([session_id], |row| {
                    Ok(TurnRow {
                        number: row.get(0)?, provider: row.get(1)?, method: row.get(2)?,
                        model: row.get(3)?, prompt: row.get(4)?, answer: row.get(5)?,
                        duration_ms: row.get(6)?, internal_prompt: row.get(7)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session_total_duration_ms(&self, id: &str) -> Result<u64> {
        self.with_connection(|connection| {
            let total: i64 = connection.query_row(
                "SELECT COALESCE(SUM(duration_ms), 0) FROM session_turns WHERE session_id = ?1",
                [id],
                |row| row.get(0),
            )?;
            u64::try_from(total).map_err(Into::into)
        })
    }

    pub fn session_agent_duration_ms(&self, session_id: &str, role: &str) -> Result<u64> {
        self.with_connection(|connection| {
            let total: i64 = connection.query_row(
                "SELECT COALESCE(SUM(t.duration_ms),0) FROM session_turns t
                 WHERE t.session_id=?1 AND t.agent_role=?2",
                params![session_id, role],
                |row| row.get(0),
            )?;
            u64::try_from(total).map_err(Into::into)
        })
    }

    pub fn session_has_campaign_activity(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_tool_calls
                 WHERE session_id = ?1 AND (name IN ('graph_delegate','graph_subagents_spawn','runbook_run')
                 OR (name = 'graph_handoff' AND is_error = 0)
                 OR (is_error = 0 AND name IN ('memory_create', 'memory_save', 'lead_create',
                     'finding_create', 'findings_create', 'claim_create', 'runbook_create', 'runbook_edit', 'surface_add', 'surface_save',
                     'surface_threat_model_edit'))))",
                    [id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    pub fn feature_enabled(&self, id: &str) -> Result<bool> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT enabled FROM feature_states WHERE id = ?1",
                    [id],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        })
    }

    pub fn extension_scope(&self, kind: &str, id: &str) -> Result<ExtensionWorkspaceScope> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        self.with_connection(|connection| {
            let all_workspaces = connection.query_row(
                "SELECT all_workspaces FROM extension_scope_modes WHERE kind = ?1 AND extension_id = ?2",
                params![kind, id],
                |row| row.get::<_, bool>(0),
            ).optional()?.unwrap_or(true);
            let mut statement = connection.prepare(
                "SELECT workspace_id FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2 ORDER BY workspace_id",
            )?;
            let rows = statement.query_map(params![kind, id], |row| row.get(0))?;
            Ok(ExtensionWorkspaceScope {
                all_workspaces,
                workspace_ids: rows.collect::<rusqlite::Result<Vec<_>>>()?,
            })
        })
    }

    pub fn set_extension_scope(
        &self,
        kind: &str,
        id: &str,
        all_workspaces: bool,
        ids: &[String],
    ) -> Result<()> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        ensure!(!id.is_empty() && id.len() <= 128, "invalid extension id");
        ensure!(ids.len() <= 128, "too many selected workspaces");
        ensure!(
            !all_workspaces || ids.is_empty(),
            "all-workspaces scope cannot include workspace ids"
        );
        let mut unique = HashSet::new();
        ensure!(
            ids.iter().all(|id| !id.is_empty() && unique.insert(id)),
            "duplicate or empty workspace id"
        );
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for workspace_id in ids {
                let exists: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
                    [workspace_id],
                    |row| row.get(0),
                )?;
                ensure!(exists, "unknown workspace");
            }
            transaction.execute(
                "DELETE FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2",
                params![kind, id],
            )?;
            transaction.execute(
                "INSERT INTO extension_scope_modes(kind, extension_id, all_workspaces) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, extension_id) DO UPDATE SET all_workspaces = excluded.all_workspaces",
                params![kind, id, all_workspaces],
            )?;
            for workspace_id in ids {
                transaction.execute(
                    "INSERT INTO extension_workspace_scope(kind, extension_id, workspace_id) VALUES (?1, ?2, ?3)",
                    params![kind, id, workspace_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn extension_applies_to(&self, kind: &str, id: &str, workspace_id: &str) -> Result<bool> {
        ensure!(
            matches!(kind, "plugin" | "feature"),
            "invalid extension kind"
        );
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE((SELECT all_workspaces FROM extension_scope_modes WHERE kind = ?1 AND extension_id = ?2), 1) = 1
                        OR EXISTS(SELECT 1 FROM extension_workspace_scope WHERE kind = ?1 AND extension_id = ?2 AND workspace_id = ?3)",
                params![kind, id, workspace_id],
                |row| row.get(0),
            ).map_err(Into::into)
        })
    }

    pub fn set_feature_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO feature_states(id, enabled) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled",
                params![id, enabled],
            )?;
            Ok(())
        })
    }

    /// Creates a session before its first turn, with an optional repeat job,
    /// and runs a feature writer in the same transaction.
    pub fn create_session(
        &self,
        session: &SessionRow,
        repeat: Option<(&str, AutomationOptions)>,
        extension: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<()>,
    ) -> Result<()> {
        let automation_next = repeat
            .map(|(preset, _)| automation_next_run(preset, session.last_message_at_ms, 1))
            .transpose()?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO sessions(id,workspace_id,title,created_at_ms,last_message_at_ms)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    session.id,
                    session.workspace_id,
                    session.title,
                    session.created_at_ms,
                    session.last_message_at_ms
                ],
            )?;
            if let (Some((preset, options)), Some(next)) = (repeat, automation_next) {
                transaction.execute(
                    "INSERT INTO automation_jobs
                     (id,session_id,workspace_id,title,repeat_preset,next_run_at_ms,last_run_at_ms)
                     VALUES (?1,?1,?2,?3,?4,?5,?6)",
                    params![
                        session.id,
                        session.workspace_id,
                        session.title,
                        preset,
                        next,
                        session.last_message_at_ms
                    ],
                )?;
                insert_initial_automation_run(&transaction, session, options)?;
            }
            extension(&transaction)?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn append_turn(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
    ) -> Result<()> {
        self.append_turn_with_tools(session, turn, new_session, &[])
    }

    pub fn quick_chat_workspace(&self, session_id: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT workspace_id FROM quick_chat_contexts WHERE session_id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn append_new_quick_chat_turn(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        workspace_id: &str,
    ) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO sessions(id, workspace_id, title, created_at_ms, archived_at_ms, last_message_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![session.id, session.workspace_id, session.title, session.created_at_ms, session.last_message_at_ms],
            )?;
            transaction.execute(
                "INSERT INTO quick_chat_contexts(session_id, workspace_id) VALUES (?1, ?2)",
                params![session.id, workspace_id],
            )?;
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms, internal_prompt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms, turn.internal_prompt],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn append_turn_with_tools(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
    ) -> Result<()> {
        self.append_turn_with_activity(session, turn, new_session, tools, &[])
    }

    pub fn append_turn_with_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
        reasoning: &[ReasoningSummaryRow],
    ) -> Result<()> {
        self.append_turn_with_full_activity(session, turn, new_session, tools, reasoning, &[])
    }

    pub fn append_turn_with_full_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        tools: &[ToolCallRow],
        reasoning: &[ReasoningSummaryRow],
        commentary: &[CommentaryRow],
    ) -> Result<()> {
        self.append_turn_with_goal_activity(
            session,
            turn,
            new_session,
            TurnActivity {
                tools,
                reasoning,
                commentary,
            },
        )
    }

    pub fn append_turn_with_goal_activity(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
    ) -> Result<()> {
        self.append_turn_with_goal_and_automation(session, turn, new_session, activity, None)
    }

    pub fn append_turn_with_goal_and_automation(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
        repeat_preset: Option<(&str, AutomationOptions)>,
    ) -> Result<()> {
        self.append_turn_with_metadata(
            session,
            turn,
            new_session,
            activity,
            repeat_preset,
            TurnMetadata::default(),
        )
    }

    pub fn append_turn_with_metadata(
        &self,
        session: &SessionRow,
        turn: &TurnRow,
        new_session: bool,
        activity: TurnActivity<'_>,
        repeat_preset: Option<(&str, AutomationOptions)>,
        metadata: TurnMetadata<'_>,
    ) -> Result<()> {
        let TurnMetadata {
            provider_context_json,
            agent_role,
            model_usage,
            child_turns,
            extensions,
        } = metadata;
        ensure!(
            provider_context_json.is_none_or(|value| value.len() <= 8_388_608),
            "provider context is too large"
        );
        ensure!(
            repeat_preset.is_none() || new_session,
            "repeat requires a new session"
        );
        ensure!(
            agent_role.is_none_or(|role| !role.is_empty() && role.len() <= 64),
            "invalid agent role"
        );
        let automation_next = repeat_preset
            .map(|(preset, _)| automation_next_run(preset, session.last_message_at_ms, 1))
            .transpose()?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if new_session {
                transaction.execute(
                    "INSERT INTO sessions(id, workspace_id, title, created_at_ms, last_message_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![session.id, session.workspace_id, session.title, session.created_at_ms, session.last_message_at_ms],
                )?;
            }
            transaction.execute(
                "INSERT INTO session_turns(session_id, number, provider, method, model, prompt, answer, duration_ms, internal_prompt, agent_role)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![session.id, turn.number, turn.provider, turn.method, turn.model, turn.prompt, turn.answer, turn.duration_ms, turn.internal_prompt, agent_role],
            )?;
            if let Some(model_usage) = model_usage {
                for (ordinal, item) in model_usage.iter().enumerate() {
                    ensure!(!item.agent_id.is_empty() && item.agent_id.len() <= 64
                        && item.model.len() <= 200 && item.role.len() <= 20
                        && item.duration_ms >= 0 && item.observed_at_ms >= 0
                        && item.input_tokens.is_none_or(|value| (0..=1_000_000_000_000).contains(&value))
                        && item.output_tokens.is_none_or(|value| (0..=1_000_000_000_000).contains(&value))
                        && item.cache_read_tokens.is_none_or(|cached| item.input_tokens.is_some_and(|input| (0..=input).contains(&cached)))
                        && item.work_elapsed_ms.is_none_or(|value| (0..=86_400_000).contains(&value)),
                        "invalid model usage");
                    transaction.execute(
                        "INSERT INTO session_model_usage
                         (session_id, turn_number, ordinal, agent_id, role, provider, method, model,
                          input_tokens, output_tokens, duration_ms, observed_at_ms, cache_read_tokens, work_elapsed_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![session.id, turn.number, ordinal as i64, item.agent_id, item.role,
                            item.provider, item.method, item.model, item.input_tokens,
                            item.output_tokens, item.duration_ms, item.observed_at_ms,
                            item.cache_read_tokens, item.work_elapsed_ms],
                    )?;
                }
            }
            if !new_session {
                transaction.execute(
                    "UPDATE sessions SET last_message_at_ms = MAX(last_message_at_ms, ?2) WHERE id = ?1",
                    params![session.id, session.last_message_at_ms],
                )?;
            }
            let context_role = agent_role.unwrap_or_default();
            if let Some(context_json) = provider_context_json {
                transaction.execute(
                    "INSERT INTO session_provider_context(session_id, agent_role, context_json)
                     VALUES (?1, ?2, ?3) ON CONFLICT(session_id, agent_role)
                     DO UPDATE SET context_json = excluded.context_json",
                    params![session.id, context_role, context_json],
                )?;
            } else if agent_role.is_some() {
                transaction.execute(
                    "DELETE FROM session_provider_context WHERE session_id=?1 AND agent_role=?2",
                    params![session.id, context_role],
                )?;
            }
            for tool in activity.tools {
                transaction.execute(
                    "INSERT INTO session_tool_calls(id, session_id, turn_number, ordinal, name, summary, arguments, output, is_error, agent_role, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![tool.id, session.id, turn.number, tool.ordinal, tool.name, tool.summary, tool.arguments, tool.output, tool.is_error, tool.agent_role, tool.created_at_ms],
                )?;
            }
            for child in child_turns {
                ensure!(
                    !child.agent_id.is_empty() && child.agent_id.len() <= 64
                        && child.role.len() <= 32 && child.task.len() <= 16_384
                        && child.completed_at_ms >= 0,
                    "invalid child transcript"
                );
                transaction.execute(
                    "INSERT INTO session_child_turns(session_id,parent_turn,agent_id,role,task,result,is_error,messages_json,reasoning_json,completed_at_ms)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![session.id, turn.number, child.agent_id, child.role, child.task,
                        child.result, child.is_error, child.messages_json, child.reasoning_json,
                        child.completed_at_ms],
                )?;
                let messages: serde_json::Value = serde_json::from_str(&child.messages_json)?;
                let messages = messages.as_array().context("child messages must be an array")?;
                for message in messages {
                    if message.get("role").and_then(serde_json::Value::as_str) != Some("tool") { continue; }
                    let id = message.get("tool_call_id").and_then(serde_json::Value::as_str)
                        .context("child tool message needs a tool call ID")?;
                    transaction.execute(
                        "INSERT INTO session_child_tools(tool_call_id,agent_id) VALUES (?1,?2)",
                        params![id,child.agent_id],
                    )?;
                }
            }
            for item in activity.reasoning {
                transaction.execute(
                    "INSERT INTO session_reasoning_summaries(session_id, turn_number, ordinal, after_tool_count, text, agent_role)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text, item.agent_role],
                )?;
            }
            for item in activity.commentary {
                transaction.execute(
                    "INSERT INTO session_commentary(session_id, turn_number, ordinal, after_tool_count, text, agent_role)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, turn.number, item.ordinal, item.after_tool_count, item.text, item.agent_role],
                )?;
            }
            let committed = CommittedTurn {
                session,
                turn_number: turn.number,
                new_session,
                agent_role,
                child_turns,
            };
            for extension in extensions {
                extension.commit(&transaction, &committed)?;
            }
            if let (Some((preset, options)), Some(next)) = (repeat_preset, automation_next) {
                transaction.execute(
                    "INSERT INTO automation_jobs
                     (id, session_id, workspace_id, title, repeat_preset, next_run_at_ms, last_run_at_ms)
                     VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6)",
                    params![session.id, session.workspace_id, session.title, preset, next, session.last_message_at_ms],
                )?;
                insert_initial_automation_run(&transaction, session, options)?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn tool_summaries(&self, session_id: &str) -> Result<Vec<ToolCallSummaryRow>> {
        self.tool_summaries_since(session_id, 1)
    }

    pub fn tool_summaries_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<ToolCallSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM
                 (SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM session_tool_calls
                  WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number DESC, ordinal DESC LIMIT 4097)
                 ORDER BY turn_number, ordinal",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(ToolCallSummaryRow {
                    id: row.get(0)?,
                    turn_number: row.get(1)?,
                    ordinal: row.get(2)?,
                    name: row.get(3)?,
                    summary: row.get(4)?,
                    is_error: row.get(5)?,
                    agent_role: row.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn tool_summaries_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ToolCallSummaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM
                 (SELECT id, turn_number, ordinal, name, summary, is_error, agent_role FROM session_tool_calls
                  WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                  ORDER BY turn_number DESC, ordinal DESC LIMIT 4097)
                 ORDER BY turn_number, ordinal",
            )?;
            statement
                .query_map(params![session_id, turns_json], |row| {
                    Ok(ToolCallSummaryRow {
                        id: row.get(0)?,
                        turn_number: row.get(1)?,
                        ordinal: row.get(2)?,
                        name: row.get(3)?,
                        summary: row.get(4)?,
                        is_error: row.get(5)?,
                        agent_role: row.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn reasoning_summaries(&self, session_id: &str) -> Result<Vec<ReasoningSummaryRow>> {
        self.reasoning_summaries_since(session_id, 1)
    }

    pub fn reasoning_summaries_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<ReasoningSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_reasoning_summaries
                 WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(ReasoningSummaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                    agent_role: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn reasoning_summaries_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ReasoningSummaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_reasoning_summaries
                 WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            statement.query_map(params![session_id, turns_json], |row| Ok(ReasoningSummaryRow {
                turn_number: row.get(0)?, ordinal: row.get(1)?, after_tool_count: row.get(2)?,
                text: row.get(3)?, agent_role: row.get(4)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn commentary(&self, session_id: &str) -> Result<Vec<CommentaryRow>> {
        self.commentary_since(session_id, 1)
    }

    pub fn commentary_since(
        &self,
        session_id: &str,
        first_turn: i64,
    ) -> Result<Vec<CommentaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_commentary
                 WHERE session_id = ?1 AND turn_number >= ?2 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            let rows = statement.query_map(params![session_id, first_turn], |row| {
                Ok(CommentaryRow {
                    turn_number: row.get(0)?,
                    ordinal: row.get(1)?,
                    after_tool_count: row.get(2)?,
                    text: row.get(3)?,
                    agent_role: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn commentary_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<CommentaryRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT turn_number, ordinal, after_tool_count, text, agent_role FROM session_commentary
                 WHERE session_id=?1 AND turn_number IN (SELECT value FROM json_each(?2))
                 ORDER BY turn_number, ordinal LIMIT 8193",
            )?;
            statement.query_map(params![session_id, turns_json], |row| Ok(CommentaryRow {
                turn_number: row.get(0)?, ordinal: row.get(1)?, after_tool_count: row.get(2)?,
                text: row.get(3)?, agent_role: row.get(4)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn child_assistant_messages_for_turns(
        &self,
        session_id: &str,
        turns: &[i64],
    ) -> Result<Vec<ChildAssistantMessageRow>> {
        let turns_json = serde_json::to_string(turns)?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT parent_turn, role, text FROM (
                   SELECT c.parent_turn, c.role, json_extract(m.value,'$.text') AS text,
                          c.completed_at_ms AS completed_at, CAST(m.key AS INTEGER) AS message_index
                   FROM session_child_turns c, json_each(c.messages_json) m
                   WHERE c.session_id=?1
                     AND c.parent_turn IN (SELECT value FROM json_each(?2))
                     AND json_extract(m.value,'$.role')='assistant'
                     AND json_type(m.value,'$.text')='text'
                   ORDER BY c.parent_turn DESC, completed_at DESC, message_index DESC LIMIT 256
                 ) ORDER BY parent_turn, completed_at, message_index",
            )?;
            statement
                .query_map(params![session_id, turns_json], |row| {
                    Ok(ChildAssistantMessageRow {
                        parent_turn: row.get(0)?,
                        role: row.get(1)?,
                        text: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn session_children(&self, session_id: &str) -> Result<Vec<SessionChildSummaryRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT COALESCE(t.agent_role,'main'),c.agent_id,c.role,c.task,c.is_error,c.completed_at_ms
                 FROM session_child_turns c
                 LEFT JOIN session_turns t ON t.session_id=c.session_id AND t.number=c.parent_turn
                 WHERE c.session_id=?1 ORDER BY c.completed_at_ms DESC,c.agent_id LIMIT 1024",
            )?;
            statement.query_map(params![session_id], |row| Ok(SessionChildSummaryRow {
                parent_role: row.get(0)?, agent_id: row.get(1)?, role: row.get(2)?,
                task: row.get(3)?, is_error: row.get(4)?, completed_at_ms: row.get(5)?,
            }))?
                .collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    pub fn session_child(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<Option<SessionChildRow>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(t.agent_role,'main'),c.agent_id,c.role,c.task,c.result,
                        c.is_error,c.messages_json,c.reasoning_json,c.completed_at_ms
                 FROM session_child_turns c
                 LEFT JOIN session_turns t ON t.session_id=c.session_id AND t.number=c.parent_turn
                 WHERE c.session_id=?1 AND c.agent_id=?2",
                    params![session_id, agent_id],
                    session_child_row,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn tool_call(&self, session_id: &str, id: &str) -> Result<Option<ToolCallRow>> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, turn_number, ordinal, name, summary, arguments, output, is_error, agent_role, created_at_ms
                 FROM session_tool_calls WHERE session_id = ?1 AND id = ?2",
                    params![session_id, id],
                    |row| {
                        Ok(ToolCallRow {
                            id: row.get(0)?,
                            turn_number: row.get(1)?,
                            ordinal: row.get(2)?,
                            name: row.get(3)?,
                            summary: row.get(4)?,
                            arguments: row.get(5)?,
                            output: row.get(6)?,
                            is_error: row.get(7)?,
                            agent_role: row.get(8)?,
                            created_at_ms: row.get(9)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT value FROM provider_settings WHERE key = ?1",
                    [key],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    pub fn permission_rule(&self, id: &str) -> Result<Option<(String, String, bool)>> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT title, instruction, enabled FROM permission_rules WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
                )
                .optional()?)
        })
    }

    pub fn custom_permission_rules(&self) -> Result<Vec<CustomPermissionRuleRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, stage, title, instruction, enabled FROM permission_rules
                 WHERE stage IS NOT NULL ORDER BY stage, position, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(CustomPermissionRuleRow {
                    id: row.get(0)?,
                    stage: row.get(1)?,
                    title: row.get(2)?,
                    instruction: row.get(3)?,
                    enabled: row.get::<_, i64>(4)? != 0,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Into::into)
        })
    }

    pub fn create_custom_permission_rule(&self, rule: &CustomPermissionRuleRow) -> Result<()> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM permission_rules WHERE stage IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            ensure!(count < 11, "permission rule catalog is full");
            let position: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(position), 0) + 1 FROM permission_rules WHERE stage = ?1",
                [rule.stage],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO permission_rules(id, title, instruction, enabled, stage, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    rule.id,
                    rule.title,
                    rule.instruction,
                    i64::from(rule.enabled),
                    rule.stage,
                    position
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn save_permission_rule(
        &self,
        id: &str,
        title: &str,
        instruction: &str,
        enabled: bool,
    ) -> Result<()> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO permission_rules(id, title, instruction, enabled) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET title = excluded.title, instruction = excluded.instruction, enabled = excluded.enabled",
                params![id, title, instruction, i64::from(enabled)],
            )?;
            Ok(())
        })
    }

    pub fn save_setting(&self, key: &str, value: Option<&str>) -> Result<()> {
        self.with_connection(|connection| {
            if let Some(value) = value {
                connection.execute(
                    "INSERT INTO provider_settings(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
            } else {
                connection.execute("DELETE FROM provider_settings WHERE key = ?1", [key])?;
            }
            Ok(())
        })
    }
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        workspace_id: row.get(1)?,
        title: row.get(2)?,
        created_at_ms: row.get(3)?,
        last_message_at_ms: row.get(4)?,
    })
}

fn insert_initial_automation_run(
    transaction: &rusqlite::Transaction<'_>,
    session: &SessionRow,
    options: AutomationOptions,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO automation_job_options
         (job_id, reasoning_effort, goal_mode, goal_max_turns, goal_max_duration_minutes, anchor_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![session.id, options.reasoning_effort, options.goal_mode,
            i64::try_from(options.goal_max_turns)?, i64::try_from(options.goal_max_duration_minutes)?,
            session.last_message_at_ms],
    )?;
    transaction.execute(
        "INSERT INTO automation_runs
         (id, job_id, session_id, title, started_at_ms, completed_at_ms, status)
         VALUES (?1, ?1, ?1, ?2, ?3, ?4, 2)",
        params![
            session.id,
            session.title,
            session.created_at_ms,
            session.last_message_at_ms
        ],
    )?;
    Ok(())
}

fn private_root(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root).context("create private host data directory")?;
    #[cfg(windows)]
    restrict_windows_acl(root, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .context("restrict host data directory")?;
    }
    Ok(())
}

#[cfg(windows)]
fn restrict_windows_acl(path: &Path, directory: bool) -> Result<()> {
    use std::process::Command;
    let user = std::env::var("USERNAME").context("Windows user is unavailable")?;
    let domain = std::env::var("USERDOMAIN").context("Windows domain is unavailable")?;
    let principal = format!("{domain}\\{user}");
    let permission = if directory {
        format!("{principal}:(OI)(CI)F")
    } else {
        format!("{principal}:F")
    };
    let status = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(permission)
        .output()
        .context("cannot set private Windows ACL")?;
    if !status.status.success() {
        bail!("cannot set private Windows ACL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// Writes a marker row to a test-owned table with each committed turn.
    struct ExampleMarker(&'static str);

    impl TurnExtension for ExampleMarker {
        fn commit(
            &self,
            transaction: &rusqlite::Transaction<'_>,
            turn: &CommittedTurn<'_>,
        ) -> Result<()> {
            ensure!(self.0 != "reject", "example extension rejected the turn");
            transaction.execute(
                "INSERT INTO example_markers(session_id, turn_number, label) VALUES (?1, ?2, ?3)",
                params![turn.session.id, turn.turn_number, self.0],
            )?;
            Ok(())
        }
    }

    const EXAMPLE_MIGRATIONS: &[&str] = &[
        "CREATE TABLE example_markers (session_id TEXT NOT NULL, turn_number INTEGER NOT NULL, label TEXT NOT NULL);",
    ];

    fn example_markers(db: &HostDatabase) -> Vec<String> {
        db.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT label FROM example_markers ORDER BY turn_number")?;
            Ok(statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?)
        })
        .unwrap()
    }

    #[test]
    fn owner_schemas_apply_once_and_reject_newer_versions() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        assert_eq!(
            db.schema_version(HOST_OWNER).unwrap(),
            HOST_MIGRATIONS.len() as i64
        );
        assert_eq!(db.schema_version("example").unwrap(), 0);
        db.migrate_schema("example", EXAMPLE_MIGRATIONS).unwrap();
        db.migrate_schema("example", EXAMPLE_MIGRATIONS).unwrap();
        assert_eq!(db.schema_version("example").unwrap(), 1);
        assert!(db.migrate_schema("example", &[]).is_err());
        assert!(db.migrate_schema(HOST_OWNER, &[]).is_err());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn earlier_development_databases_are_rejected_without_changes() {
        let root = root();
        std::fs::create_dir_all(&root).unwrap();
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        connection
            .execute_batch("CREATE TABLE workspaces(id TEXT); PRAGMA user_version = 49;")
            .unwrap();
        drop(connection);
        let error = HostDatabase::open(&root).err().unwrap().to_string();
        assert!(
            error.contains("earlier Beale v2 development build"),
            "{error}"
        );
        let connection = Connection::open(root.join("state.sqlite")).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 49);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn turn_extensions_commit_or_roll_back_with_the_turn() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.migrate_schema("example", EXAMPLE_MIGRATIONS).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "example-model".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        let activity = || TurnActivity {
            tools: &[],
            reasoning: &[],
            commentary: &[],
        };
        assert!(
            db.append_turn_with_metadata(
                &session,
                &turn,
                true,
                activity(),
                None,
                TurnMetadata {
                    extensions: &[&ExampleMarker("kept"), &ExampleMarker("reject")],
                    ..TurnMetadata::default()
                },
            )
            .is_err()
        );
        assert!(db.session(&session.id).unwrap().is_none());
        assert!(example_markers(&db).is_empty());
        db.append_turn_with_metadata(
            &session,
            &turn,
            true,
            activity(),
            None,
            TurnMetadata {
                agent_role: Some("discovery"),
                provider_context_json: Some("[]"),
                extensions: &[&ExampleMarker("kept")],
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        assert_eq!(example_markers(&db), ["kept"]);
        assert_eq!(
            db.graph_agent_turns(&session.id, "discovery")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.graph_provider_context(&session.id, "discovery")
                .unwrap()
                .as_deref(),
            Some("[]")
        );
        assert!(db.provider_context(&session.id).unwrap().is_none());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
    use super::*;

    #[test]
    fn long_graph_turn_commits_usage_and_child_transcript_without_work_quota() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example research".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        let usage = (0..513)
            .map(|ordinal| ModelUsageRow {
                agent_id: "scout-example".to_owned(),
                role: "scout".to_owned(),
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                input_tokens: Some(100),
                output_tokens: Some(10),
                cache_read_tokens: None,
                work_elapsed_ms: Some(ordinal * 1_000),
                duration_ms: 100,
                observed_at_ms: ordinal * 1_000 + 1,
            })
            .collect::<Vec<_>>();
        let child = ChildTurnRow {
            agent_id: "scout-example".to_owned(),
            role: "scout".to_owned(),
            task: "Inspect a synthetic component".to_owned(),
            result: "x".repeat(65_537),
            is_error: false,
            messages_json: serde_json::json!([{"role":"assistant","text":"x".repeat(1_048_577)}])
                .to_string(),
            reasoning_json: serde_json::json!(["x".repeat(131_073)]).to_string(),
            completed_at_ms: 513_001,
        };
        db.append_turn_with_metadata(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: "Inspect the example".to_owned(),
                answer: "Example result".to_owned(),
                duration_ms: 513_001,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata {
                model_usage: Some(&usage),
                child_turns: &[child],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.session_model_usage(&session.id).unwrap().len(), 513);
        let saved: (i64, i64, i64) = db.with_connection(|connection| {
            connection.query_row(
                "SELECT length(result),length(messages_json),length(reasoning_json) FROM session_child_turns WHERE session_id=?1",
                [&session.id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).map_err(Into::into)
        }).unwrap();
        assert!(saved.0 > 65_536 && saved.1 > 1_048_576 && saved.2 > 131_072);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tool_summaries_keep_the_latest_calls_of_a_long_turn() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        let tools = (0..4_100)
            .map(|ordinal| ToolCallRow {
                id: format!("tool-example-{ordinal}"),
                turn_number: 1,
                ordinal,
                name: "read".to_owned(),
                summary: "Read example.txt".to_owned(),
                arguments: "{}".to_owned(),
                output: String::new(),
                is_error: false,
                agent_role: "main".to_owned(),
                created_at_ms: 0,
            })
            .collect::<Vec<_>>();
        db.append_turn_with_metadata(
            &session,
            &TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "example-model".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            },
            true,
            TurnActivity {
                tools: &tools,
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata::default(),
        )
        .unwrap();
        let summaries = db.tool_summaries_for_turns(&session.id, &[1]).unwrap();
        assert_eq!(summaries.len(), 4_097);
        assert_eq!(summaries.first().unwrap().ordinal, 3);
        assert_eq!(summaries.last().unwrap().ordinal, 4_099);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn peer_agents_count_as_top_level_usage() {
        for role in [
            "main",
            "discovery_harness",
            "validation_system",
            "discovery",
            "validation",
            "reporter",
            "patch_generation",
        ] {
            assert!(is_top_level_role(role));
        }
        for role in ["scout", "reviewer", "prover", ""] {
            assert!(!is_top_level_role(role));
        }
    }

    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("beale-storage-example-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn new_database_has_only_host_tables() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let tables: Vec<String> = db
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                )?;
                Ok(statement
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap();
        for prefix in [
            "knowledge_",
            "finding",
            "runbook_",
            "fleet_",
            "manual_browsers",
            "memory_",
            "session_goals",
            "session_graph",
            "subscription_usage",
        ] {
            assert!(
                tables.iter().all(|table| !table.starts_with(prefix)),
                "host schema created feature table {prefix}"
            );
        }
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_snapshots_follow_active_work_and_keep_legacy_calls_at_turn_end() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 2,
        };
        for (number, duration_ms, work_elapsed_ms, input_tokens, cache_read_tokens) in [
            (1, 240_000, Some(180_000), 100, Some(20)),
            (2, 240_000, Some(120_000), 200, None),
            (3, 300_000, None, 50, Some(10)),
        ] {
            let turn = TurnRow {
                number,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms,
                internal_prompt: false,
            };
            let usage = [ModelUsageRow {
                agent_id: if number == 2 { "scout-example" } else { "main" }.to_owned(),
                role: if number == 2 { "scout" } else { "main" }.to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(10),
                cache_read_tokens,
                work_elapsed_ms,
                duration_ms: 100,
                observed_at_ms: number,
            }];
            db.append_turn_with_metadata(
                &session,
                &turn,
                number == 1,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
                    ..TurnMetadata::default()
                },
            )
            .unwrap();
        }
        let snapshots = db.session_usage_snapshots(&session.id).unwrap();
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.work_duration_ms)
                .collect::<Vec<_>>(),
            [0, 300_000, 600_000, 780_000]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 100, 300, 350]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|item| item.totals.cache_read_tokens)
                .collect::<Vec<_>>(),
            [0, 20, 20, 30]
        );
        assert_eq!(snapshots.last().unwrap().totals.cache_input_tokens, 150);
        assert_eq!(
            snapshots
                .iter()
                .map(|p| p.main_totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 100, 100, 150]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|p| p.subagent_totals.input_tokens)
                .collect::<Vec<_>>(),
            [0, 0, 200, 200]
        );
        let last = snapshots.last().unwrap();
        assert_eq!(last.main_totals.cache_read_tokens, 30);
        assert_eq!(last.main_totals.cache_input_tokens, 150);
        assert_eq!(last.main_totals.total_calls, 2);
        assert_eq!(last.subagent_totals.measured_calls, 1);
        assert_eq!(last.subagent_totals.cache_measured_calls, 0);
        assert_eq!(last.subagent_totals.cache_input_tokens, 0);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automation_series_persists_and_skips_one_occurrence() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let anchor = chrono::DateTime::parse_from_rfc3339("2026-01-31T12:00:00Z")
            .unwrap()
            .timestamp_millis();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example job".to_owned(),
            created_at_ms: anchor,
            last_message_at_ms: anchor,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn_with_goal_and_automation(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            Some(("month", AutomationOptions::default())),
        )
        .unwrap();
        assert_eq!(db.automation_jobs().unwrap()[0].title, "Example job");
        assert!(!db.automation_job_canceled(&session.id).unwrap());
        assert!(db.cancel_automation(&session.id, false).unwrap());
        assert!(!db.automation_job_canceled(&session.id).unwrap());
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            chrono::DateTime::parse_from_rfc3339("2026-03-31T12:00:00Z")
                .unwrap()
                .timestamp_millis()
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.automation_jobs().unwrap().len(), 1);
        assert!(reopened.cancel_automation(&session.id, true).unwrap());
        assert!(reopened.automation_jobs().unwrap()[0].canceled);
        assert!(reopened.automation_job_canceled(&session.id).unwrap());
        assert!(!reopened.cancel_automation(&session.id, true).unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn due_automation_is_claimed_once_and_recovers_after_lease_expiry() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let anchor = 1_800_000_000_000_i64;
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example scheduled job".to_owned(),
            created_at_ms: anchor,
            last_message_at_ms: anchor,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 2,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1,
            internal_prompt: false,
        };
        db.append_turn_with_goal_and_automation(
            &session,
            &turn,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            Some((
                "minute",
                AutomationOptions {
                    reasoning_effort: 3,
                    goal_mode: true,
                    goal_max_turns: 4,
                    goal_max_duration_minutes: 5,
                },
            )),
        )
        .unwrap();
        let due = anchor + 60_000;
        assert!(db.claim_due_automation(due - 1).unwrap().is_none());
        let claim = db.claim_due_automation(due).unwrap().unwrap();
        assert_eq!(claim.prompt, "Example prompt");
        assert_eq!(claim.model, "model-example");
        assert_eq!(claim.options.reasoning_effort, 3);
        assert!(claim.options.goal_mode);
        assert_eq!(claim.options.goal_max_turns, 4);
        assert_eq!(claim.options.goal_max_duration_minutes, 5);
        assert!(db.claim_due_automation(due).unwrap().is_none());
        assert_eq!(db.automation_runs().unwrap()[0].status, 1);
        assert_eq!(db.interrupt_automation_runs(due + 29_999).unwrap(), 0);
        assert!(
            db.renew_automation_lease(&claim.run_id, due + 20_000)
                .unwrap()
        );
        assert_eq!(db.interrupt_automation_runs(due + 40_000).unwrap(), 0);
        assert!(
            db.finish_automation_run(&claim.run_id, Some(&session.id), 2, "", due + 40_000)
                .unwrap()
        );
        assert_eq!(db.automation_runs().unwrap()[0].status, 2);
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            anchor + 120_000
        );
        let next = db.claim_due_automation(anchor + 120_000).unwrap().unwrap();
        assert_eq!(db.interrupt_automation_runs(anchor + 150_000).unwrap(), 1);
        assert_eq!(db.automation_runs().unwrap()[0].status, 4);
        assert!(
            !db.finish_automation_run(&next.run_id, None, 3, "failed", anchor + 150_000)
                .unwrap()
        );
        let late = db.claim_due_automation(anchor + 720_000).unwrap().unwrap();
        assert_eq!(late.prompt, "Example prompt");
        assert_eq!(
            db.automation_jobs().unwrap()[0].next_run_at_ms,
            anchor + 780_000
        );
        assert!(db.claim_due_automation(anchor + 720_000).unwrap().is_none());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persists_workspace_and_provider_state() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let workspace = WorkspaceRow {
            id: "workspace-example".to_owned(),
            name: "Example Workspace".to_owned(),
            directory: "example-directory".to_owned(),
        };
        db.replace_workspaces(std::slice::from_ref(&workspace))
            .unwrap();
        assert_eq!(db.workspaces().unwrap(), [workspace]);
        db.save_setting("lead-provider", Some("xai")).unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(reopened.workspaces().unwrap().len(), 1);
        assert_eq!(
            reopened.setting("lead-provider").unwrap().as_deref(),
            Some("xai")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn appends_session_turns_atomically() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.migrate_schema("example", EXAMPLE_MIGRATIONS).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example prompt".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        let first = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 1_234,
            internal_prompt: false,
        };
        db.append_turn_with_metadata(
            &session,
            &first,
            true,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata {
                extensions: &[&ExampleMarker("first-turn")],
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        assert_eq!(example_markers(&db), ["first-turn"]);
        assert!(db.append_turn(&session, &first, false).is_err());
        let second = TurnRow {
            number: 2,
            ..first.clone()
        };
        let updated_session = SessionRow {
            last_message_at_ms: 42,
            ..session.clone()
        };
        let reasoning = ReasoningSummaryRow {
            turn_number: 2,
            ordinal: 0,
            after_tool_count: 0,
            text: "Checked the example input.".to_owned(),
            agent_role: "main".to_owned(),
        };
        let context = "[{\"type\":\"compaction\",\"encrypted_content\":\"opaque-example\"}]";
        db.append_turn_with_metadata(
            &updated_session,
            &second,
            false,
            TurnActivity {
                tools: &[],
                reasoning: std::slice::from_ref(&reasoning),
                commentary: &[],
            },
            None,
            TurnMetadata {
                provider_context_json: Some(context),
                model_usage: None,
                ..TurnMetadata::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.provider_context(&session.id).unwrap().as_deref(),
            Some(context)
        );
        assert!(
            db.append_turn_with_metadata(
                &updated_session,
                &second,
                false,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    provider_context_json: Some("[]"),
                    model_usage: None,
                    ..TurnMetadata::default()
                },
            )
            .is_err()
        );
        assert_eq!(
            db.provider_context(&session.id).unwrap().as_deref(),
            Some(context)
        );
        assert_eq!(db.reasoning_summaries(&session.id).unwrap(), [reasoning]);
        assert_eq!(
            db.session(&session.id).unwrap().unwrap().turns,
            [first, second]
        );
        assert_eq!(db.sessions().unwrap(), [updated_session]);
        assert_eq!(db.session_total_duration_ms(&session.id).unwrap(), 2_468);
        assert!(!db.session_has_campaign_activity(&session.id).unwrap());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_totals_and_campaign_activity_survive_transcript_trimming() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: 1,
            last_message_at_ms: 1,
        };
        for number in 1..=65 {
            let turn = TurnRow {
                number,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example prompt".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1_000,
                internal_prompt: false,
            };
            let tools = (number == 1).then(|| ToolCallRow {
                id: "tool-example".to_owned(),
                turn_number: number,
                ordinal: 0,
                name: "memory_save".to_owned(),
                summary: "Saved example memory".to_owned(),
                arguments: "{}".to_owned(),
                output: "{}".to_owned(),
                is_error: false,
                agent_role: "main".to_owned(),
                created_at_ms: 0,
            });
            let usage = [ModelUsageRow {
                agent_id: "main".into(),
                role: "main".into(),
                provider: 1,
                method: 1,
                model: "model-example".into(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(40),
                work_elapsed_ms: Some(500),
                duration_ms: 500,
                observed_at_ms: number,
            }];
            db.append_turn_with_metadata(
                &session,
                &turn,
                number == 1,
                TurnActivity {
                    tools: tools.as_slice(),
                    reasoning: &[],
                    commentary: &[],
                },
                None,
                TurnMetadata {
                    model_usage: Some(&usage),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert_eq!(db.session(&session.id).unwrap().unwrap().turns.len(), 64);
        assert_eq!(db.session_total_duration_ms(&session.id).unwrap(), 65_000);
        let models = db.session_model_totals(&session.id).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].totals.total_calls, 65);
        assert_eq!(models[0].totals.input_tokens, 6_500);
        assert_eq!(models[0].totals.cache_read_tokens, 2_600);
        assert!(db.session_has_campaign_activity(&session.id).unwrap());
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_listing_excludes_internal_quick_chats() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        for (id, workspace_id) in [
            ("research-session-example", "workspace-example"),
            ("quick-chat-example", "internal:quick-chats"),
        ] {
            let session = SessionRow {
                id: id.to_owned(),
                workspace_id: workspace_id.to_owned(),
                title: "Example".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            };
            let turn = TurnRow {
                number: 1,
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                prompt: "Example question".to_owned(),
                answer: "Example answer".to_owned(),
                duration_ms: 1,
                internal_prompt: false,
            };
            if workspace_id == "internal:quick-chats" {
                db.append_new_quick_chat_turn(&session, &turn, "workspace-example")
                    .unwrap();
            } else {
                db.append_turn(&session, &turn, true).unwrap();
            }
        }
        let visible = db.sessions_excluding("internal:quick-chats").unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "research-session-example");
        assert_eq!(
            db.archived_quick_chat_ids().unwrap(),
            ["quick-chat-example"]
        );
        assert!(db.archive_session("research-session-example", 2).unwrap());
        assert!(
            db.sessions_excluding("internal:quick-chats")
                .unwrap()
                .is_empty()
        );
        assert!(db.session("research-session-example").unwrap().is_some());
        let archive = db.archive_items().unwrap();
        assert_eq!(archive.len(), 2);
        assert_eq!(archive[0].id, "research-session-example");
        assert_eq!(archive[0].model, "model-example");
        assert_eq!(archive[0].turn_count, 1);
        assert!(!archive[0].quick_chat);
        assert!(archive[1].quick_chat);
        assert_eq!(archive[1].workspace_id, "workspace-example");
        assert!(db.restore_session("research-session-example").unwrap());
        assert!(!db.restore_session("research-session-example").unwrap());
        assert!(!db.restore_session("quick-chat-example").unwrap());
        assert_eq!(
            db.sessions_excluding("internal:quick-chats").unwrap().len(),
            1
        );
        assert_eq!(db.archive_items().unwrap().len(), 1);
        assert_eq!(
            db.quick_chat_workspace("quick-chat-example")
                .unwrap()
                .as_deref(),
            Some("workspace-example")
        );
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert_eq!(
            reopened
                .quick_chat_workspace("quick-chat-example")
                .unwrap()
                .as_deref(),
            Some("workspace-example")
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn features_default_off_and_persist_enablement() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        assert!(!db.feature_enabled("example-feature").unwrap());
        db.set_feature_enabled("example-feature", true).unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        assert!(reopened.feature_enabled("example-feature").unwrap());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_workspace_scope_does_not_broaden_after_deletion() {
        let root = root();
        let db = HostDatabase::open(&root).unwrap();
        db.replace_workspaces(&[
            WorkspaceRow {
                id: "workspace-a".to_owned(),
                name: "Example A".to_owned(),
                directory: "example-a".to_owned(),
            },
            WorkspaceRow {
                id: "workspace-b".to_owned(),
                name: "Example B".to_owned(),
                directory: "example-b".to_owned(),
            },
        ])
        .unwrap();
        assert!(
            db.extension_scope("feature", "example-feature")
                .unwrap()
                .all_workspaces
        );
        db.set_extension_scope(
            "feature",
            "example-feature",
            false,
            &["workspace-a".to_owned()],
        )
        .unwrap();
        assert!(
            db.extension_applies_to("feature", "example-feature", "workspace-a")
                .unwrap()
        );
        assert!(
            !db.extension_applies_to("feature", "example-feature", "workspace-b")
                .unwrap()
        );
        for workspace_id in ["workspace-a", "workspace-b"] {
            let session = SessionRow {
                id: format!("session-{workspace_id}"),
                workspace_id: workspace_id.to_owned(),
                title: "Example objective".to_owned(),
                created_at_ms: 1,
                last_message_at_ms: 1,
            };
            db.append_turn_with_goal_activity(
                &session,
                &TurnRow {
                    number: 1,
                    provider: 1,
                    method: 1,
                    model: "model-example".to_owned(),
                    prompt: "Example objective".to_owned(),
                    answer: "Example observation".to_owned(),
                    duration_ms: 1,
                    internal_prompt: false,
                },
                true,
                TurnActivity {
                    tools: &[],
                    reasoning: &[],
                    commentary: &[],
                },
            )
            .unwrap();
        }
        assert!(
            db.set_extension_scope("feature", "example-feature", false, &["missing".to_owned()])
                .is_err()
        );
        db.replace_workspaces(&[WorkspaceRow {
            id: "workspace-b".to_owned(),
            name: "Example B".to_owned(),
            directory: "example-b".to_owned(),
        }])
        .unwrap();
        drop(db);
        let reopened = HostDatabase::open(&root).unwrap();
        let scope = reopened
            .extension_scope("feature", "example-feature")
            .unwrap();
        assert!(!scope.all_workspaces && scope.workspace_ids.is_empty());
        assert!(
            !reopened
                .extension_applies_to("feature", "example-feature", "workspace-b")
                .unwrap()
        );
        reopened
            .set_extension_scope("feature", "example-feature", true, &[])
            .unwrap();
        assert!(
            reopened
                .extension_applies_to("feature", "example-feature", "workspace-b")
                .unwrap()
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
