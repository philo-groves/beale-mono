use rune::Color;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AppearanceTheme {
    Light,
    #[default]
    Dark,
    Cream,
    Midnight,
}

impl AppearanceTheme {
    pub(crate) const ALL: [Self; 4] = [Self::Light, Self::Dark, Self::Cream, Self::Midnight];

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Light => 0,
            Self::Dark => 1,
            Self::Cream => 2,
            Self::Midnight => 3,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Cream => "Cream",
            Self::Midnight => "Midnight",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Light => "Bright with cool neutral surfaces.",
            Self::Dark => "The default low-light interface.",
            Self::Cream => "Warm light with soft brown surfaces.",
            Self::Midnight => "Dark with deep blue surfaces.",
        }
    }

    pub(crate) fn palette(self) -> Palette<'static> {
        match self {
            Self::Light => Palette {
                chrome: "#e9edf0",
                panel: "#ffffff",
                sidebar: "#f4f6f7",
                card: "#e6eaed",
                raised: "#f1f3f5",
                text: "#15191d",
                muted: "#68737d",
                border: "#ccd3d9",
                gradient_start: "#dcebf5",
                gradient_middle: "#ece1f4",
                gradient_end: "#e3eaee",
                success: "#247d3e",
                warning: "#8a621b",
                error: "#b8333d",
            },
            Self::Dark => Palette {
                chrome: "#151515",
                panel: "#0d0d0d",
                sidebar: "#111111",
                card: "#191919",
                raised: "#242424",
                text: "#f2f2f2",
                muted: "#969696",
                border: "#2a2a2a",
                gradient_start: "#2a2a2a",
                gradient_middle: "#1d1d1d",
                gradient_end: "#151515",
                success: "#72b783",
                warning: "#dca15f",
                error: "#e07676",
            },
            Self::Cream => Palette {
                chrome: "#eadfce",
                panel: "#fffaf1",
                sidebar: "#f6eee2",
                card: "#e6dac8",
                raised: "#f1e7d8",
                text: "#211a14",
                muted: "#746453",
                border: "#cdbb9f",
                gradient_start: "#f4d9bd",
                gradient_middle: "#eadfd0",
                gradient_end: "#d9c7ac",
                success: "#397744",
                warning: "#8a611c",
                error: "#a53b3b",
            },
            Self::Midnight => Palette {
                chrome: "#091321",
                panel: "#08111f",
                sidebar: "#0a1424",
                card: "#192a43",
                raised: "#182a46",
                text: "#edf4fb",
                muted: "#8194aa",
                border: "#2a4264",
                gradient_start: "#18365a",
                gradient_middle: "#242047",
                gradient_end: "#07111f",
                success: "#72d3a0",
                warning: "#ddb768",
                error: "#f47888",
            },
        }
    }
}

fn mapped_color(theme: AppearanceTheme, dark: &str, p: Palette<'_>) -> Color {
    if dark == "#323232" && p.border != theme.palette().border {
        return rune::color::hex(p.border).unwrap_or(Color::BLACK);
    }
    let mapped = match dark {
        "#0d0d0d" => p.panel,
        "#111111" => p.sidebar,
        "#151515" => p.chrome,
        "#191919" => p.card,
        "#242424" => p.raised,
        "#2a2a2a" => p.border,
        "#969696" => p.muted,
        "#f2f2f2" => p.text,
        "#72b783" => p.success,
        "#b9a15e" | "#dca15f" => p.warning,
        "#a83232" | "#cf7777" | "#e07676" => p.error,
        _ => {
            let shades = match dark {
                "#171717" => Some(["#e6eaed", dark, "#e6dac8", "#15253d"]),
                "#1d1d1d" => Some(["#dbe1e5", dark, "#dacbb5", "#1b2d47"]),
                "#222222" => Some(["#d6dce1", dark, "#d5c4ac", "#1e314c"]),
                "#282828" => Some(["#d1d8de", dark, "#cfbca2", "#223652"]),
                "#2c2c2c" => Some(["#c3ccd3", dark, "#c3ad90", "#2d4565"]),
                "#303030" => Some(["#c3ccd3", dark, "#c3ad90", "#2d4565"]),
                "#323232" | "#333333" | "#383838" => Some(["#b9c2ca", dark, "#b8a387", "#334b6b"]),
                "#414141" | "#424242" => Some(["#b9c2ca", dark, "#b8a387", "#334b6b"]),
                "#4a4a4a" | "#555555" => Some(["#929ca5", dark, "#948067", "#536b87"]),
                "#777777" => Some(["#68737d", dark, "#6f604f", "#7489a1"]),
                "#888888" | "#929292" => Some(["#58636d", dark, "#605244", "#879bb1"]),
                "#a8a8a8" => Some(["#454d55", dark, "#4d4136", "#a5b7c9"]),
                "#aaaaaa" => Some(["#414950", dark, "#493d33", "#aabaca"]),
                "#b8b8b8" | "#bcbcbc" | "#bdbdbd" => Some(["#353c43", dark, "#3d332b", "#bbc9d7"]),
                "#c8c8c8" => Some(["#2c3238", dark, "#342b25", "#ccd7e2"]),
                "#e2e2e2" => Some(["#171c20", dark, "#1e1915", "#e6edf3"]),
                "#eeeeee" => Some(["#111519", dark, "#17130f", "#f2f6fa"]),
                _ => None,
            };
            shades.map_or(dark, |values| values[theme.index()])
        }
    };
    rune::color::hex(mapped).unwrap_or(Color::BLACK)
}

