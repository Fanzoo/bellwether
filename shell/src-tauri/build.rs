fn main() {
    // Only bellwether's own commands exist, and each gets a generated permission
    // (allow-report-hitboxes, …) that a capability grants explicitly: the strip's six in
    // capabilities/strip.json, the onboarding window's eleven in capabilities/onboarding.json.
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "report_hitboxes",
            "sprite_clicked",
            "get_pet",
            "get_snapshot",
            "shell_log",
            "debug_snapshot",
            "setup_status",
            "hooks_preview",
            "hooks_apply",
            "login_items_set",
            "permission_request",
            "pet_select",
            "pet_import",
            "migration_preview",
            "migration_apply",
            "cli_tool_install",
            "setup_close",
        ])),
    )
    .expect("failed to run tauri-build");
}
