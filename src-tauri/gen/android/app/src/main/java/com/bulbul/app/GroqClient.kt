// Talks to Groq's OpenAI-compatible /audio/transcriptions endpoint.
//
// We use HttpURLConnection rather than OkHttp to avoid pulling another
// Gradle dependency for a single POST — the multipart code below is
// ~80 lines but it's stdlib-only and exactly what we need: stream the
// WAV bytes as a form file, parse the JSON response, return the text.
//
// STT model chain: whisper-large-v3-turbo first (fastest Whisper variant on
// Groq, quality close enough for short dictation that the latency win wins),
// falling back to whisper-large-v3 if turbo fails. Mirrors the desktop STT
// chain (groq.rs STT_FALLBACK) so a decommissioned model can't kill dictation.

package com.bulbul.app

import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.DataOutputStream
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL

object GroqClient {

    private const val TAG = "BulbulGroq"
    private const val ENDPOINT = "https://api.groq.com/openai/v1/audio/transcriptions"
    private const val CHAT_ENDPOINT = "https://api.groq.com/openai/v1/chat/completions"
    /// STT model order, primary-first — mirrors the desktop STT chain
    /// (groq.rs STT_FALLBACK). turbo is fast; large-v3 is the resilience
    /// fallback so a decommissioned turbo doesn't kill dictation.
    private val STT_MODELS = listOf("whisper-large-v3-turbo", "whisper-large-v3")
    private const val BOUNDARY = "----BulbulMultipartBoundary"
    private const val CRLF = "\r\n"

    /// Posts the WAV bytes to Groq Whisper and returns the transcript.
    /// Tries each model in STT_MODELS until one succeeds; returns null only
    /// if every model fails, so the caller can fall back to disk (audio kept
    /// so the user doesn't lose their dictation).
    fun transcribe(apiKey: String, wav: ByteArray): String? {
        if (apiKey.isBlank()) {
            Log.w(TAG, "no API key set; not transcribing")
            return null
        }
        for (model in STT_MODELS) {
            val text = transcribeWith(apiKey, wav, model)
            if (text != null) return text
            Log.w(TAG, "STT model $model failed; trying next in chain")
        }
        Log.w(TAG, "all STT models in the fallback chain failed")
        return null
    }

    private fun transcribeWith(apiKey: String, wav: ByteArray, model: String): String? {
        return try {
            val url = URL(ENDPOINT)
            val conn = (url.openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"
                doOutput = true
                connectTimeout = 10_000
                readTimeout = 30_000
                setRequestProperty("Authorization", "Bearer $apiKey")
                setRequestProperty("Content-Type", "multipart/form-data; boundary=$BOUNDARY")
            }

            DataOutputStream(conn.outputStream).use { out ->
                writeFormField(out, "model", model)
                writeFormField(out, "response_format", "json")
                writeFileField(out, "file", "audio.wav", "audio/wav", wav)
                out.writeBytes("--$BOUNDARY--$CRLF")
            }

            val code = conn.responseCode
            val body = if (code in 200..299) {
                conn.inputStream.bufferedReader().use(BufferedReader::readText)
            } else {
                val err = conn.errorStream?.let { InputStreamReader(it).buffered().readText() } ?: ""
                Log.w(TAG, "Groq ($model) returned $code: $err")
                return null
            }
            JSONObject(body).optString("text").trim().takeIf { it.isNotEmpty() }
        } catch (t: Throwable) {
            Log.w(TAG, "Groq transcribe ($model) failed", t)
            null
        }
    }

