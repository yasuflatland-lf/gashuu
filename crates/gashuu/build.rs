fn main() {
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
