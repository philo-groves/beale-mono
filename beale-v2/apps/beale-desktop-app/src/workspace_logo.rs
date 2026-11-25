use crate::{State, icons::FOLDER_OPEN};
use beale_contracts::{
    v1::WorkspaceSummary,
    workspace_logo::{BYTE_LEN, SIDE},
};
use rune::{Color, El, FileDialogOptions, Pixels, Tx, el, icon};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{Cursor, Read},
    path::Path,
};

const MAX_UPLOAD_BYTES: u64 = 8 * 1024 * 1024;

thread_local! {
    static PIXELS: RefCell<VecDeque<(Vec<u8>, Pixels)>> = const { RefCell::new(VecDeque::new()) };
}

pub(crate) fn mark(workspace: Option<&WorkspaceSummary>, size: f32, color: Color) -> El {
    preview(
        workspace.map_or(&[], |workspace| workspace.logo_rgba.as_slice()),
        size,
        color,
    )
}

pub(crate) fn preview(bytes: &[u8], size: f32, color: Color) -> El {
    if bytes.len() != BYTE_LEN {
        return icon(&FOLDER_OPEN, size).color(color);
    }
    let pixels = PIXELS.with_borrow_mut(|cache| {
        if let Some((_, pixels)) = cache.iter().find(|(data, _)| data == bytes) {
            return pixels.clone();
        }
        let pixels = Pixels::new();
        pixels.set(SIDE, SIDE, bytes.to_vec());
        if cache.len() >= 128 {
            cache.pop_front();
        }
        cache.push_back((bytes.to_vec(), pixels.clone()));
        pixels
    });
    el().size(size, size).pixels(&pixels)
}

fn load(path: &Path) -> Result<Vec<u8>, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "Could not read the selected logo.")?;
    let mut bytes = Vec::new();
    file.take(MAX_UPLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the selected logo.")?;
    if bytes.len() as u64 > MAX_UPLOAD_BYTES {
        return Err("Choose a logo smaller than 8 MB.");
    }
    decode(&bytes)
}

fn decode(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Choose a valid PNG, JPEG, or WebP image.")?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP)
    ) {
        return Err("Choose a PNG, JPEG, or WebP image.");
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Could not decode the logo. Choose an image up to 4096 × 4096 pixels.")?;
    let thumbnail = image.thumbnail(SIDE, SIDE).to_rgba8();
    let mut square = image::RgbaImage::new(SIDE, SIDE);
    image::imageops::overlay(
        &mut square,
        &thumbnail,
        i64::from((SIDE - thumbnail.width()) / 2),
        i64::from((SIDE - thumbnail.height()) / 2),
    );
    Ok(square.into_raw())
}

