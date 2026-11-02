use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let icon = include_bytes!("assets/app-icon.ico");
    let resource = icon_resource(icon).expect("the bundled Beale icon must be a valid ICO");
    let path = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("beale-icon.res");
    std::fs::write(&path, resource).expect("write the Windows icon resource");
    println!(
        "cargo:rustc-link-arg-bin=beale-desktop-app={}",
        path.display()
    );
}

fn icon_resource(icon: &[u8]) -> Result<Vec<u8>, &'static str> {
    if icon.get(0..4) != Some(&[0, 0, 1, 0]) {
        return Err("invalid ICO header");
    }
    let count = usize::from(u16::from_le_bytes(
        icon.get(4..6)
            .ok_or("missing ICO count")?
            .try_into()
            .unwrap(),
    ));
    if count == 0 || count > u16::MAX as usize {
        return Err("invalid ICO entry count");
    }
    let directory_end = 6usize
        .checked_add(count.checked_mul(16).ok_or("ICO directory overflow")?)
        .ok_or("ICO directory overflow")?;
    if icon.len() < directory_end {
        return Err("truncated ICO directory");
    }

    let mut output = Vec::new();
    // Win32 .res files begin with a zero-length, zero-ID header.
    append_resource(&mut output, 0, 0, &[])?;
    let mut group = Vec::with_capacity(6 + count * 14);
    group.extend_from_slice(&icon[..6]);
    for index in 0..count {
        let entry = &icon[6 + 16 * index..6 + 16 * (index + 1)];
        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let end = offset.checked_add(length).ok_or("ICO image overflow")?;
        let image = icon.get(offset..end).ok_or("truncated ICO image")?;
        let id = u16::try_from(index + 1).map_err(|_| "too many ICO entries")?;
        append_resource(&mut output, 3, id, image)?; // RT_ICON
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    append_resource(&mut output, 14, 1, &group)?; // RT_GROUP_ICON
    Ok(output)
}

fn append_resource(
    output: &mut Vec<u8>,
    kind: u16,
    id: u16,
    data: &[u8],
) -> Result<(), &'static str> {
    output.extend_from_slice(
        &u32::try_from(data.len())
            .map_err(|_| "icon resource too large")?
            .to_le_bytes(),
    );
    output.extend_from_slice(&32u32.to_le_bytes()); // header size
    output.extend_from_slice(&0xffffu16.to_le_bytes());
    output.extend_from_slice(&kind.to_le_bytes());
    output.extend_from_slice(&0xffffu16.to_le_bytes());
    output.extend_from_slice(&id.to_le_bytes());
    output.extend_from_slice(&0u32.to_le_bytes()); // data version
    output.extend_from_slice(&0x1030u16.to_le_bytes()); // standard icon memory flags
    output.extend_from_slice(&0u16.to_le_bytes()); // language
    output.extend_from_slice(&0u32.to_le_bytes()); // version
    output.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    output.extend_from_slice(data);
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
    Ok(())
}
