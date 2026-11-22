// Writes a Windows .res file by hand (icon + manifest) so no Windows SDK / rc.exe is needed.
use std::{env, fs, path::PathBuf};

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Aperture" version="1.0.0.0"/>
  <description>Aperture</description>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
    <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
  </windowsSettings></application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
    <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
  </requestedPrivileges></security></trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1"><application>
    <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
  </application></compatibility>
</assembly>
"#;

fn entry(out: &mut Vec<u8>, ty: u16, id: u16, data: &[u8], flags: u16) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend(0xFFFFu16.to_le_bytes());
    out.extend(ty.to_le_bytes());
    out.extend(0xFFFFu16.to_le_bytes());
    out.extend(id.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(flags.to_le_bytes());
    out.extend(if ty == 0 { 0u16 } else { 0x0409u16 }.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/aperture.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return; // the icon/manifest resource is Windows-only
    }
    let mut res = Vec::new();
    entry(&mut res, 0, 0, &[], 0);

    if let Ok(ico) = fs::read("assets/aperture.ico") {
        let count = u16le(&ico, 4) as usize;
        let mut group = Vec::new();
        group.extend(0u16.to_le_bytes());
        group.extend(1u16.to_le_bytes());
        group.extend((count as u16).to_le_bytes());
        for i in 0..count {
            let e = 6 + i * 16;
            let size = u32le(&ico, e + 8) as usize;
            let off = u32le(&ico, e + 12) as usize;
            entry(&mut res, 3, (i + 1) as u16, &ico[off..off + size], 0x1010);
            group.extend(&ico[e..e + 12]); // w,h,colors,reserved,planes,bitcount,bytes
            group.extend(((i + 1) as u16).to_le_bytes());
        }
        entry(&mut res, 14, 1, &group, 0x1030);
    }
    entry(&mut res, 24, 1, MANIFEST.as_bytes(), 0x1030);

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("aperture.res");
    fs::write(&out, res).unwrap();
    println!("cargo:rustc-link-arg-bins={}", out.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}