pub(crate) fn choose(state: &mut State, tx: &mut Tx) {
    if state.workspace_save_busy.is_some() {
        return;
    }
    let workspace = state.workspace_id;
    let generation = state.workspace_details_generation;
    tx.open_file(
        FileDialogOptions {
            title: "Choose Workspace Logo".to_owned(),
            ..Default::default()
        }
        .filter("Images", &["png", "jpg", "jpeg", "webp"]),
        move |state: &mut State, result, _| {
            if state.workspace_id != workspace
                || state.workspace_details_generation != generation
                || state.workspace_save_busy.is_some()
            {
                return;
            }
            match result {
                Ok(Some(path)) => match load(&path) {
                    Ok(bytes) => {
                        state.workspace_logo_edit = bytes;
                        state.workspace_save_error.clear();
                        state.workspace_save_message.clear();
                    }
                    Err(message) => state.workspace_save_error = message.to_owned(),
                },
                Ok(None) => {}
                Err(_) => state.workspace_save_error = "Could not open the logo picker.".to_owned(),
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_picker_supports_cancel_upload_and_removal() {
        use rune::{Env, Runtime};
        let mut runtime = Runtime::new(
            crate::visual_preview::fixture("workspace"),
            crate::view,
            (1440.0, 960.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        runtime.frame(1.0);
        let click = |runtime: &mut Runtime<State>, key: &str| {
            let (x, y) = runtime.find(key).unwrap();
            runtime.pointer_down(x, y);
            runtime.pointer_up(x, y);
            runtime.frame(1.0);
        };
        assert!(runtime.find("saved-workspace-save-disabled").is_some());
        click(&mut runtime, "workspace-logo-upload");
        assert_eq!(runtime.file_dialog().unwrap().0, rune::FileDialogKind::Open);
        runtime.resolve_file_dialog(None).unwrap();
        runtime.frame(1.0);
        assert!(runtime.state.workspace_logo_edit.is_empty());
        let path =
            std::env::temp_dir().join(format!("beale-example-logo-{}.png", uuid::Uuid::new_v4()));
        image::RgbaImage::from_pixel(128, 64, image::Rgba([20, 100, 200, 255]))
            .save(&path)
            .unwrap();
        click(&mut runtime, "workspace-logo-upload");
        runtime.resolve_file_dialog(Some(path.clone())).unwrap();
        runtime.frame(1.0);
        assert_eq!(runtime.state.workspace_logo_edit.len(), BYTE_LEN);
        assert!(runtime.find("saved-workspace-save").is_some());
        assert!(
            runtime.state.workspaces[0].logo_rgba.is_empty(),
            "upload remains a draft until saved"
        );
        let request =
            crate::foundation_request(&runtime.state, crate::FoundationSaveTarget::Identity);
        let logo = request.logo_rgba.unwrap();
        let id = runtime.state.workspace_id.unwrap();
        runtime.state.workspace_save_busy = Some(crate::workspace_view::WorkspaceTab::Foundation);
        let mut summary = runtime.state.workspaces[0].clone();
        summary.logo_rgba = logo.clone();
        runtime.state.finish_workspace_save(
            id,
            crate::workspace_view::WorkspaceTab::Foundation,
            Ok(crate::WorkspaceSaveResult::Foundation(
                beale_contracts::v1::WorkspaceFoundation {
                    summary: Some(summary),
                    guidance: request.guidance,
                    revision: 2,
                },
                crate::FoundationSaveTarget::Identity,
            )),
        );
        runtime = Runtime::new(
            runtime.state,
            crate::view,
            (1440.0, 960.0),
            Env::new(true, 1.0),
        );
        runtime.frame(0.0);
        runtime.frame(1.0);
        assert_eq!(runtime.state.workspaces[0].logo_rgba, logo);
        assert!(runtime.find("saved-workspace-save-disabled").is_some());
        click(&mut runtime, "workspace-logo-remove");
        assert!(runtime.state.workspace_logo_edit.is_empty());
        assert!(runtime.find("saved-workspace-save").is_some());
        assert_eq!(
            crate::foundation_request(&runtime.state, crate::FoundationSaveTarget::Identity)
                .logo_rgba,
            Some(Vec::new())
        );
        // A late picker result cannot apply to a newly loaded workspace.
        click(&mut runtime, "workspace-logo-upload");
        runtime.state.workspace_details_generation += 1;
        runtime.resolve_file_dialog(Some(path.clone())).unwrap();
        runtime.frame(1.0);
        assert!(runtime.state.workspace_logo_edit.is_empty());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn normalizes_images_without_stretching_and_preserves_transparency() {
        let source = image::RgbaImage::from_pixel(128, 64, image::Rgba([20, 100, 200, 128]));
        let mut encoded = Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let result = decode(encoded.get_ref()).unwrap();
        assert_eq!(result.len(), BYTE_LEN);
        assert_eq!(&result[..4], &[0, 0, 0, 0]);
        let center = ((SIDE / 2 * SIDE + SIDE / 2) * 4) as usize;
        assert_eq!(&result[center..center + 4], &[20, 100, 200, 128]);
        assert!(decode(b"not an image").is_err());
    }
}
