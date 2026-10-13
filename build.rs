fn main() {
    println!("cargo:rerun-if-changed=packaging/assets/marzianetto.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource
            .set_icon("packaging/assets/marzianetto.ico")
            .set("ProductName", "marzianetto")
            .set("FileDescription", "marzianetto")
            .set("OriginalFilename", "marzianetto.exe");
        resource
            .compile()
            .expect("failed to embed Windows application icon");
    }
}
