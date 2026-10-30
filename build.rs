//! Puts the icon into the exe (the window icon comes from `assets/icon.png` at run time).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon.ico");
        resource.compile().expect("exe icon");
    }
}
