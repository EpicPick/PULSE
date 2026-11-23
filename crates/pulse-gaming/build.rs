fn main() -> std::io::Result<()> {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=PULSE_PRESENTMON_BUNDLE");
    let generated =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("presentmon.rs");
    let source = match env::var_os("PULSE_PRESENTMON_BUNDLE") {
        Some(path) if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") => {
            let path = fs::canonicalize(path)?;
            println!("cargo:rerun-if-changed={}", path.display());
            format!(
                "pub const BUNDLED: Option<&[u8]> = Some(include_bytes!({:?}));",
                path.to_str().expect("Unicode bundle path")
            )
        }
        _ => "pub const BUNDLED: Option<&[u8]> = None;".into(),
    };
    fs::write(generated, source)
}
