package com.bulbul.app;

// The on-device speech service, which runs in its own process (":stt") so a
// native crash or an out-of-memory kill there can't take the app down.
interface ISttService {
    // Transcribes the 16-bit mono WAV file at [wavPath] with the model in
    // [modelDir]. Returns JSON: {"text": "..."} or {"error": "..."}.
    String transcribe(String wavPath, String modelId, String modelDir, String language, int idleMinutes);
}
