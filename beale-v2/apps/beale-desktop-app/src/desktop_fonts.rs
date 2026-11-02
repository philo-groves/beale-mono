#[cfg(not(target_os = "macos"))]
use std::path::Path;
use std::sync::OnceLock;

use crate::appearance::InterfaceFont;

#[derive(Clone, Copy)]
pub(crate) struct FontSpec {
    pub fonts: &'static [&'static [u8]],
    pub families: (&'static str, &'static str),
    pub system_sans: &'static str,
}

const INTER_FONTS: [&[u8]; 3] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
];

#[cfg(not(target_os = "macos"))]
fn bundled_inter() -> Vec<&'static [u8]> {
    INTER_FONTS.to_vec()
}

pub(crate) fn v1_font_spec() -> FontSpec {
    static SPEC: OnceLock<FontSpec> = OnceLock::new();
    *SPEC.get_or_init(platform_fonts)
}

pub(crate) fn families(choice: InterfaceFont) -> (&'static str, &'static str) {
    let spec = v1_font_spec();
    let sans = match choice {
        InterfaceFont::Beale => spec.families.0,
        InterfaceFont::System => spec.system_sans,
    };
    (sans, spec.families.1)
}

#[cfg(not(target_os = "macos"))]
fn load_font(path: &Path, fonts: &mut Vec<&'static [u8]>) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    fonts.push(Box::leak(bytes.into_boxed_slice()));
    true
}

#[cfg(not(target_os = "macos"))]
fn finish(
    fonts: Vec<&'static [u8]>,
    families: (&'static str, &'static str),
    system_sans: &'static str,
) -> FontSpec {
    FontSpec {
        fonts: Box::leak(fonts.into_boxed_slice()),
        families,
        system_sans,
    }
}

#[cfg(target_os = "windows")]
fn platform_fonts() -> FontSpec {
    let mut fonts = bundled_inter();
    let system_dir = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(|root| std::path::PathBuf::from(root).join("Fonts"));
    if let Some(dir) = system_dir {
        for file in [
            "segoeui.ttf",
            "seguisb.ttf",
            "segoeuib.ttf",
            "segoeuii.ttf",
            "consola.ttf",
            "consolab.ttf",
        ] {
            load_font(&dir.join(file), &mut fonts);
        }
    }
    finish(
        fonts,
        (
            "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
            "'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
        ),
        "'Segoe UI', system-ui, sans-serif",
    )
}

#[cfg(target_os = "macos")]
fn platform_fonts() -> FontSpec {
    // Rune loads the system UI face for symbols and ui-monospace resolves SF Mono.
    FontSpec {
        fonts: &INTER_FONTS,
        families: (
            "Inter, System Font, '.SF NS', ui-sans-serif, sans-serif",
            "ui-monospace, Menlo, monospace",
        ),
        system_sans: "System Font, '.SF NS', ui-sans-serif, sans-serif",
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_fonts() -> FontSpec {
    use std::process::Command;

    fn match_font(pattern: &str) -> Option<(String, std::path::PathBuf)> {
        let output = Command::new("fc-match")
            .args(["--format=%{family[0]}\n%{file}", pattern])
            .output()
            .ok()?;
        let output = String::from_utf8(output.stdout).ok()?;
        let (family, path) = output.split_once('\n')?;
        (!family.is_empty() && !path.trim().is_empty())
            .then(|| (family.to_owned(), path.trim().into()))
    }

    let mut fonts = bundled_inter();
    let sans = match_font("sans-serif");
    let mono = match_font("Liberation Mono").or_else(|| match_font("monospace"));
    if let Some((family, path)) = &sans {
        load_font(path, &mut fonts);
        if let Some((_, bold)) = match_font(&format!("{family}:style=Bold")) {
            load_font(&bold, &mut fonts);
        }
    }
    if let Some((family, path)) = &mono {
        load_font(path, &mut fonts);
        if let Some((_, bold)) = match_font(&format!("{family}:style=Bold")) {
            load_font(&bold, &mut fonts);
        }
    }
    let sans_family = sans.map_or("sans-serif", |(name, _)| Box::leak(name.into_boxed_str()));
    let mono_family = mono.map_or("Inter", |(name, _)| Box::leak(name.into_boxed_str()));
    finish(
        fonts,
        ("Inter, ui-sans-serif, sans-serif", mono_family),
        sans_family,
    )
}
