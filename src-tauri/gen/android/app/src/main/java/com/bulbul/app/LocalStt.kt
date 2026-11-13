// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// The main process's side of on-device transcription: starts the speech
// service (its own process, see SttService), hands it a recording, and returns
// the text. If that process dies mid-call (a crash, a hang the watchdog
// ended, Android reclaiming memory) it is started again and the recording is
// retried once, the same recovery the desktop worker gets.
//
// Models are downloaded by the Rust side (mobile.rs, the shared model
// library) into <data dir>/local-stt/models/<id>; this only reads them.

package com.bulbul.app

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.DeadObjectException
import android.os.IBinder
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

object LocalStt {
    private const val TAG = "BulbulLocalStt"

    @Volatile private var service: ISttService? = null
    private var connection: ServiceConnection? = null
    private val lock = Any()

    /// Folder the Rust model library installs [modelId] into.
    fun modelDir(context: Context, modelId: String): File =
        File(File(BulbulConfig.dataDir(context), "local-stt/models"), modelId)

    fun isInstalled(context: Context, modelId: String): Boolean =
        File(modelDir(context, modelId), ".complete").exists()

    /// Transcribes [wav] with [modelId]; null when it can't (not downloaded,
    /// the speech process failed twice, ...). Blocking: call off the main thread.
    fun transcribe(context: Context, wav: ByteArray, modelId: String, language: String, idleMinutes: Int): String? {
        if (modelId.isBlank() || !isInstalled(context, modelId)) {
            Log.w(TAG, "model '$modelId' isn't downloaded")
            return null
        }
        val file = File(context.cacheDir, "stt-${System.nanoTime()}.wav")
        return try {
            file.writeBytes(wav)
            for (attempt in 1..2) {
                val result = callOnce(context, file, modelId, language, idleMinutes)
                when {
                    result is Outcome.Text -> return result.text
                    result is Outcome.Failed -> {
                        Log.w(TAG, "model reported: ${result.message}")
                        return null
                    }
                    // The process went away: try once more with a new one.
                    else -> Log.w(TAG, "the speech process died (attempt $attempt)")
                }
            }
            null
        } catch (t: Throwable) {
            Log.w(TAG, "on-device transcription failed", t)
            null
        } finally {
            file.delete()
        }
    }

    private sealed class Outcome {
        data class Text(val text: String) : Outcome()
        data class Failed(val message: String) : Outcome()
        object Died : Outcome()
    }

    private fun callOnce(context: Context, wav: File, modelId: String, language: String, idleMinutes: Int): Outcome {
        val svc = connect(context) ?: return Outcome.Died
        return try {
            val json = JSONObject(
                svc.transcribe(wav.absolutePath, modelId, modelDir(context, modelId).absolutePath, language, idleMinutes),
            )
            if (json.has("error")) Outcome.Failed(json.getString("error")) else Outcome.Text(json.optString("text").trim())
        } catch (e: DeadObjectException) {
            disconnect(context)
            Outcome.Died
        } catch (e: android.os.RemoteException) {
            disconnect(context)
            Outcome.Died
        }
    }

    private fun connect(context: Context): ISttService? {
        synchronized(lock) {
            service?.let { if (it.asBinder().isBinderAlive) return it }
            disconnect(context)
            val latch = CountDownLatch(1)
            val conn = object : ServiceConnection {
                override fun onServiceConnected(name: ComponentName, binder: IBinder) {
                    service = ISttService.Stub.asInterface(binder)
                    latch.countDown()
                }

                override fun onServiceDisconnected(name: ComponentName) {
                    service = null
                }
            }
            val app = context.applicationContext
            if (!app.bindService(Intent(app, SttService::class.java), conn, Context.BIND_AUTO_CREATE)) {
                Log.w(TAG, "couldn't bind the speech service")
                return null
            }
            connection = conn
            if (!latch.await(15, TimeUnit.SECONDS)) {
                Log.w(TAG, "the speech service didn't start in time")
                disconnect(context)
                return null
            }
            return service
        }
    }

    private fun disconnect(context: Context) {
        synchronized(lock) {
            service = null
            connection?.let {
                try {
                    context.applicationContext.unbindService(it)
                } catch (_: IllegalArgumentException) {
                    // already unbound
                }
            }
            connection = null
        }
    }
}
