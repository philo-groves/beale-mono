//! Docker host operations. Research receives neither Docker nor host credentials.
mod http;
mod runtime;
mod service;
mod store;
#[cfg(test)]
mod tests;
pub use http::router;
pub use runtime::RuntimeBundle;
pub use service::DockerEnvironments;
pub(crate) use service::InstanceBusy;
pub use store::{DockerStore, Instance, PreflightObservation};
mod cli;
pub use cli::{
    ContainerHandle, ContainerOptions, ContainerStatus, Docker, ResolvedImage, StartupMode,
};
