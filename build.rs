fn main() {
    println!("cargo:rerun-if-changed=assets/molo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/molo.ico")
            .compile()
            .expect("embed the Molo application icon");
    }
}
