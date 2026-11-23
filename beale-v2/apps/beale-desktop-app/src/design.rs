//! Shared visual language for the research workbench.
use crate::{ThemeStyle, ui_font};
use rune::tok::TextStyle;
use rune::{Color, El, Icon, col, el, icon, row, text};

pub(crate) const CONTROL_HEIGHT: f32 = 32.0;
pub(crate) const SIDENAV_ROW_HEIGHT: f32 = 32.0;
pub(crate) const SIDENAV_ACTIVE_FOREGROUND: &str = "#f2f2f2";
pub(crate) const CARD_RADIUS: f32 = 12.0;
pub(crate) const PAGE_GAP: f32 = 24.0;
pub(crate) const TAB_ICON_SIZE: f32 = 15.0;
pub(crate) const TAB_ICON_GAP: f32 = 6.0;
/// Trailing fade width, in ems, for single-line text that overflows its box.
pub(crate) const TRUNCATE_FADE_EM: f32 = 2.5;

/// The one overflow treatment for single-line text: clip with a trailing fade, never "…".
pub(crate) trait FadeText {
    fn fade_truncate(self) -> Self;
}

impl FadeText for El {
    fn fade_truncate(self) -> Self {
        self.truncate_fade_width(TRUNCATE_FADE_EM)
    }
}

/// Wrapped text cut to `lines` lines with a bottom fade, the multi-line counterpart of
/// [`FadeText::fade_truncate`]. `width` is the width the text wraps at; the clip and fade
/// apply only when the text needs more lines, and the full text shows on hover.
pub(crate) fn faded_lines(
    content: impl Into<String>,
    font: TextStyle,
    color: Color,
    lines: usize,
    width: f32,
) -> El {
    let content = content.into();
    let line = font.line_px();
    let max_height = line * lines as f32;
    let (_, height) = rune::text::measure(&content, &[], font, Some(width.max(1.0)));
    let body = text(content.clone())
        .w_full()
        .wrap()
        .font(font)
        .color(color);
    if height <= max_height + 0.5 {
        return body;
    }
    col()
        .w_full()
        .min_w(0.0)
        .h(max_height)
        .clip_fade(0.0, 0.0, 0.0, line)
        .help(content)
        .child(body)
}

/// Inner text width of a card in an `equal_columns` grid with `gap` between `columns` cards,
/// `padding` on each side, and a 1 point border.
pub(crate) fn grid_card_text_width(page_width: f32, columns: usize, gap: f32, padding: f32) -> f32 {
    let columns = columns.max(1) as f32;
    ((page_width - gap * (columns - 1.0)) / columns - padding * 2.0 - 2.0).max(1.0)
}
const TAB_ICON_PADDING: f32 = 10.0;
const TAB_TEXT_PADDING: f32 = 13.0;
const TAB_GROUP_PADDING: f32 = 4.0;
const TAB_GROUP_GAP: f32 = 2.0;

pub(crate) fn segmented_group_width(tab_widths: &[f32]) -> f32 {
    tab_widths.iter().sum::<f32>()
        + TAB_GROUP_PADDING * 2.0
        + TAB_GROUP_GAP * tab_widths.len().saturating_sub(1) as f32
}

pub(crate) fn segmented_tab_width(label: &str, has_icon: bool) -> f32 {
    let text_width = rune::text::measure(label, &[], ui_font(13.0, 550.0), None).0;
    text_width
        + if has_icon {
            TAB_ICON_PADDING * 2.0 + TAB_ICON_SIZE + TAB_ICON_GAP
        } else {
            TAB_TEXT_PADDING * 2.0
        }
}

pub(crate) fn tabs_with_control(
    tabs: El,
    tabs_width: f32,
    control: El,
    control_width: f32,
    available_width: f32,
) -> El {
    let content = col().w_full();
    if available_width < tabs_width + 16.0 + control_width {
        content
            .gap(8.0)
            .child(tabs)
            .child(row().w_full().child(el().grow(1.0)).child(control))
    } else {
        content.child(
            row()
                .w_full()
                .center_y()
                .child(tabs)
                .child(el().grow(1.0).min_w(16.0))
                .child(control),
        )
    }
}

pub(crate) fn segmented_control(theme: ThemeStyle<'_>) -> El {
    row()
        .role("tablist")
        .self_start()
        .min_w(0.0)
        .flex_wrap()
        .padding(TAB_GROUP_PADDING)
        .gap(TAB_GROUP_GAP)
        .radius(12.0)
        .continuous_corners()
        .fill(theme.color("#111111"))
        .border(1.0, theme.color("#2a2a2a").alpha(0.65))
}

