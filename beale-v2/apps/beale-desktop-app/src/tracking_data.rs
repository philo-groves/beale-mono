use std::collections::{BTreeMap, BTreeSet};

use beale_contracts::v1::TrackingModelDay;

pub(crate) const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AgentFilter {
    #[default]
    All,
    Main,
    Subagents,
}

impl AgentFilter {
    pub(crate) const ALL: [Self; 3] = [Self::All, Self::Main, Self::Subagents];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All agents",
            Self::Main => "Main",
            Self::Subagents => "Subagents",
        }
    }

    pub(crate) fn accepts(self, role: &str) -> bool {
        let top_level =
            beale_graphs::ProfileId::from_id(role).is_some_and(|role| role.is_top_level());
        match self {
            Self::All => true,
            Self::Main => top_level,
            Self::Subagents => !role.is_empty() && !top_level,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) calls: u64,
    pub(crate) measured: u64,
    pub(crate) cached: u64,
    pub(crate) cache_input: u64,
    pub(crate) cache_calls: u64,
}

impl Totals {
    pub(crate) fn tokens_unreported(self) -> bool {
        self.calls > 0 && self.measured == 0
    }

    pub(crate) fn tokens(self) -> u64 {
        self.input.saturating_add(self.output)
    }

    pub(crate) fn cache_rate(self) -> Option<f64> {
        (self.cache_calls > 0 && self.cache_input > 0)
            .then(|| self.cached as f64 / self.cache_input as f64 * 100.0)
    }

    fn add(&mut self, day: &TrackingModelDay) {
        self.input = self.input.saturating_add(day.input_tokens);
        self.output = self.output.saturating_add(day.output_tokens);
        self.calls = self.calls.saturating_add(u64::from(day.total_calls));
        self.measured = self.measured.saturating_add(u64::from(day.measured_calls));
        self.cached = self
            .cached
            .saturating_add(day.cache_read_tokens.unwrap_or(0));
        self.cache_input = self
            .cache_input
            .saturating_add(day.cache_input_tokens.unwrap_or(0));
        self.cache_calls = self
            .cache_calls
            .saturating_add(u64::from(day.cache_measured_calls.unwrap_or(0)));
    }
}

#[derive(Default)]
pub(crate) struct ModelTotal {
    pub(crate) provider: i32,
    pub(crate) method: i32,
    pub(crate) model: String,
    pub(crate) roles: BTreeSet<String>,
    pub(crate) total: Totals,
}

pub(crate) struct Summary {
    pub(crate) first_day: u64,
    pub(crate) days: [Totals; 7],
    pub(crate) total: Totals,
    pub(crate) models: Vec<ModelTotal>,
    pub(crate) selected_day: Option<u64>,
}

pub(crate) fn summarize(
    rows: &[TrackingModelDay],
    now: u64,
    filter: AgentFilter,
    selected: Option<u64>,
) -> Summary {
    let today = now / DAY_MS * DAY_MS;
    let first_day = today.saturating_sub(6 * DAY_MS);
    let selected_day =
        selected.filter(|day| *day >= first_day && *day <= today && *day % DAY_MS == 0);
    let mut days = [Totals::default(); 7];
    let mut total = Totals::default();
    let mut models = BTreeMap::<(i32, i32, String), ModelTotal>::new();
    for day in rows.iter().filter(|day| {
        day.day_start_ms >= first_day && day.day_start_ms <= today && filter.accepts(&day.role)
    }) {
        let index = ((day.day_start_ms - first_day) / DAY_MS) as usize;
        days[index].add(day);
        if selected_day.is_some_and(|selected| selected != day.day_start_ms) {
            continue;
        }
        total.add(day);
        let model = models
            .entry((day.provider, day.method, day.model.clone()))
            .or_insert_with(|| ModelTotal {
                provider: day.provider,
                method: day.method,
                model: day.model.clone(),
                ..Default::default()
            });
        model.roles.insert(day.role.clone());
        model.total.add(day);
    }
    let mut models: Vec<_> = models.into_values().collect();
    models.sort_by(|a, b| {
        b.total
            .tokens()
            .cmp(&a.total.tokens())
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.provider.cmp(&b.provider))
            .then_with(|| a.method.cmp(&b.method))
    });
    Summary {
        first_day,
        days,
        total,
        models,
        selected_day,
    }
}

