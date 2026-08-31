fn main() {
    println!("cargo:rerun-if-env-changed=CHATCMD_BUILD_VERSION");
    println!("cargo:rerun-if-changed=assets/icons/favicon.ico");

    let version = std::env::var("CHATCMD_BUILD_VERSION")
        .or_else(|_| std::env::var("CARGO_PKG_VERSION"))
        .unwrap_or_else(|_| "0.1.0".to_owned());
    println!("cargo:rustc-env=CHATCMD_BUILD_VERSION={version}");

    #[cfg(windows)]
    {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/icons/favicon.ico");
        resource.set("ProductName", "ChatCMD");
        resource.set("FileDescription", "ChatCMD");
        resource.set("FileVersion", &version);
        resource.set("ProductVersion", &version);
        resource.compile().expect("compile Windows resources");
    }
}
