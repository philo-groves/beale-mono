use beale_contracts::v1::{QuickChatMessage, SessionMessage, SessionRole};
use rune::{El, Icon, el, icon};

use crate::{State, ThemeStyle};

pub(crate) fn button(
    key: &'static str,
    label: &'static str,
    glyph: &'static Icon,
    enabled: bool,
    colors: (&str, &str),
    theme: ThemeStyle<'_>,
    on_press: fn(&mut State),
) -> El {
    let mut button = el()
        .key(key)
        .role("button")
        .label(label)
        .help(label)
        .size(24.0, 24.0)
        .center()
        .radius(8.0)
        .continuous_corners()
        .child(icon(glyph, 14.0).color(theme.color(if enabled { colors.0 } else { colors.1 })));
    if enabled {
        button = button.hover(theme.color("#303030")).on_press(on_press);
    }
    button
}

#[derive(Default)]
pub(crate) struct InputHistory {
    index: Option<usize>,
    draft: String,
}

impl InputHistory {
    pub(crate) fn reset(&mut self) {
        self.index = None;
        self.draft.clear();
    }

    pub(crate) fn can_previous(&self, len: usize) -> bool {
        len > 0 && self.index != Some(0)
    }

    pub(crate) fn can_next(&self) -> bool {
        self.index.is_some()
    }

    pub(crate) fn previous(&mut self, entries: &[String], current: &str) -> Option<String> {
        if !self.can_previous(entries.len()) {
            return None;
        }
        let index = self.index.map_or(entries.len() - 1, |index| {
            index.saturating_sub(1).min(entries.len() - 1)
        });
        if self.index.is_none() {
            self.draft = current.to_owned();
        }
        self.index = Some(index);
        Some(entries[index].clone())
    }

    pub(crate) fn next(&mut self, entries: &[String]) -> Option<String> {
        let index = self.index?;
        if index + 1 >= entries.len() {
            self.index = None;
            return Some(std::mem::take(&mut self.draft));
        }
        self.index = Some(index + 1);
        Some(entries[index + 1].clone())
    }
}

pub(crate) fn session_entries(messages: &[SessionMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == SessionRole::User as i32)
        .map(|message| message.text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect()
}

pub(crate) fn quick_chat_entries(messages: &[QuickChatMessage]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == SessionRole::User as i32)
        .map(|message| message.text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_restores_the_unsent_draft_and_stops_at_both_ends() {
        let entries = vec!["First example".to_owned(), "Second example".to_owned()];
        let mut history = InputHistory::default();
        assert!(!history.can_previous(0));
        assert_eq!(
            history.previous(&entries, "Unsent draft"),
            Some(entries[1].clone())
        );
        assert_eq!(
            history.previous(&entries, "Second example"),
            Some(entries[0].clone())
        );
        assert!(!history.can_previous(entries.len()));
        assert_eq!(history.previous(&entries, "First example"), None);
        assert_eq!(history.next(&entries), Some(entries[1].clone()));
        assert_eq!(history.next(&entries), Some("Unsent draft".to_owned()));
        assert!(!history.can_next());
        assert_eq!(history.next(&entries), None);

        history.previous(&entries, "Edited draft");
        history.reset();
        assert!(!history.can_next());
        assert_eq!(
            history.previous(&entries, "New draft"),
            Some(entries[1].clone())
        );
        assert_eq!(history.next(&entries), Some("New draft".to_owned()));
    }

    #[test]
    fn histories_include_only_sent_user_messages() {
        let session = [
            SessionMessage {
                role: SessionRole::User as i32,
                text: " First ".to_owned(),
                ..Default::default()
            },
            SessionMessage {
                role: SessionRole::Assistant as i32,
                text: "Response".to_owned(),
                ..Default::default()
            },
            SessionMessage {
                role: SessionRole::User as i32,
                text: "Second".to_owned(),
                ..Default::default()
            },
        ];
        assert_eq!(session_entries(&session), ["First", "Second"]);
        let chat = [
            QuickChatMessage {
                role: SessionRole::User as i32,
                text: "Question".to_owned(),
            },
            QuickChatMessage {
                role: SessionRole::Assistant as i32,
                text: "Answer".to_owned(),
            },
        ];
        assert_eq!(quick_chat_entries(&chat), ["Question"]);
    }
}
