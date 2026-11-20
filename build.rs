// Windows: put ÁrbolView's icon and name into the .exe (Explorer, taskbar, Alt+Tab).
fn main() {
    println!("cargo:rerun-if-changed=packaging/arbolview.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("packaging/arbolview.ico");
    res.set("ProductName", "ÁrbolView");
    res.set("FileDescription", "ÁrbolView — disk usage");
    res.set("OriginalFilename", "arbolview.exe");
    if let Err(e) = res.compile() {
        // cross-checks without a resource compiler still build, just without the icon
        println!("cargo:warning=icon not embedded: {e}");
    }
}
