//! Exact-match redaction of host-held secrets from model-visible output.
//!
//! The host knows the credential values it stores or can read. Removing exact
//! occurrences before tool output is saved or returned to a model keeps an
//! accidental `cat` or `env` from copying them into transcripts. Encoded or
//! transformed values are not detected; this is a last line of defense behind
//! the scrubbed tool environment, not a guarantee.

use std::borrow::Cow;

/// Replacement text for a redacted value.
pub const REDACTED: &str = "[redacted host secret]";

/// Shorter values are skipped to avoid redacting ordinary words or numbers.
pub const MIN_SECRET_BYTES: usize = 12;

#[derive(Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(values: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = values
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| value.len() >= MIN_SECRET_BYTES)
            .collect();
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then(left.cmp(right)));
        secrets.dedup();
        Self { secrets }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut output = Cow::Borrowed(text);
        for secret in &self.secrets {
            if output.contains(secret.as_str()) {
                output = Cow::Owned(output.replace(secret.as_str(), REDACTED));
            }
        }
        output
    }

    /// Redacts every string and object key in a JSON value. Structure, numbers
    /// and booleans are kept, so the result stays a valid document of the same
    /// shape. Escaped forms of a secret are matched because strings are
    /// compared after JSON decoding.
    pub fn redact_json(&self, value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        if self.is_empty() {
            return value;
        }
        match value {
            Value::String(text) => match self.redact(&text) {
                Cow::Borrowed(_) => Value::String(text),
                Cow::Owned(redacted) => Value::String(redacted),
            },
            Value::Array(values) => {
                Value::Array(values.into_iter().map(|v| self.redact_json(v)).collect())
            }
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (self.redact(&key).into_owned(), self.redact_json(value)))
                    .collect(),
            ),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_redaction_covers_nested_strings_keys_and_escaped_values() {
        let secret = "example\"secret\\value-0001";
        let redactor = Redactor::new([secret.to_owned()]);
        let value = serde_json::json!({
            "role": "tool",
            "content": [{"type": "text", "text": format!("token={secret}")}],
            secret: 7,
            "count": 3,
            "ok": true,
        });
        let redacted = redactor.redact_json(value);
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains("example\\\"secret"));
        assert_eq!(redacted["content"][0]["text"], format!("token={REDACTED}"));
        assert_eq!(redacted[REDACTED], 7);
        assert_eq!(redacted["count"], 3);
        assert_eq!(redacted["ok"], true);
    }

    #[test]
    fn redacts_longest_values_first_and_ignores_short_ones() {
        let redactor = Redactor::new([
            "example-secret-value".to_owned(),
            "example-secret-value-extended".to_owned(),
            "short".to_owned(),
        ]);
        assert_eq!(
            redactor.redact("a example-secret-value-extended b example-secret-value short"),
            format!("a {REDACTED} b {REDACTED} short")
        );
    }

    #[test]
    fn untouched_text_is_borrowed() {
        let redactor = Redactor::new(["example-secret-value".to_owned()]);
        assert!(matches!(redactor.redact("nothing here"), Cow::Borrowed(_)));
        assert!(Redactor::new(Vec::new()).is_empty());
    }
}
