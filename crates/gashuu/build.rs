fn main() {
    // Windows: embed an application manifest (and the app icon) into gashuu.exe.
    // Two guards: the `#[cfg(windows)]` block keeps the winresource crate (a
    // Windows-only build-dependency) out of macOS/Linux compiles, and the runtime
    // `CARGO_CFG_TARGET_OS` check reflects the BUILD TARGET (build.rs runs on the
    // HOST, so `cfg!(target_os)` would lie under a cross-compile).
    //
    // The manifest declares Per-Monitor-V2 DPI awareness. Without it the process
    // is DPI-unaware, so Windows bitmap-stretches the window whenever it moves to
    // a monitor with a different scale factor — the window's apparent size jumps
    // and the page blurs. A manifest is the authoritative, Microsoft-recommended
    // way to set this: the OS applies it at process load, before any window is
    // created, unlike winit's runtime `SetProcessDpiAwarenessContext` call, which
    // can silently no-op if awareness is locked before the event loop starts.
    //
    // The icon is generated from app-icon.png by the release workflow on the
    // Windows runner; a plain dev `cargo build` without it still gets the manifest.
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            // <dpiAware> is the Win8.1 fallback; <dpiAwareness> takes precedence on
            // Windows 10 1607+. "PerMonitorV2, PerMonitor" falls back to v1 on the
            // few builds (10 < 1703) that lack v2.
            const DPI_AWARE_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;
            let mut res = winresource::WindowsResource::new();
            res.set_manifest(DPI_AWARE_MANIFEST);
            if std::path::Path::new("ui/assets/app-icon.ico").exists() {
                res.set_icon("ui/assets/app-icon.ico");
            }
            res.compile()
                .expect("failed to embed the Windows resources (DPI manifest + icon)");
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
