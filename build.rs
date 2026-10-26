// Windows: compiles assets/dfc.rc (icons + manifest) with windres and links it into dfc.exe.
// The host is Linux when cross-compiling, so look at the target, not cfg!(windows).
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
        if cfg!(windows) { "windres".into() } else { "x86_64-w64-mingw32-windres".into() }
    });
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("dfc-res.o");
    let status = Command::new(&windres)
        .args(["--input", "assets/dfc.rc", "--include-dir", "assets", "--output-format", "coff", "--output"])
        .arg(&out)
        .status()
        .unwrap_or_else(|e| panic!("{windres}: {e} (install mingw-w64-binutils, or set WINDRES)"));
    assert!(status.success(), "{windres} failed on assets/dfc.rc");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}
