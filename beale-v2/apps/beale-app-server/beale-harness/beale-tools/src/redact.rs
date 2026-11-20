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
}

#[cfg(test)]
mod tests {
    use super::*;

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
