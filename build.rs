fn main() {
    println!("cargo:rerun-if-env-changed=CHATCMD_BUILD_VERSION");
    println!("cargo:rerun-if-changed=assets/icons/favicon.ico");

    #[cfg(windows)]
    {
        let version = std::env::var("CHATCMD_BUILD_VERSION").unwrap_or_else(|_| "0.1.0".to_owned());
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/icons/favicon.ico");
        resource.set("ProductName", "ChatCMD");
        resource.set("FileDescription", "ChatCMD");
        resource.set("FileVersion", &version);
        resource.set("ProductVersion", &version);
        resource.compile().expect("compile Windows resources");
    }
}
