fn main() {
    // Only pasture's own commands exist, and each gets a generated permission
    // (allow-report-hitboxes, …) that capabilities/strip.json grants explicitly.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "report_hitboxes",
            "sprite_clicked",
            "get_pet",
            "get_snapshot",
            "shell_log",
            "debug_snapshot",
        ])),
    )
    .expect("failed to run tauri-build");
}
