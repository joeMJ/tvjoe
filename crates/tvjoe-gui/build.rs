//! Windows: Programmsymbol einbinden (packaging/windows/tvjoe.rc, übersetzt mit windres aus MSYS2).
//! Fehlt windres, wird ohne Symbol gebaut.

fn main() {
    let rc =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/tvjoe.rc");
    println!("cargo:rerun-if-changed={}", rc.display());
    println!(
        "cargo:rerun-if-changed={}",
        rc.with_extension("ico").display()
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("tvjoe-symbol.o");
    let ok = std::process::Command::new("windres")
        .arg("--include-dir")
        .arg(rc.parent().expect("Ordner"))
        .arg(&rc)
        .args(["-O", "coff", "-o"])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=windres nicht gefunden oder fehlgeschlagen: Programm ohne Symbol");
    }
}
