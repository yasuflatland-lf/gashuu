fn main() {
    // Windows-only: embed the manifest (+ icon) for Per-Monitor-V2 DPI awareness — else Windows
    // stretches/blurs on scale changes. Gate on CARGO_CFG_TARGET_OS (build target), not host cfg!.
    #[cfg(windows)]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            // <dpiAware> is the Win8.1 fallback; <dpiAwareness> wins on Win10 1607+.
            // "PerMonitorV2, PerMonitor" falls back to v1 on the few builds (<1703) lacking v2.
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

    // Slint's compiler recurses deeply while lowering the UI, overflowing Windows'
    // 1 MiB main-thread stack (Linux/macOS get 8 MiB). Compile on a 32 MiB-stack thread.
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