#[derive(Clone, Copy)]
pub(crate) struct Palette<'a> {
    pub chrome: &'a str,
    pub panel: &'a str,
    pub sidebar: &'a str,
    pub card: &'a str,
    pub raised: &'a str,
    pub text: &'a str,
    pub muted: &'a str,
    pub border: &'a str,
    pub gradient_start: &'a str,
    pub gradient_middle: &'a str,
    pub gradient_end: &'a str,
    pub success: &'a str,
    pub warning: &'a str,
    pub error: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PaletteColor {
    Chrome,
    Panel,
    Sidebar,
    Card,
    Raised,
    Text,
    Muted,
    Border,
    GradientStart,
    GradientMiddle,
    GradientEnd,
    Success,
    Warning,
    Error,
}

impl PaletteColor {
    pub(crate) const ALL: [Self; 14] = [
        Self::Chrome,
        Self::Panel,
        Self::Sidebar,
        Self::Card,
        Self::Raised,
        Self::Text,
        Self::Muted,
        Self::Border,
        Self::GradientStart,
        Self::GradientMiddle,
        Self::GradientEnd,
        Self::Success,
        Self::Warning,
        Self::Error,
    ];

    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|item| *item == self).unwrap()
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Panel => "panel",
            Self::Sidebar => "sidebar",
            Self::Card => "card",
            Self::Raised => "raised",
            Self::Text => "text",
            Self::Muted => "muted",
            Self::Border => "border",
            Self::GradientStart => "gradient-start",
            Self::GradientMiddle => "gradient-middle",
            Self::GradientEnd => "gradient-end",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Panel => "Panel",
            Self::Sidebar => "Sidebar",
            Self::Card => "Card",
            Self::Raised => "Raised",
            Self::Text => "Text",
            Self::Muted => "Muted",
            Self::Border => "Border",
            Self::GradientStart => "Gradient Start",
            Self::GradientMiddle => "Gradient Middle",
            Self::GradientEnd => "Gradient End",
            Self::Success => "Success",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Chrome => "Window chrome and header.",
            Self::Panel => "Main content surface.",
            Self::Sidebar => "Settings sidebar.",
            Self::Card => "Grouped form surfaces.",
            Self::Raised => "Elevated surfaces.",
            Self::Text => "Primary interface text.",
            Self::Muted => "Secondary interface text.",
            Self::Border => "Dividers and outlines.",
            Self::GradientStart => "Background gradient start.",
            Self::GradientMiddle => "Background gradient middle.",
            Self::GradientEnd => "Background gradient end.",
            Self::Success => "Success status.",
            Self::Warning => "Warning status.",
            Self::Error => "Error status.",
        }
    }

    fn in_palette<'a>(self, palette: Palette<'a>) -> &'a str {
        match self {
            Self::Chrome => palette.chrome,
            Self::Panel => palette.panel,
            Self::Sidebar => palette.sidebar,
            Self::Card => palette.card,
            Self::Raised => palette.raised,
            Self::Text => palette.text,
            Self::Muted => palette.muted,
            Self::Border => palette.border,
            Self::GradientStart => palette.gradient_start,
            Self::GradientMiddle => palette.gradient_middle,
            Self::GradientEnd => palette.gradient_end,
            Self::Success => palette.success,
            Self::Warning => palette.warning,
            Self::Error => palette.error,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ThemeStyle<'a> {
    preferences: &'a AppearancePreferences,
    theme: AppearanceTheme,
}

