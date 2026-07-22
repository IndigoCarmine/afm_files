fn main() {
    // Embed the icon into the .exe so Explorer and the taskbar show it even
    // before the app creates a window.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().unwrap();
    }

    println!("cargo:rerun-if-changed=assets/icon.ico");
}
