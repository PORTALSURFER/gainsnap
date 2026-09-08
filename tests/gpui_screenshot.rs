//! Release screenshot coverage for the live embedded GPUI renderer.

#[cfg(target_os = "macos")]
#[test]
fn screenshot_renders_initial_ui() {
    if std::env::var_os("TOYBOX_UI_SCREENSHOT").is_none() {
        return;
    }

    let runner = std::env::var_os("CARGO_BIN_EXE_gainsnap_gpui_screenshot")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let executable = std::env::current_exe().ok()?;
            let target_dir = executable.parent()?.parent()?;
            let direct = target_dir.join("gainsnap_gpui_screenshot");
            direct.is_file().then_some(direct).or_else(|| {
                let example = target_dir.join("examples/gainsnap_gpui_screenshot");
                example.is_file().then_some(example)
            })
        })
        .expect("Cargo should build the GPUI screenshot runner");

    let status = std::process::Command::new(runner)
        .status()
        .expect("GPUI screenshot runner should start");
    assert!(status.success(), "GPUI screenshot runner failed: {status}");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn screenshot_renders_initial_ui() {
    // The release screenshot is a native macOS GPU capture. Windows coverage
    // exercises the same hosted GPUI facade through the plugin-editor probe.
}
