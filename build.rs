//! Windows: puts the app icon (packaging/windows/dost.ico) into dost.exe, so Explorer, the
//! taskbar and Alt+Tab show the paw. The `.res` file is written here directly, so no resource
//! compiler (rc.exe / windres) is needed, also when cross-compiling with cargo-xwin.

use std::path::PathBuf;

const ICO: &str = "packaging/windows/dost.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICO}");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::fs::read(ICO).expect("packaging/windows/dost.ico");
    let res = to_res(&ico).expect("dost.ico is not a valid icon file");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("dost-icon.res");
    std::fs::write(&out, res).unwrap();
    // Only the binary: tests and examples do not need it.
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// ICO file -> 32-bit `.res`: one RT_ICON per image and an RT_GROUP_ICON (id 1) listing them.
fn to_res(ico: &[u8]) -> Option<Vec<u8>> {
    let u16_at = |o: usize| Some(u16::from_le_bytes(ico.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |o: usize| Some(u32::from_le_bytes(ico.get(o..o + 4)?.try_into().ok()?));
    if u16_at(0)? != 0 || u16_at(2)? != 1 {
        return None;
    }
    let count = u16_at(4)? as usize;
    let mut res = Vec::new();
    // The empty resource every .res file starts with.
    resource(&mut res, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let e = 6 + i * 16;
        let entry = ico.get(e..e + 12)?;
        let size = u32_at(e + 8)? as usize;
        let offset = u32_at(e + 12)? as usize;
        let data = ico.get(offset..offset + size)?;
        let id = (i + 1) as u16;
        // RT_ICON = 3
        resource(&mut res, 3, id, data);
        // GRPICONDIRENTRY: the ICONDIRENTRY's first 12 bytes, then the icon's id.
        group.extend_from_slice(entry);
        group.extend_from_slice(&id.to_le_bytes());
    }
    // RT_GROUP_ICON = 14; id 1 is what Explorer shows.
    resource(&mut res, 14, 1, &group);
    Some(res)
}

/// One resource entry with a numeric type and name, language neutral.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    let header_size: u32 = 32;
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_size.to_le_bytes());
    out.extend_from_slice(&[0xff, 0xff]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xff, 0xff]);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // data version
    // MOVEABLE | PURE (| DISCARDABLE for icons), as rc.exe writes them.
    let flags: u16 = if kind == 0 { 0 } else { 0x1030 };
    out.extend_from_slice(&flags.to_le_bytes());
    let lang: u16 = if kind == 0 { 0 } else { 0x0409 };
    out.extend_from_slice(&lang.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // characteristics
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}