pub(crate) fn valid_cache(day: &TrackingModelDay) -> bool {
    match (
        day.cache_read_tokens,
        day.cache_input_tokens,
        day.cache_measured_calls,
    ) {
        (None, None, None) => true,
        (Some(cached), Some(input), Some(calls)) => {
            cached <= input
                && input <= day.input_tokens
                && calls <= day.total_calls
                && (calls > 0 || input == 0)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_roles_are_top_level_for_usage_filters() {
        for role in [
            "main",
            "discovery_harness",
            "validation_system",
            "discovery",
            "validation",
            "reporter",
            "patch_generation",
        ] {
            assert!(AgentFilter::Main.accepts(role));
            assert!(!AgentFilter::Subagents.accepts(role));
        }
        for role in ["scout", "reviewer", "prover"] {
            assert!(!AgentFilter::Main.accepts(role));
            assert!(AgentFilter::Subagents.accepts(role));
        }
    }

    fn day(at: u64, role: &str) -> TrackingModelDay {
        TrackingModelDay {
            day_start_ms: at,
            provider: 1,
            method: 1,
            model: "example-model".into(),
            role: role.into(),
            input_tokens: 100,
            output_tokens: 20,
            measured_calls: 1,
            total_calls: 2,
            cache_read_tokens: Some(40),
            cache_input_tokens: Some(100),
            cache_measured_calls: Some(1),
            ..Default::default()
        }
    }

    #[test]
    fn filters_keep_daily_context_and_separate_authentication_sources() {
        let today = 20 * DAY_MS;
        let mut api = day(today, "scout");
        api.method = 2;
        let rows = [
            day(today - DAY_MS, "main"),
            day(today, "main"),
            api,
            day(today - 7 * DAY_MS, "main"),
            day(today + DAY_MS, "main"),
        ];
        let all = summarize(&rows, today + 123, AgentFilter::All, Some(today));
        assert_eq!(all.total.tokens(), 240);
        assert_eq!(all.days[5].tokens(), 120);
        assert_eq!(all.models.len(), 2);
        assert_eq!(all.total.cache_rate(), Some(40.0));
        assert_eq!(all.total.measured, 2);
        assert_eq!(all.total.calls, 4);
        let main = summarize(&rows, today, AgentFilter::Main, None);
        assert_eq!(main.total.tokens(), 240);
        let sub = summarize(&rows, today, AgentFilter::Subagents, None);
        assert_eq!(sub.total.tokens(), 120);
        assert_eq!(sub.models[0].roles.iter().next().unwrap(), "scout");
    }

    #[test]
    fn missing_cache_is_not_a_zero_hit_rate_and_stale_selection_resets() {
        let today = 20 * DAY_MS;
        let mut legacy = day(today, "main");
        legacy.cache_read_tokens = None;
        legacy.cache_input_tokens = None;
        legacy.cache_measured_calls = None;
        assert!(valid_cache(&legacy));
        let summary = summarize(
            &[legacy.clone()],
            today,
            AgentFilter::All,
            Some(today - 7 * DAY_MS),
        );
        assert_eq!(summary.selected_day, None);
        assert_eq!(summary.total.cache_rate(), None);
        assert!(!summary.total.tokens_unreported());
        let unknown = Totals {
            calls: 2,
            ..Default::default()
        };
        assert!(unknown.tokens_unreported());
        assert!(
            !Totals {
                measured: 2,
                ..unknown
            }
            .tokens_unreported()
        );
        assert!(!Totals::default().tokens_unreported());
        legacy.cache_read_tokens = Some(101);
        assert!(!valid_cache(&legacy));
        legacy.cache_input_tokens = Some(100);
        legacy.cache_measured_calls = Some(1);
        assert!(!valid_cache(&legacy));
    }

    #[test]
    fn totals_saturate_instead_of_overflowing() {
        let today = 20 * DAY_MS;
        let mut row = day(today, "main");
        row.input_tokens = u64::MAX;
        let summary = summarize(&[row.clone(), row], today, AgentFilter::All, None);
        assert_eq!(summary.total.tokens(), u64::MAX);
    }
}