pub(crate) fn segmented_tab(
    label: impl Into<String>,
    glyph: Option<&'static Icon>,
    selected: bool,
    enabled: bool,
    theme: ThemeStyle<'_>,
) -> El {
    let label = label.into();
    let foreground = if selected {
        theme.accent()
    } else {
        theme.color("#969696")
    };
    let surface = theme.color(if theme.is_dark() {
        "#242424"
    } else {
        "#191919"
    });
    let mut tab = row()
        .role("tab")
        .label(format!(
            "{label}{}",
            if !enabled {
                ", unavailable"
            } else if selected {
                ", selected"
            } else {
                ""
            }
        ))
        .h(CONTROL_HEIGHT)
        .px(if glyph.is_some() {
            TAB_ICON_PADDING
        } else {
            TAB_TEXT_PADDING
        })
        .gap(TAB_ICON_GAP)
        .center()
        .radius(8.0)
        .continuous_corners()
        .border(
            1.0,
            if selected {
                theme.accent().alpha(0.18)
            } else {
                Color::TRANSPARENT
            },
        )
        .fill(if selected {
            surface
        } else {
            Color::TRANSPARENT
        });
    if let Some(glyph) = glyph {
        tab = tab.child(icon(glyph, TAB_ICON_SIZE).color(foreground));
    }
    if !label.is_empty() {
        tab = tab.child(text(label).font(ui_font(13.0, 550.0)).color(foreground));
    }
    if selected {
        tab = tab.shadow(
            1.0,
            3.0,
            Color::BLACK.alpha(if theme.is_dark() { 0.18 } else { 0.07 }),
        );
    }
    if enabled {
        tab = tab.hover(if selected {
            surface
        } else {
            theme.color("#242424").alpha(0.65)
        });
    } else {
        tab = tab.opacity(0.5);
    }
    tab
}

pub(crate) fn segmented_icon_tab(
    label: &'static str,
    glyph: &'static Icon,
    selected: bool,
    theme: ThemeStyle<'_>,
) -> El {
    segmented_tab("", None, selected, true, theme)
        .label(format!(
            "{label}{}",
            if selected { ", selected" } else { "" }
        ))
        .w(CONTROL_HEIGHT)
        .px(0.0)
        .help(label)
        .child(
            icon(glyph, TAB_ICON_SIZE)
                .key(format!("segmented-tab-icon-{}", label.to_lowercase()))
                .color(if selected {
                    theme.accent()
                } else {
                    theme.color("#969696")
                }),
        )
}

impl ThemeStyle<'_> {
    pub(crate) fn accent(self) -> Color {
        rune::color::hex(match self.name() {
            "Light" => "#27634d",
            "Cream" => "#77552c",
            "Midnight" => "#93c5ee",
            _ => "#dedede",
        })
        .unwrap()
    }

    pub(crate) fn accent_ink(self) -> Color {
        rune::color::hex(match self.name() {
            "Light" | "Cream" => "#ffffff",
            "Midnight" => "#14251f",
            _ => "#171717",
        })
        .unwrap()
    }

    pub(crate) fn subtle_accent(self) -> Color {
        self.accent().alpha(0.12)
    }
}

pub(crate) fn eyebrow(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    text(label.into())
        .font(ui_font(10.5, 600.0))
        .color(theme.accent())
}

pub(crate) fn breadcrumb_title(parent: &str, title: El, theme: ThemeStyle<'_>) -> El {
    row()
        .w_full()
        .min_w(0.0)
        .center_y()
        .gap(10.0)
        .child(
            text(parent)
                .font(ui_font(13.0, 500.0))
                .color(theme.color("#969696")),
        )
        .child(icon(&crate::icons::CHEVRON_RIGHT, 13.0).color(theme.color("#969696").alpha(0.65)))
        .child(title.min_w(0.0).grow(1.0))
}

pub(crate) fn subtitle(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    text(label.into())
        .w_full()
        .wrap()
        .font(ui_font(13.0, 400.0))
        .color(theme.color("#969696"))
}

pub(crate) fn badge(label: impl Into<String>, theme: ThemeStyle<'_>) -> El {
    row()
        .px(9.0)
        .py(4.0)
        .radius(6.0)
        .fill(theme.subtle_accent())
        .child(
            text(label.into())
                .font(ui_font(11.0, 550.0))
                .color(theme.accent()),
        )
}

pub(crate) fn primary_button(label: &str, theme: ThemeStyle<'_>) -> El {
    row()
        .h(CONTROL_HEIGHT)
        .px(14.0)
        .center()
        .radius(8.0)
        .fill(theme.accent())
        .hover(theme.accent().alpha(0.85))
        .child(
            text(label)
                .font(ui_font(13.0, 600.0))
                .color(theme.accent_ink()),
        )
}

pub(crate) fn empty_state(
    glyph: &'static Icon,
    title: &str,
    description: &str,
    theme: ThemeStyle<'_>,
) -> El {
    col()
        .w_full()
        .max_w(500.0)
        .self_center()
        .padding(32.0)
        .gap(12.0)
        .child(
            el().size(44.0, 44.0)
                .radius(12.0)
                .center()
                .fill(theme.subtle_accent())
                .child(icon(glyph, 23.0).color(theme.accent())),
        )
        .child(
            text(title)
                .font(ui_font(21.0, 600.0))
                .color(theme.color("#f2f2f2")),
        )
        .child(subtitle(description, theme))
}
