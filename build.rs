//! Embeds the app icon and the version details (name, company, copyright) in the Windows exe, where Explorer and the
//! taskbar show them.

fn main() {
    println!("cargo:rerun-if-changed=assets/Tech_Planet.ico");
    // Only the op2dll program has the icon; a program using the library alone gets none of it.
    #[cfg(windows)]
    if std::env::var_os("CARGO_FEATURE_APP").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/Tech_Planet.ico");
        // The exe's Properties > Details in Explorer.
        res.set("FileDescription", "OP2MissionReader");
        res.set("ProductName", "OP2MissionReader");
        res.set("CompanyName", "Outpost Universe");
        res.set("LegalCopyright", "Outpost Universe © 2026");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