impl<'a> ThemeStyle<'a> {
    pub(crate) fn is_dark(self) -> bool {
        matches!(
            self.theme,
            AppearanceTheme::Dark | AppearanceTheme::Midnight
        )
    }
    pub(crate) fn name(self) -> &'static str {
        self.theme.name()
    }
    pub(crate) fn description(self) -> &'static str {
        self.theme.description()
    }
    pub(crate) fn palette(self) -> Palette<'a> {
        self.preferences.palette(self.theme)
    }
    pub(crate) fn color(self, dark: &str) -> Color {
        mapped_color(self.theme, dark, self.palette())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HeatLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl HeatLevel {
    pub(crate) const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Critical];
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Low => "Light activity.",
            Self::Medium => "Steady activity.",
            Self::High => "Heavy activity.",
            Self::Critical => "Peak activity.",
        }
    }
}

pub(crate) const HEAT_DEFAULTS: [[&str; 4]; 4] = [
    ["#e6ce7f", "#ce9564", "#d37e83", "#d2565e"],
    ["#857300", "#612d00", "#7e1117", "#5c0006"],
    ["#dbb061", "#d0793e", "#e66565", "#c24747"],
    ["#657be6", "#4b5baa", "#31296a", "#181433"],
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HeatOverrides {
    pub low: Option<String>,
    pub medium: Option<String>,
    pub high: Option<String>,
    pub critical: Option<String>,
}

impl HeatOverrides {
    fn get(&self, level: HeatLevel) -> Option<&str> {
        match level {
            HeatLevel::Low => self.low.as_deref(),
            HeatLevel::Medium => self.medium.as_deref(),
            HeatLevel::High => self.high.as_deref(),
            HeatLevel::Critical => self.critical.as_deref(),
        }
    }
    fn set(&mut self, level: HeatLevel, color: Option<String>) {
        *match level {
            HeatLevel::Low => &mut self.low,
            HeatLevel::Medium => &mut self.medium,
            HeatLevel::High => &mut self.high,
            HeatLevel::Critical => &mut self.critical,
        } = color;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InterfaceFont {
    #[default]
    Beale,
    System,
}

impl InterfaceFont {
    pub(crate) const ALL: [Self; 2] = [Self::Beale, Self::System];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Beale => "Beale",
            Self::System => "System",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TextSize {
    ExtraSmall,
    #[serde(alias = "compact")]
    Small,
    #[default]
    Standard,
    Large,
    ExtraLarge,
}

impl TextSize {
    pub(crate) const ALL: [Self; 5] = [
        Self::ExtraSmall,
        Self::Small,
        Self::Standard,
        Self::Large,
        Self::ExtraLarge,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ExtraSmall => "Extra Small",
            Self::Small => "Small",
            Self::Standard => "Standard",
            Self::Large => "Large",
            Self::ExtraLarge => "Extra Large",
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ExtraSmall => "extra-small",
            Self::Small => "small",
            Self::Standard => "standard",
            Self::Large => "large",
            Self::ExtraLarge => "extra-large",
        }
    }

    pub(crate) fn factor(self) -> f32 {
        match self {
            Self::ExtraSmall => 0.8,
            Self::Small => 0.9,
            Self::Standard => 1.0,
            Self::Large => 1.1,
            Self::ExtraLarge => 1.2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppearancePreferences {
    pub theme: AppearanceTheme,
    pub heat: [HeatOverrides; 4],
    pub palettes: [BTreeMap<PaletteColor, String>; 4],
    pub interface_font: InterfaceFont,
    pub text_size: TextSize,
}

impl Default for AppearancePreferences {
    fn default() -> Self {
        Self {
            theme: AppearanceTheme::Dark,
            heat: std::array::from_fn(|_| HeatOverrides::default()),
            palettes: std::array::from_fn(|_| BTreeMap::new()),
            interface_font: InterfaceFont::Beale,
            text_size: TextSize::Standard,
        }
    }
}

impl AppearancePreferences {
    pub(crate) fn style(&self) -> ThemeStyle<'_> {
        ThemeStyle {
            preferences: self,
            theme: self.theme,
        }
    }

    pub(crate) fn palette_color(&self, theme: AppearanceTheme, field: PaletteColor) -> &str {
        self.palettes[theme.index()]
            .get(&field)
            .map(String::as_str)
            .unwrap_or_else(|| field.in_palette(theme.palette()))
    }

    pub(crate) fn palette_color_is_default(
        &self,
        theme: AppearanceTheme,
        field: PaletteColor,
    ) -> bool {
        self.palette_color(theme, field) == field.in_palette(theme.palette())
    }

    pub(crate) fn set_palette_color(
        &mut self,
        theme: AppearanceTheme,
        field: PaletteColor,
        value: Option<String>,
    ) {
        let colors = &mut self.palettes[theme.index()];
        if let Some(value) = value.filter(|value| value != field.in_palette(theme.palette())) {
            colors.insert(field, value);
        } else {
            colors.remove(&field);
        }
    }

    pub(crate) fn palette(&self, theme: AppearanceTheme) -> Palette<'_> {
        Palette {
            chrome: self.palette_color(theme, PaletteColor::Chrome),
            panel: self.palette_color(theme, PaletteColor::Panel),
            sidebar: self.palette_color(theme, PaletteColor::Sidebar),
            card: self.palette_color(theme, PaletteColor::Card),
            raised: self.palette_color(theme, PaletteColor::Raised),
            text: self.palette_color(theme, PaletteColor::Text),
            muted: self.palette_color(theme, PaletteColor::Muted),
            border: self.palette_color(theme, PaletteColor::Border),
            gradient_start: self.palette_color(theme, PaletteColor::GradientStart),
            gradient_middle: self.palette_color(theme, PaletteColor::GradientMiddle),
            gradient_end: self.palette_color(theme, PaletteColor::GradientEnd),
            success: self.palette_color(theme, PaletteColor::Success),
            warning: self.palette_color(theme, PaletteColor::Warning),
            error: self.palette_color(theme, PaletteColor::Error),
        }
    }

    pub(crate) fn heat_color(&self, theme: AppearanceTheme, level: HeatLevel) -> &str {
        self.heat[theme.index()]
            .get(level)
            .unwrap_or(HEAT_DEFAULTS[theme.index()][level.index()])
    }

    pub(crate) fn heat_color_is_default(&self, theme: AppearanceTheme, level: HeatLevel) -> bool {
        self.heat_color(theme, level) == HEAT_DEFAULTS[theme.index()][level.index()]
    }

    pub(crate) fn set_heat_color(
        &mut self,
        theme: AppearanceTheme,
        level: HeatLevel,
        value: Option<String>,
    ) {
        self.heat[theme.index()].set(level, value);
    }

    pub(crate) fn read(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .unwrap_or_default()
            .validated()
    }

    fn validated(mut self) -> Self {
        for theme in AppearanceTheme::ALL {
            for level in HeatLevel::ALL {
                let value = self.heat[theme.index()].get(level).and_then(normalize_hex);
                self.set_heat_color(theme, level, value);
            }
            for field in PaletteColor::ALL {
                let value = self.palettes[theme.index()]
                    .get(&field)
                    .and_then(|value| normalize_hex(value));
                self.set_palette_color(theme, field, value);
            }
        }
        self
    }

    pub(crate) fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, bytes)
    }
}

pub(crate) fn normalize_hex(value: &str) -> Option<String> {
    let raw = value.trim();
    (raw.len() == 7 && raw.starts_with('#') && raw[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| raw.to_ascii_lowercase())
}

pub(crate) fn preference_path() -> Option<PathBuf> {
    let root = if let Some(root) = std::env::var_os("BEALE_V2_HOME") {
        PathBuf::from(root)
    } else {
        PathBuf::from(std::env::var_os(if cfg!(windows) {
            "USERPROFILE"
        } else {
            "HOME"
        })?)
        .join(".beale")
        .join("v2")
    };
    Some(root.join("desktop-appearance.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_default_validate_and_round_trip() {
        let path =
            std::env::temp_dir().join(format!("beale-appearance-{}.json", uuid::Uuid::new_v4()));
        let mut prefs = AppearancePreferences::read(&path);
        assert_eq!(prefs.theme, AppearanceTheme::Dark);
        assert_eq!(
            prefs.heat_color(AppearanceTheme::Cream, HeatLevel::High),
            "#e66565"
        );
        prefs.theme = AppearanceTheme::Cream;
        prefs.interface_font = InterfaceFont::System;
        prefs.text_size = TextSize::Large;
        prefs.set_heat_color(
            AppearanceTheme::Cream,
            HeatLevel::High,
            Some("#AABBCC".to_owned()),
        );
        prefs.write(&path).unwrap();
        let read = AppearancePreferences::read(&path);
        assert_eq!(read.theme, AppearanceTheme::Cream);
        assert_eq!(read.interface_font, InterfaceFont::System);
        assert_eq!(read.text_size, TextSize::Large);
        assert_eq!(
            read.heat_color(AppearanceTheme::Cream, HeatLevel::High),
            "#aabbcc"
        );
        assert_eq!(
            read.heat_color(AppearanceTheme::Dark, HeatLevel::High),
            "#7e1117"
        );
        std::fs::remove_file(&path).unwrap();

        let prior = serde_json::json!({ "theme": "light" });
        let prior: AppearancePreferences = serde_json::from_value(prior).unwrap();
        assert_eq!(prior.interface_font, InterfaceFont::Beale);
        assert_eq!(prior.text_size, TextSize::Standard);

        let compact: AppearancePreferences =
            serde_json::from_value(serde_json::json!({ "text_size": "compact" })).unwrap();
        assert_eq!(compact.text_size, TextSize::Small);
        assert_eq!(TextSize::ExtraSmall.factor(), 0.8);
        assert_eq!(TextSize::ExtraLarge.factor(), 1.2);
    }

    #[test]
    fn theme_palettes_keep_text_legible_on_light_surfaces() {
        let mut prefs = AppearancePreferences::default();
        for theme in AppearanceTheme::ALL {
            prefs.theme = theme;
            let p = theme.palette();
            assert_ne!(p.panel, p.text);
            assert_ne!(p.chrome, p.text);
            assert_eq!(
                prefs.style().color("#0d0d0d"),
                rune::color::hex(p.panel).unwrap()
            );
        }
        assert_eq!(normalize_hex("#12abcd"), Some("#12abcd".to_owned()));
        assert_eq!(normalize_hex("#abc"), None);
    }

    #[test]
    fn palette_overrides_apply_to_selected_theme_and_round_trip() {
        let path =
            std::env::temp_dir().join(format!("beale-palette-{}.json", uuid::Uuid::new_v4()));
        let mut prefs = AppearancePreferences {
            theme: AppearanceTheme::Light,
            ..AppearancePreferences::default()
        };
        prefs.set_palette_color(
            AppearanceTheme::Light,
            PaletteColor::Panel,
            Some("#123456".to_owned()),
        );
        prefs.set_palette_color(
            AppearanceTheme::Light,
            PaletteColor::GradientStart,
            Some("#AABBCC".to_owned()),
        );
        prefs.write(&path).unwrap();
        let mut read = AppearancePreferences::read(&path);
        assert_eq!(
            read.palette_color(AppearanceTheme::Light, PaletteColor::Panel),
            "#123456"
        );
        assert_eq!(
            read.palette_color(AppearanceTheme::Light, PaletteColor::GradientStart),
            "#aabbcc"
        );
        assert_eq!(
            read.style().color("#0d0d0d"),
            rune::color::hex("#123456").unwrap()
        );
        assert_eq!(
            read.palette(AppearanceTheme::Light).gradient_start,
            "#aabbcc"
        );
        assert!(read.palette_color_is_default(AppearanceTheme::Dark, PaletteColor::Panel));
        read.set_palette_color(AppearanceTheme::Light, PaletteColor::Panel, None);
        assert!(read.palette_color_is_default(AppearanceTheme::Light, PaletteColor::Panel));
        std::fs::remove_file(path).unwrap();
    }
}
