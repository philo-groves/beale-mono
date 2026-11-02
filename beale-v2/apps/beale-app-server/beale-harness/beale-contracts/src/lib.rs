//! Versioned client-to-host wire contracts. No host-only types belong here.

pub mod v1 {
    /// `proto/beale/v1/host.proto` is the source wire schema. Field tags and
    /// types must change together here and in that file.
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct HealthResponse {
        #[prost(uint32, tag = "1")]
        pub protocol_major: u32,
        #[prost(uint32, tag = "2")]
        pub protocol_minor: u32,
        #[prost(string, tag = "3")]
        pub service: String,
    }
}

pub const PROTOCOL_MAJOR: u32 = 1;
pub const PROTOCOL_MINOR: u32 = 0;
