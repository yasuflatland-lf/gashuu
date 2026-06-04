fn main() {
    // Windows: embed the application icon into gashuu.exe so it shows in the
    // taskbar / Explorer. Two guards: the `#[cfg(windows)]` block keeps the
    // winresource crate (a Windows-only build-dependency) out of macOS/Linux
    // compiles, and the runtime `CARGO_CFG_TARGET_OS` check reflects the BUILD
    // TARGET (build.rs runs on the HOST, so `cfg!(target_os)` would lie under a
    // cross-compile). The `.ico` is generated from app-icon.png by the release
    // workflow on the Windows runner; a plain dev `cargo build` without it is a
    // no-op, so the icon is best-effort and never a build blocker.
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
            && std::path::Path::new("ui/assets/app-icon.ico").exists()
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("ui/assets/app-icon.ico");
            res.compile()
                .expect("failed to embed the Windows application icon");
        }
    }

    // Slint's compiler recurses deeply while lowering the UI. On Windows the
    // default main-thread stack is only 1 MiB (Linux/macOS get 8 MiB), and once
    // the UI grew past a certain complexity (the PR8a thumbnail strip's nested
    // Flickable + Repeater pushed it over the edge) the Windows build script
    // overflowed its stack (STATUS_STACK_OVERFLOW). Run the compile on a thread
    // with a generous stack so the build stays robust on every platform and as
    // the UI keeps growing.
    let build = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            slint_build::compile_with_config(
                "ui/ViewerWindow.slint",
                slint_build::CompilerConfiguration::new().with_style("fluent-dark".into()),
            )
            .expect("Slint build failed");
        })
        .expect("failed to spawn Slint build thread");
    build.join().expect("Slint build thread panicked");
}
