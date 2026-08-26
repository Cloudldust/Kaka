fn main() {
    // Embed the KAKA.ico as the application icon on Windows.
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("resources/KAKA.ico");
        res.set("FileDescription", "Kaka - Camera Photo Import & Cull");
        res.set("ProductName", "Kaka");
        if let Err(err) = res.compile() {
            eprintln!("[kaka] failed to embed icon: {err}");
            // Non-fatal; the app still builds and runs without an icon.
        }
    }
    println!("cargo:rerun-if-changed=resources/KAKA.ico");
}
