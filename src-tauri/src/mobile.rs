// Bulbul on Android (and eventually iOS).
//
// Phase 1: just get a Tauri app that boots, opens the WebView, and
// loads dist/index.html. The existing React UI mostly carries over
// visually; commands the UI calls (get_config, list_dictation_history,
// etc.) will return errors at first because they're desktop-only —
// that's expected. We stub them out one-by-one as we port settings,
// history, and the floating-bubble overlay (Phase 2-5).
//
// What's deliberately NOT here yet:
//   - cpal-based audio capture (replaced by Kotlin AudioRecord + JNI)
//   - tauri-plugin-global-shortcut (no global hotkeys on Android)
//   - tauri-plugin-single-instance (Android process model handles this)
//   - tray icon / overlay window / hover-watcher (different on mobile)
//   - hotkey orchestrator, dictation pipeline (pulled in once audio
//     bridge is in place)

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .run(tauri::generate_context!())
        .expect("error while running Bulbul on mobile");
}