    /// Runs a single chat completion — the transform pipeline. [systemPrompt]
    /// is the transform's instruction (see Transforms.kt), [userText] is the
    /// selected text to transform. Returns the model's output, or null on any
    /// failure (no key, network, non-2xx, empty completion) so the caller can
    /// surface a toast instead of silently replacing the selection with junk.
    fun chat(apiKey: String, systemPrompt: String, userText: String, model: String, temperature: Double = 0.3): String? {
        if (apiKey.isBlank()) {
            Log.w(TAG, "no API key set; not transforming")
            return null
        }
        return try {
            val payload = JSONObject().apply {
                put("model", model)
                put("temperature", temperature)
                reasoningEffortFor(model)?.let { put("reasoning_effort", it) }
                put("messages", JSONArray().apply {
                    put(JSONObject().put("role", "system").put("content", systemPrompt))
                    put(JSONObject().put("role", "user").put("content", userText))
                })
            }.toString()

            val conn = (URL(CHAT_ENDPOINT).openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"
                doOutput = true
                connectTimeout = 10_000
                readTimeout = 30_000
                setRequestProperty("Authorization", "Bearer $apiKey")
                setRequestProperty("Content-Type", "application/json")
            }
            conn.outputStream.use { it.write(payload.toByteArray(Charsets.UTF_8)) }

            val code = conn.responseCode
            if (code !in 200..299) {
                val err = conn.errorStream?.let { InputStreamReader(it).buffered().readText() } ?: ""
                Log.w(TAG, "Groq chat returned $code: $err")
                return null
            }
            val body = conn.inputStream.bufferedReader().use(BufferedReader::readText)
            JSONObject(body)
                .getJSONArray("choices").getJSONObject(0)
                .getJSONObject("message").getString("content")
                .trim().takeIf { it.isNotEmpty() }
        } catch (t: Throwable) {
            Log.w(TAG, "Groq chat failed", t)
            null
        }
    }

    /// Transform-model order, primary-first — mirrors Cleanup.CLEANUP_FALLBACK
    /// and groq.rs CLEANUP_FALLBACK. Keep in sync.
    private val CHAT_FALLBACK = listOf("openai/gpt-oss-20b", "openai/gpt-oss-120b")

    /// Chat completion with model fallthrough — the transform pipeline's
    /// resilience layer, mirroring desktop execute_transform. Tries
    /// [primaryModel, gpt-oss-20b, gpt-oss-120b] (deduped) and returns the first
    /// non-null; null only if EVERY model fails (rate-limit / dead model /
    /// network), so a rate-limited cleanup model no longer breaks a transform.
    fun chatWithFallback(apiKey: String, systemPrompt: String, userText: String, primaryModel: String): String? {
        val chain = ArrayList<String>()
        val p = primaryModel.trim()
        if (p.isNotEmpty()) chain.add(p)
        for (m in CHAT_FALLBACK) if (!chain.contains(m)) chain.add(m)
        for (model in chain) {
            val out = chat(apiKey, systemPrompt, userText, model)
            if (out != null) return out
            Log.w(TAG, "transform model $model failed; trying next in chain")
        }
        Log.w(TAG, "transform: all models in the fallback chain failed")
        return null
    }

    /// Groq's reasoning models add latency + burn tokens unless told to stop.
    /// Mirrors desktop (groq.rs reasoning_effort_for): qwen fully disables
    /// reasoning ("none"); gpt-oss accepts only low/medium/high (use "low");
    /// anything else omits the field.
    private fun reasoningEffortFor(model: String): String? = when {
        model.startsWith("qwen/") -> "none"
        model.startsWith("openai/gpt-oss") -> "low"
        else -> null
    }

    private fun writeFormField(out: DataOutputStream, name: String, value: String) {
        out.writeBytes("--$BOUNDARY$CRLF")
        out.writeBytes("Content-Disposition: form-data; name=\"$name\"$CRLF$CRLF")
        out.write(value.toByteArray(Charsets.UTF_8))
        out.writeBytes(CRLF)
    }

    private fun writeFileField(
        out: DataOutputStream,
        name: String,
        filename: String,
        contentType: String,
        bytes: ByteArray,
    ) {
        out.writeBytes("--$BOUNDARY$CRLF")
        out.writeBytes("Content-Disposition: form-data; name=\"$name\"; filename=\"$filename\"$CRLF")
        out.writeBytes("Content-Type: $contentType$CRLF$CRLF")
        out.write(bytes)
        out.writeBytes(CRLF)
    }
}
