// Embeds assets/icon.ico (the same image the window uses, assets/icon.png) as
// the exe's icon resource, so Explorer, shortcuts and the taskbar match the app.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to compile the Windows icon resource");
    }
}
