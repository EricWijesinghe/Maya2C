// Every app command is named here, so the capability in capabilities/default.json
// grants each one by its allow-* permission and the window cannot reach any other.
// The names must match the functions registered in src/lib.rs.
const COMMANDS: &[&str] = &[
    "status",
    "create_identity",
    "set_relay",
    "publish",
    "add_contact",
    "history",
    "send",
    "poll",
    "deliveries",
    "dismiss_delivery",
    "sticker_packs",
    "sticker_file",
    "import_sticker",
    "create_pack",
    "remove_pack",
    "send_sticker",
    "request_pack",
];

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run tauri-build");
}
