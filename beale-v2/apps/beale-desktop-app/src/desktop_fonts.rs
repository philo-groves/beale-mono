use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub(crate) struct FontSpec {
    pub fonts: &'static [&'static [u8]],
    pub families: (&'static str, &'static str),
}

pub(crate) fn v1_font_spec() -> FontSpec {
    static SPEC: OnceLock<FontSpec> = OnceLock::new();
    *SPEC.get_or_init(platform_fonts)
}

fn load_font(path: &Path, fonts: &mut Vec<&'static [u8]>) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    fonts.push(Box::leak(bytes.into_boxed_slice()));
    true
}

fn finish(fonts: Vec<&'static [u8]>, families: (&'static str, &'static str)) -> FontSpec {
    FontSpec {
        fonts: Box::leak(fonts.into_boxed_slice()),
        families,
    }
}

#[cfg(target_os = "windows")]
fn platform_fonts() -> FontSpec {
    let mut fonts = Vec::new();
    let system_dir = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(|root| std::path::PathBuf::from(root).join("Fonts"));
    let user_dir = std::env::var_os("LOCALAPPDATA")
        .map(|root| std::path::PathBuf::from(root).join("Microsoft/Windows/Fonts"));
    for dir in [system_dir.as_deref(), user_dir.as_deref()]
        .into_iter()
        .flatten()
    {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("inter") && (name.ends_with(".ttf") || name.ends_with(".otf")) {
                load_font(&entry.path(), &mut fonts);
            }
        }
    }
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
    if fonts.is_empty() {
        return FontSpec {
            fonts: &[],
            families: ("Inter", "Inter"),
        };
    }
    finish(
        fonts,
        (
            "Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
            "'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
        ),
    )
}

#[cfg(target_os = "macos")]
fn platform_fonts() -> FontSpec {
    // Rune loads the system UI face for symbols and ui-monospace resolves SF Mono.
    FontSpec {
        fonts: &[],
        families: (
            "System Font, '.SF NS', Inter, ui-sans-serif, sans-serif",
            "ui-monospace, Menlo, monospace",
        ),
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

    let mut fonts = Vec::new();
    let sans = match_font("Inter")
        .filter(|(family, _)| family == "Inter")
        .or_else(|| match_font("sans-serif"));
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
    if fonts.is_empty() {
        return FontSpec {
            fonts: &[],
            families: ("Inter", "Inter"),
        };
    }
    let sans_family = sans.map_or("Inter", |(name, _)| Box::leak(name.into_boxed_str()));
    let mono_family = mono.map_or("Inter", |(name, _)| Box::leak(name.into_boxed_str()));
    finish(fonts, (sans_family, mono_family))
}
