//! Assigned-environment execution with leader-owned inference and research state.
//! Worker transport and supervision compose the durable spool and local lease.
mod bootstrap;
mod canonical_allocation;
pub mod docker;
mod gateway;
pub(crate) mod local_allocation;
pub(crate) mod placement;
pub use bootstrap::prepare_worker;
#[cfg(feature = "findings")]
mod findings;
#[cfg(test)]
mod gateway_tests;
mod inference;
mod lease;
mod local_host;
pub mod peer_coordinator;
pub mod peer_host;
pub(crate) mod peer_recovery;
pub mod peer_workspace;
pub(super) use local_host::{LocalWorkerHost, WorkerRecovery};
mod plugin_resources;
mod plugins;
mod process;
mod recovery;
mod restart;
pub(crate) use restart::{
    discard_unclaimed_lock, lock as acquire_local_execution_lock, recover_coordinators,
    recover_page as recover_local_workers,
};
#[cfg(feature = "runbooks")]
mod runbooks;
mod supervisor;
mod tool_errors;
mod transport;
mod worker;
#[cfg(target_os = "linux")]
pub use beale_tools::linux_worker::run as linux_worker_supervisor;
pub(super) use gateway::router;
pub use lease::{Lease, LeaseState, Renewal};
pub use process::{ManagedWorker, ProcessFuture, WorkerProcess};
pub use supervisor::{StopReason, SupervisionReport, supervise, supervise_assignment};
pub use transport::{TransportError, TransportFailure, WorkerTransport};
pub use worker::{WorkerLaunch, run_assigned, worker_entry};

#[cfg(all(test, feature = "findings"))]
mod tests;
