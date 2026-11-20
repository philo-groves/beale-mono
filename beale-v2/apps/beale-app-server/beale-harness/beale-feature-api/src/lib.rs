//! The descriptor each native feature crate exports as `FEATURE`.
//!
//! The host feature catalog and the session harness read one descriptor per
//! feature instead of restating its identity, guidance, tools, review policy,
//! and subagent availability. Feature execution stays behind narrow, typed
//! harness hooks because features need different host resources.

use serde_json::Value;

/// How a feature's model tools pass Auto-Review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolReview {
    /// Every tool call is reviewed before it runs.
    Required,
    /// Calls run without model review, such as harness feedback or delegation.
    Exempt,
}

#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Guidance published to sessions as `FEATURE.md`; empty when none.
    pub prompt_extension: &'static str,
    /// One-line use case listed in the session extension catalog.
    pub use_case: &'static str,
    /// Every model tool name the feature can offer.
    pub tool_names: &'static [&'static str],
    /// Static tool definitions, or `None` when the session builds them from
    /// runtime state (such as the selected graph).
    pub definitions: Option<fn() -> Vec<Value>>,
    pub review: ToolReview,
    /// Whether graph subagents receive the feature's tools and guidance.
    pub subagents: bool,
    /// Ordered schema versions for tables the feature owns in the host
    /// database, applied under the feature ID at host start.
    pub migrations: &'static [&'static str],
    /// Statements run at every host start after migrations, such as marking
    /// work that was running when the host stopped as interrupted.
    pub startup: &'static [&'static str],
}

impl Feature {
    pub fn owns_tool(&self, name: &str) -> bool {
        self.tool_names.contains(&name)
    }

    pub fn has_prompt_extension(&self) -> bool {
        !self.prompt_extension.is_empty()
    }

    pub fn tool_count(&self) -> u32 {
        u32::try_from(self.tool_names.len()).unwrap_or(u32::MAX)
    }

    /// Whether a call to `name` needs Auto-Review under this feature.
    pub fn reviews(&self, name: &str) -> bool {
        self.review == ToolReview::Required && self.owns_tool(name)
    }

    /// Names in the static definitions, for consistency checks.
    pub fn definition_names(&self) -> Option<Vec<String>> {
        self.definitions.map(|definitions| {
            definitions()
                .iter()
                .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn example_tools() -> Vec<Value> {
        vec![json!({"type":"function","name":"example_query"})]
    }

    const EXAMPLE: Feature = Feature {
        id: "example",
        name: "Example",
        description: "Synthetic feature.",
        prompt_extension: "",
        use_case: "Use for synthetic checks.",
        tool_names: &["example_query"],
        definitions: Some(example_tools),
        review: ToolReview::Required,
        subagents: false,
        migrations: &[],
        startup: &[],
    };

    #[test]
    fn descriptor_derives_catalog_and_review_facts() {
        assert!(EXAMPLE.owns_tool("example_query"));
        assert!(EXAMPLE.reviews("example_query"));
        assert!(!EXAMPLE.reviews("read"));
        assert!(!EXAMPLE.has_prompt_extension());
        assert_eq!(EXAMPLE.tool_count(), 1);
        assert_eq!(
            EXAMPLE.definition_names(),
            Some(vec!["example_query".to_owned()])
        );
        let exempt = Feature {
            review: ToolReview::Exempt,
            ..EXAMPLE
        };
        assert!(!exempt.reviews("example_query"));
    }
}
