use beale_contracts::swarms::Role;
use serde::Deserialize;
use std::{collections::HashMap, sync::LazyLock};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProfileId(&'static str);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    id: String,
    pub revision: u32,
    label: String,
    role: Role,
    pub instructions: String,
    #[serde(default)]
    spawn_when: String,
    pub max_models: usize,
    pub default_max_concurrent: Option<u32>,
    pub runtime_max_concurrent: u32,
    #[serde(default)]
    required_context_policy: Option<crate::ContextPolicy>,
}

static PROFILES: LazyLock<HashMap<&'static str, Profile>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Catalog {
        schema_version: u32,
        profiles: Vec<Profile>,
    }
    let catalog: Catalog =
        serde_json::from_str(include_str!("../profiles.json")).expect("bundled profiles");
    assert_eq!(catalog.schema_version, 1);
    let mut profiles = HashMap::new();
    for profile in catalog.profiles {
        assert!(!profile.id.is_empty() && profile.id.len() <= 64);
        assert!(
            profile
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        );
        assert!(profile.revision > 0 && !profile.label.is_empty());
        assert!((1..=16).contains(&profile.max_models));
        assert!((1..=64).contains(&profile.runtime_max_concurrent));
        assert!(
            profile
                .default_max_concurrent
                .is_none_or(|limit| (1..=profile.runtime_max_concurrent).contains(&limit))
        );
        assert!(
            profile.role != Role::Leader
                || (profile.max_models == 1 && profile.runtime_max_concurrent == 1)
        );
        let id: &'static str = Box::leak(profile.id.clone().into_boxed_str());
        assert!(
            profiles.insert(id, profile).is_none(),
            "duplicate profile ID"
        );
    }
    profiles
});

pub(crate) fn profile(id: ProfileId) -> &'static Profile {
    &PROFILES[id.0]
}

pub fn task_profiles() -> Vec<ProfileId> {
    let mut ids: Vec<_> = PROFILES
        .iter()
        .filter(|(_, p)| p.role == Role::Worker)
        .map(|(id, _)| ProfileId(id))
        .collect();
    ids.sort();
    ids
}

// Named bundled profile identities. Role is independent of specialist identity.
#[allow(non_upper_case_globals)]
impl ProfileId {
    pub const Leader: Self = Self("leader");
    pub const ResearchDeputy: Self = Self("research_deputy");
    pub const DiscoveryHarness: Self = Self("discovery_harness");
    pub const ValidationSystem: Self = Self("validation_system");
    pub const Discovery: Self = Self("discovery");
    pub const Validation: Self = Self("validation");
    pub const Reporter: Self = Self("reporter");
    pub const PatchGeneration: Self = Self("patch_generation");
    pub const AuditTargets: Self = Self("audit_targets");
    pub fn from_id(id: &str) -> Option<Self> {
        PROFILES.get_key_value(id).map(|(id, _)| Self(id))
    }
    pub const fn id(self) -> &'static str {
        self.0
    }
    pub fn role(self) -> Role {
        profile(self).role
    }
    pub fn revision(self) -> u32 {
        profile(self).revision
    }
    pub fn label(self) -> &'static str {
        &profile(self).label
    }
    pub fn instruction(self) -> &'static str {
        &profile(self).instructions
    }
    pub fn spawn_when(self) -> &'static str {
        &profile(self).spawn_when
    }
    pub fn is_top_level(self) -> bool {
        profile(self).role != Role::Worker
    }
    pub fn required_context_policy(self) -> Option<crate::ContextPolicy> {
        profile(self).required_context_policy
    }

    pub fn default_max_concurrent(self) -> Option<u32> {
        profile(self).default_max_concurrent
    }
    pub fn max_models(self) -> usize {
        profile(self).max_models
    }
    pub fn runtime_max_concurrent(self) -> u32 {
        profile(self).runtime_max_concurrent
    }
}
