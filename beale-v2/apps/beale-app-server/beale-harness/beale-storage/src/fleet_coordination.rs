//! Trusted host Fleet coordination state, independent of optional Swarms initialization.
use crate::HostDatabase;
use crate::swarm_state::{next, unique};
use anyhow::{Context, Result, bail, ensure};
use beale_contracts::swarms::*;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct FleetCoordinationStore {
    db: HostDatabase,
}

impl FleetCoordinationStore {
    /// The owning app-server registers its own loopback control endpoint. This
    /// is not a remote pairing and never enables local research fallback.
    pub fn save_local_endpoint(&self, endpoint: &str, token: &str) -> Result<()> {
        crate::fleet_executions::FleetExecutionStore::open(self.db.clone())?;
        beale_contracts::swarm_host::validate_origin(endpoint).map_err(anyhow::Error::msg)?;
        let url = url::Url::parse(endpoint)?;
        ensure!(
            url.scheme() == "http"
                && url.host_str().is_some_and(|s| s == "localhost"
                    || s.parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())),
            "local control endpoint must be loopback"
        );
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid host credential"
        );
        let id = crate::fleet_executions::FleetExecutionStore::open(self.db.clone())?.host_id()?;
        self.transaction(|tx| {tx.execute("INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,1,?4) ON CONFLICT(id) DO UPDATE SET endpoint=excluded.endpoint,control_token=excluded.control_token,enabled=1,protocol_major=excluded.protocol_major",params![id,endpoint,token,beale_contracts::PROTOCOL_MAJOR])?;Ok(())})
    }
    pub fn open(db: HostDatabase) -> Result<Self> {
        db.migrate_schema("fleet-coordination", &[SCHEMA])?;
        Ok(Self { db })
    }
    fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.db.with_connection(|db| {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = f(&tx)?;
            tx.commit()?;
            Ok(result)
        })
    }
}
fn new_id() -> String {
    Uuid::new_v4().to_string()
}

const SCHEMA: &str = "
CREATE TABLE fleet_coordination_peers(id TEXT PRIMARY KEY, endpoint TEXT NOT NULL, control_token TEXT NOT NULL,
 enabled INTEGER NOT NULL, protocol_major INTEGER NOT NULL);
CREATE TABLE fleet_coordination_accounts(provider TEXT NOT NULL, account_key TEXT NOT NULL, group_id TEXT NOT NULL UNIQUE,
 document TEXT NOT NULL, PRIMARY KEY(provider, account_key));
CREATE TABLE fleet_coordination_usage(id TEXT PRIMARY KEY, document TEXT NOT NULL);
CREATE TABLE fleet_coordination_pacing(id TEXT PRIMARY KEY, document TEXT NOT NULL);
CREATE TABLE fleet_coordination_containers(id TEXT PRIMARY KEY, document TEXT NOT NULL);
";

/// Trusted-host connection material; intentionally has no Serialize or Debug.
/// The existing pairing protocol provides the token, never a research worker.
pub struct FleetPeer {
    pub id: String,
    pub endpoint: String,
    pub control_token: String,
    pub enabled: bool,
    pub protocol_major: u32,
}

impl FleetCoordinationStore {
    pub fn containers(&self) -> Result<Vec<ContainerConfiguration>> {
        self.db.with_connection(|db| {
            let mut query =
                db.prepare("SELECT document FROM fleet_coordination_containers ORDER BY id")?;
            let documents = query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            documents
                .into_iter()
                .map(|s| Ok(serde_json::from_str(&s)?))
                .collect()
        })
    }

    pub fn usage(&self, group: &str) -> Result<Vec<UsageEvent>> {
        self.db.with_connection(|db| {
            let mut query =
                db.prepare("SELECT document FROM fleet_coordination_usage ORDER BY id")?;
            let documents = query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let values = documents
                .into_iter()
                .map(|s| serde_json::from_str::<UsageEvent>(&s))
                .collect::<serde_json::Result<Vec<_>>>()?;
            Ok(values.into_iter().filter(|u| u.group_id == group).collect())
        })
    }

