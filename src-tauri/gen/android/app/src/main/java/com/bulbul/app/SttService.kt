// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// The on-device speech service. Declared in the manifest with
// android:process=":stt", so it runs in its own process: a native crash in the
// speech library, or Android killing it for memory, can't take the rest of
// Bulbul down, and ending the process frees all of the model's memory.
//
// Two safety nets live here, mirroring the desktop worker's supervisor:
//   - a watchdog that ends the process if a transcription hangs;
//   - an idle timer that ends it after a quiet spell (the user's "free memory"
//     setting), so a loaded model doesn't sit in RAM all day.
// The caller (LocalStt) notices the process going away and starts a fresh one.

package com.bulbul.app

import android.app.Service
import android.content.Intent
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Process
import android.util.Log
import org.json.JSONObject
import java.io.File

class SttService : Service() {
    private val handler = Handler(Looper.getMainLooper())
    private val idleKill = Runnable { endProcess("idle") }

    private val binder = object : ISttService.Stub() {
        override fun transcribe(
            wavPath: String,
            modelId: String,
            modelDir: String,
            language: String?,
            idleMinutes: Int,
        ): String {
            handler.removeCallbacks(idleKill)
            // A hung decode can't be cancelled from inside, so end the whole
            // process; the caller starts another and retries once.
            val watchdog = Runnable { endProcess("a transcription hung") }
            handler.postDelayed(watchdog, hangTimeoutMs(File(wavPath)))
            return try {
                val text = SttEngine.transcribe(wavPath, modelId, modelDir, language)
                JSONObject().put("text", text).toString()
            } catch (t: Throwable) {
                Log.w(TAG, "transcription failed", t)
                JSONObject().put("error", t.message ?: t.javaClass.simpleName).toString()
            } finally {
                handler.removeCallbacks(watchdog)
                if (idleMinutes > 0) handler.postDelayed(idleKill, idleMinutes * 60_000L)
            }
        }
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        SttEngine.release()
        super.onDestroy()
    }

    private fun endProcess(why: String) {
        Log.i(TAG, "ending the speech process: $why")
        Process.killProcess(Process.myPid())
    }

    /// 60 s plus 4 s for every second of audio (a 16 kHz 16-bit mono file is
    /// 32 000 bytes a second), the same allowance as the desktop worker.
    private fun hangTimeoutMs(wav: File): Long {
        val seconds = (wav.length() / 32_000L).coerceAtLeast(1L)
        return 60_000L + 4_000L * seconds
    }

    private companion object {
        const val TAG = "BulbulSttService"
    }
}
