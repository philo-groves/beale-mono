//! Tracking persistence in the shared host database.
//!
//! This feature owns these tables and applies [`MIGRATIONS`] under its feature
//! ID; builds without the feature never create them.

use anyhow::{Result, ensure};
use beale_storage::{
    HostDatabase,
    rusqlite::{self, params},
};

/// Ordered schema versions for this feature's tables.
pub const MIGRATIONS: &[&str] = &[V1];

const V1: &str = "
CREATE TABLE subscription_usage_history (
    provider INTEGER NOT NULL,
    window_id TEXT NOT NULL,
    label TEXT NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    used_percent INTEGER NOT NULL,
    resets_at_unix INTEGER NOT NULL,
    PRIMARY KEY(provider, window_id, observed_at_ms)
);
CREATE INDEX subscription_usage_observed ON subscription_usage_history(observed_at_ms);
";

/// Creates or upgrades this feature's tables.
pub fn migrate(db: &HostDatabase) -> Result<()> {
    db.migrate_schema(crate::FEATURE_ID, MIGRATIONS)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionUsageRow {
    pub provider: i32,
    pub window_id: String,
    pub label: String,
    pub observed_at_ms: i64,
    pub used_percent: i64,
    pub resets_at_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUsageDayRow {
    pub day_start_ms: i64,
    pub role: String,
    pub provider: i32,
    pub method: i32,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub duration_ms: i64,
    pub measured_calls: i64,
    pub total_calls: i64,
    pub cache_read_tokens: i64,
    pub cache_input_tokens: i64,
    pub cache_measured_calls: i64,
}

/// Tracking history on the host database.
pub trait TrackingStore {
    fn record_subscription_usage(&self, points: &[SubscriptionUsageRow]) -> Result<()>;
    fn subscription_history(&self, since_ms: i64) -> Result<Vec<SubscriptionUsageRow>>;
    fn model_usage_days(&self, since_ms: i64) -> Result<Vec<ModelUsageDayRow>>;
}

impl TrackingStore for HostDatabase {
    fn record_subscription_usage(&self, points: &[SubscriptionUsageRow]) -> Result<()> {
        ensure!(points.len() <= 8, "too many usage windows");
        if points.is_empty() {
            return Ok(());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for point in points {
                ensure!(
                    point.window_id.len() <= 40
                        && point.label.len() <= 40
                        && (0..=100).contains(&point.used_percent)
                        && point.observed_at_ms >= 0,
                    "invalid subscription history point"
                );
                transaction.execute(
                    "INSERT OR IGNORE INTO subscription_usage_history
                     (provider, window_id, label, observed_at_ms, used_percent, resets_at_unix)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        point.provider,
                        point.window_id,
                        point.label,
                        point.observed_at_ms,
                        point.used_percent,
                        point.resets_at_unix
                    ],
                )?;
            }
            transaction.execute(
                "DELETE FROM subscription_usage_history WHERE observed_at_ms < ?1",
                [points[0].observed_at_ms.saturating_sub(8 * 86_400_000)],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn subscription_history(&self, since_ms: i64) -> Result<Vec<SubscriptionUsageRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT provider, window_id, label, observed_at_ms, used_percent, resets_at_unix
                 FROM subscription_usage_history WHERE observed_at_ms >= ?1
                 ORDER BY observed_at_ms, provider, window_id LIMIT 20001",
            )?;
            let rows = statement
                .query_map([since_ms], |row| {
                    Ok(SubscriptionUsageRow {
                        provider: row.get(0)?,
                        window_id: row.get(1)?,
                        label: row.get(2)?,
                        observed_at_ms: row.get(3)?,
                        used_percent: row.get(4)?,
                        resets_at_unix: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 20000, "too many subscription history points");
            Ok(rows)
        })
    }

    fn model_usage_days(&self, since_ms: i64) -> Result<Vec<ModelUsageDayRow>> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT (observed_at_ms / 86400000) * 86400000 AS day, role, provider, method, model,
                        SUM(COALESCE(input_tokens, 0)), SUM(COALESCE(output_tokens, 0)),
                        SUM(duration_ms), SUM(CASE WHEN input_tokens IS NOT NULL AND output_tokens IS NOT NULL THEN 1 ELSE 0 END), COUNT(*),
                        SUM(COALESCE(cache_read_tokens, 0)),
                        SUM(CASE WHEN cache_read_tokens IS NOT NULL THEN COALESCE(input_tokens, 0) ELSE 0 END),
                        COUNT(cache_read_tokens)
                 FROM session_model_usage WHERE observed_at_ms >= ?1
                 GROUP BY day, role, provider, method, model ORDER BY day, provider, model, role LIMIT 2001")?;
            let rows = statement.query_map([since_ms], |row| Ok(ModelUsageDayRow {
                day_start_ms: row.get(0)?, role: row.get(1)?, provider: row.get(2)?,
                method: row.get(3)?, model: row.get(4)?, input_tokens: row.get(5)?,
                output_tokens: row.get(6)?, duration_ms: row.get(7)?,
                measured_calls: row.get(8)?, total_calls: row.get(9)?,
                cache_read_tokens: row.get(10)?, cache_input_tokens: row.get(11)?,
                cache_measured_calls: row.get(12)?,
            }))?.collect::<rusqlite::Result<Vec<_>>>()?;
            ensure!(rows.len() <= 2000, "too many model usage day rows");
            Ok(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beale_storage::{
        ModelUsageRow, SessionRow, SessionUsageTotals, TurnActivity, TurnMetadata, TurnRow,
    };

    fn root() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "beale-feature-store-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ))
    }

    fn open(root: &std::path::Path) -> HostDatabase {
        let db = HostDatabase::open(root).unwrap();
        migrate(&db).unwrap();
        db
    }

    #[test]
    fn tracking_history_persists_and_aggregates_agent_calls() {
        let root = root();
        let db = open(&root);
        let day = 1_800_000_000_000_i64 / 86_400_000 * 86_400_000;
        db.record_subscription_usage(&[SubscriptionUsageRow {
            provider: 1,
            window_id: "weekly".to_owned(),
            label: "Week".to_owned(),
            observed_at_ms: day + 1_000,
            used_percent: 35,
            resets_at_unix: 1_800_100_000,
        }])
        .unwrap();
        let session = SessionRow {
            id: "session-example".to_owned(),
            workspace_id: "workspace-example".to_owned(),
            title: "Example session".to_owned(),
            created_at_ms: day,
            last_message_at_ms: day + 3_000,
        };
        let turn = TurnRow {
            number: 1,
            provider: 1,
            method: 1,
            model: "model-example".to_owned(),
            prompt: "Example prompt".to_owned(),
            answer: "Example answer".to_owned(),
            duration_ms: 2_000,
            internal_prompt: false,
        };
        let usage = [
            ModelUsageRow {
                agent_id: "main".to_owned(),
                role: "main".to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_tokens: Some(37),
                work_elapsed_ms: Some(800),
                duration_ms: 800,
                observed_at_ms: day + 1_000,
            },
            ModelUsageRow {
                agent_id: "scout-example".to_owned(),
                role: "scout".to_owned(),
                provider: 1,
                method: 1,
                model: "model-example".to_owned(),
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                work_elapsed_ms: Some(1_500),
                duration_ms: 500,
                observed_at_ms: day + 2_000,
            },
        ];
        db.append_turn_with_metadata(
            &session,
            &turn,
            true,
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
        drop(db);

        let db = open(&root);
        assert_eq!(db.subscription_history(day).unwrap()[0].used_percent, 35);
        let calls = db.session_model_usage(&session.id).unwrap();
        assert_eq!(
            db.session_last_top_level_role(&session.id)
                .unwrap()
                .as_deref(),
            Some("main")
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].agent_id, "main");
        assert_eq!(calls[0].cache_read_tokens, Some(37));
        assert_eq!(calls[1].agent_id, "scout-example");
        assert_eq!(
            db.session_usage_totals(&session.id).unwrap(),
            SessionUsageTotals {
                input_tokens: 100,
                output_tokens: 20,
                measured_calls: 1,
                total_calls: 2,
                cache_read_tokens: 37,
                cache_input_tokens: 100,
                cache_measured_calls: 1,
            }
        );
        let rows = db.model_usage_days(day).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "main");
        assert_eq!((rows[0].input_tokens, rows[0].output_tokens), (100, 20));
        assert_eq!((rows[0].measured_calls, rows[0].total_calls), (1, 1));
        assert_eq!(
            (
                rows[0].cache_read_tokens,
                rows[0].cache_input_tokens,
                rows[0].cache_measured_calls
            ),
            (37, 100, 1)
        );
        assert_eq!(rows[1].role, "scout");
        assert_eq!((rows[1].duration_ms, rows[1].measured_calls), (500, 0));
        assert_eq!(
            (
                rows[1].cache_read_tokens,
                rows[1].cache_input_tokens,
                rows[1].cache_measured_calls
            ),
            (0, 0, 0)
        );
        assert!(db.model_usage_days(day + 86_400_000).unwrap().is_empty());
        let other = SessionRow {
            id: "other-session-example".into(),
            ..session.clone()
        };
        db.append_turn_with_metadata(
            &other,
            &turn,
            true,
            TurnActivity {
                tools: &[],
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
        let sources = db.session_model_totals(&session.id).unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].role, "main");
        assert_eq!(sources[0].totals.input_tokens, 100);
        assert_eq!(sources[0].totals.cache_read_tokens, 37);
        assert_eq!(sources[0].totals.cache_input_tokens, 100);
        assert_eq!(sources[0].totals.total_calls, 1);
        assert_eq!(sources[1].role, "scout");
        assert_eq!(sources[1].totals.measured_calls, 0);
        assert_eq!(sources[1].totals.total_calls, 1);
        assert_eq!(sources[1].totals.cache_measured_calls, 0);
        let points = db.session_usage_snapshots(&session.id).unwrap();
        let last = points.last().unwrap();
        assert_eq!(last.main_totals.input_tokens, 100);
        assert_eq!(last.main_totals.total_calls, 1);
        assert_eq!(last.subagent_totals.input_tokens, 0);
        assert_eq!(last.subagent_totals.measured_calls, 0);
        assert_eq!(last.subagent_totals.total_calls, 1);
        assert!(
            db.session_model_totals("missing-session-example")
                .unwrap()
                .is_empty()
        );
        let peer_turn = TurnRow {
            number: 2,
            ..turn.clone()
        };
        let peer_usage = [
            ModelUsageRow {
                agent_id: "reporter".to_owned(),
                role: "reporter".to_owned(),
                ..usage[0].clone()
            },
            ModelUsageRow {
                agent_id: "scout-peer-example".to_owned(),
                ..usage[1].clone()
            },
        ];
        db.append_turn_with_metadata(
            &session,
            &peer_turn,
            false,
            TurnActivity {
                tools: &[],
                reasoning: &[],
                commentary: &[],
            },
            None,
            TurnMetadata {
                model_usage: Some(&peer_usage),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            db.session_last_top_level_role(&session.id)
                .unwrap()
                .as_deref(),
            Some("reporter")
        );
        let peer_last = db
            .session_usage_snapshots(&session.id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(peer_last.main_totals.total_calls, 2);
        assert_eq!(peer_last.subagent_totals.total_calls, 2);
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}