    pub fn pacing_reservations(&self, group: &str) -> Result<Vec<PacingReservation>> {
        self.db.with_connection(|db| {
            let mut query =
                db.prepare("SELECT document FROM fleet_coordination_pacing ORDER BY id")?;
            let documents = query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let values = documents
                .into_iter()
                .map(|s| serde_json::from_str::<PacingReservation>(&s))
                .collect::<serde_json::Result<Vec<_>>>()?;
            Ok(values.into_iter().filter(|u| u.group_id == group).collect())
        })
    }

    pub fn save_peer(&self, peer: &FleetPeer) -> Result<()> {
        let endpoint = url::Url::parse(&peer.endpoint).context("invalid peer origin")?;
        ensure!(
            valid_id(&peer.id)
                && endpoint.scheme() == "https"
                && endpoint.host_str().is_some_and(|h| h.ends_with(".ts.net")
                    && h.split('.').all(|part| !part.is_empty()
                        && !part.starts_with('-')
                        && !part.ends_with('-')
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')))
                && endpoint
                    .port_or_known_default()
                    .is_some_and(|port| port > 0)
                && endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.path() == "/"
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "peer must use a paired Tailscale HTTPS origin"
        );
        ensure!(
            peer.control_token.len() == 64
                && peer.control_token.bytes().all(|c| c.is_ascii_hexdigit())
                && peer.protocol_major == beale_contracts::PROTOCOL_MAJOR,
            "invalid peer pairing"
        );
        self.transaction(|tx| {
            tx.execute("INSERT INTO fleet_coordination_peers VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET endpoint=excluded.endpoint,control_token=excluded.control_token,enabled=excluded.enabled,protocol_major=excluded.protocol_major", params![peer.id, endpoint.as_str(), peer.control_token, peer.enabled, peer.protocol_major])?;
            Ok(())
        })
    }

    /// Resolve one saved pairing without decoding unrelated peers. Disabled
    /// pairings remain available for termination and retained-work recovery.
    pub fn peer(&self, id: &str) -> Result<Option<FleetPeer>> {
        self.db.with_connection(|db| {
            Ok(db.query_row("SELECT id,endpoint,control_token,enabled,protocol_major FROM fleet_coordination_peers WHERE id=?1", [id], |r| Ok(FleetPeer {
                id:r.get(0)?, endpoint:r.get(1)?, control_token:r.get(2)?, enabled:r.get(3)?, protocol_major:r.get(4)?,
            })).optional()?)
        })
    }

    pub fn peers(&self) -> Result<Vec<FleetPeer>> {
        self.db.with_connection(|db| {
            let mut query = db.prepare("SELECT id,endpoint,control_token,enabled,protocol_major FROM fleet_coordination_peers ORDER BY id")?;
            Ok(query.query_map([], |r| Ok(FleetPeer { id: r.get(0)?, endpoint: r.get(1)?, control_token: r.get(2)?, enabled: r.get(3)?, protocol_major: r.get(4)? }))?.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Stable provider account key or explicit local grouping, supplied by host identity policy.
    /// No credential is accepted, stored, or hashed into the budget identity.
    pub fn account_group(
        &self,
        provider: &str,
        account_key: &str,
        owner: &str,
    ) -> Result<BudgetGroup> {
        ensure!(
            valid_id(provider)
                && !account_key.is_empty()
                && account_key.len() <= 512
                && valid_id(owner),
            "invalid account identity"
        );
        self.transaction(|tx| {
            let saved: Option<String> = tx.query_row("SELECT document FROM fleet_coordination_accounts WHERE provider=?1 AND account_key=?2", params![provider, account_key], |r| r.get(0)).optional()?;
            if let Some(saved) = saved { return Ok(serde_json::from_str(&saved)?); }
            let group = BudgetGroup { id: new_id(), provider_id: provider.to_owned(), owner_host_id: owner.to_owned(), owner_generation: 1, participants: vec![] };
            tx.execute("INSERT INTO fleet_coordination_accounts VALUES(?1,?2,?3,?4)", params![provider, account_key, group.id, serde_json::to_string(&group)?])?;
            Ok(group)
        })
    }

    pub fn update_group(&self, group: &BudgetGroup, expected_generation: u64) -> Result<()> {
        ensure!(
            valid_id(&group.owner_host_id)
                && unique(group.participants.iter().map(String::as_str))
                && group.owner_generation == next(expected_generation)?,
            "invalid group update"
        );
        self.transaction(|tx| {
            let saved: String = tx.query_row(
                "SELECT document FROM fleet_coordination_accounts WHERE group_id=?1",
                [&group.id],
                |r| r.get(0),
            )?;
            let old: BudgetGroup = serde_json::from_str(&saved)?;
            ensure!(
                old.owner_generation == expected_generation && old.provider_id == group.provider_id,
                "stale accounting ownership"
            );
            tx.execute(
                "UPDATE fleet_coordination_accounts SET document=?2 WHERE group_id=?1",
                params![group.id, serde_json::to_string(group)?],
            )?;
            Ok(())
        })
    }

    pub fn record_usage(&self, usage: &UsageEvent) -> Result<()> {
        ensure!(
            valid_id(&usage.id) && valid_id(&usage.attempt_id),
            "invalid usage event"
        );
        self.transaction(|tx| {
            registered_group(tx, &usage.group_id, &usage.participant_id)?;
            insert_document(
                tx,
                "fleet_coordination_usage",
                &usage.id,
                &serde_json::to_value(usage)?,
            )
        })
    }

    pub fn record_pacing_reservation(&self, reservation: &PacingReservation) -> Result<()> {
        ensure!(valid_id(&reservation.id), "invalid pacing reservation");
        self.transaction(|tx| {
            let group = registered_group(tx, &reservation.group_id, &reservation.participant_id)?;
            let document = serde_json::to_value(reservation)?;
            // Registered participants may reconcile an acknowledged reservation from
            // an earlier owner generation. This never admits new work or changes it.
            if matching_document(tx, "fleet_coordination_pacing", &reservation.id, &document)? {
                return Ok(());
            }
            ensure!(
                group.owner_generation == reservation.owner_generation,
                "stale accounting owner"
            );
            insert_document(
                tx,
                "fleet_coordination_pacing",
                &reservation.id,
                &serde_json::to_value(reservation)?,
            )
        })
    }

    pub fn register_container(&self, config: &ContainerConfiguration) -> Result<()> {
        ensure!(
            valid_id(&config.id)
                && valid_id(&config.host_id)
                && config.revision > 0
                && !config.image.trim().is_empty()
                && config.image.len() <= 1024,
            "invalid container configuration"
        );
        self.transaction(|tx| {
            insert_document(
                tx,
                "fleet_coordination_containers",
                &config.id,
                &serde_json::to_value(config)?,
            )
        })
    }
}

fn registered_group(tx: &Transaction<'_>, id: &str, participant: &str) -> Result<BudgetGroup> {
    let document: String = tx.query_row(
        "SELECT document FROM fleet_coordination_accounts WHERE group_id=?1",
        [id],
        |r| r.get(0),
    )?;
    let group: BudgetGroup = serde_json::from_str(&document)?;
    ensure!(
        group.participants.iter().any(|p| p == participant),
        "unregistered accounting participant"
    );
    Ok(group)
}

// Table names are private constants chosen above, never interpolated external input.
fn insert_document(tx: &Transaction<'_>, table: &str, id: &str, document: &Value) -> Result<()> {
    if !matching_document(tx, table, id, document)? {
        tx.execute(
            &format!("INSERT INTO {table} VALUES(?1,?2)"),
            params![id, serde_json::to_string(document)?],
        )?;
    }
    Ok(())
}

fn matching_document(
    tx: &Transaction<'_>,
    table: &str,
    id: &str,
    document: &Value,
) -> Result<bool> {
    let saved: Option<String> = tx
        .query_row(
            &format!("SELECT document FROM {table} WHERE id=?1"),
            [id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(saved) = saved {
        if serde_json::from_str::<Value>(&saved)? != *document {
            bail!("record identity reused with different content");
        }
        Ok(true)
    } else {
        Ok(false)
    }
}
