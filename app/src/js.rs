//! Bindings to `js/bridge.js`.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/bridge.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadSession)]
    pub fn load_session() -> String;
    #[wasm_bindgen(js_name = saveSession)]
    pub fn save_session(json: &str);
    #[wasm_bindgen(js_name = clearSession)]
    pub fn clear_session();

    pub fn connect(server: &str, token: &str, user_id: f64, callback: &JsValue);
    pub fn disconnect();
    #[wasm_bindgen(js_name = logoutNative)]
    pub fn logout_native();

    #[wasm_bindgen(js_name = sendTyping)]
    pub fn send_typing(conversation: f64);

    #[wasm_bindgen(js_name = scrollBottom)]
    pub fn scroll_bottom(id: &str);
    #[wasm_bindgen(js_name = pushScreen)]
    pub fn push_screen();
    #[wasm_bindgen(js_name = popScreen)]
    pub fn pop_screen();
    #[wasm_bindgen(js_name = onBack)]
    pub fn on_back(callback: &JsValue);
    #[wasm_bindgen(js_name = pickFile)]
    pub fn pick_file(input_id: &str);

    #[wasm_bindgen(catch, js_name = uploadFromInput)]
    pub async fn upload_from_input(input_id: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = startRecording)]
    pub async fn start_recording() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = stopRecording)]
    pub async fn stop_recording() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = cancelRecording)]
    pub fn cancel_recording();

    #[wasm_bindgen(js_name = onCallState)]
    pub fn on_call_state(callback: &JsValue);
    #[wasm_bindgen(js_name = startCall)]
    pub fn start_call(conversation: f64, peer: &str, video: bool);
    #[wasm_bindgen(js_name = acceptCall)]
    pub fn accept_call();
    #[wasm_bindgen(js_name = rejectCall)]
    pub fn reject_call();
    pub fn hangup();
    #[wasm_bindgen(js_name = toggleMute)]
    pub fn toggle_mute();
    #[wasm_bindgen(js_name = toggleCamera)]
    pub fn toggle_camera();
}
