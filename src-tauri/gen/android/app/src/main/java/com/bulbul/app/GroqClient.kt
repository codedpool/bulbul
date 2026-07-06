// Talks to Groq's OpenAI-compatible /audio/transcriptions endpoint.
//
// We use HttpURLConnection rather than OkHttp to avoid pulling another
// Gradle dependency for a single POST — the multipart code below is
// ~80 lines but it's stdlib-only and exactly what we need: stream the
// WAV bytes as a form file, parse the JSON response, return the text.
//
// Model choice: whisper-large-v3-turbo. It's the fastest Whisper
// variant on Groq (~10× quicker than large-v3) and quality is close
// enough for short dictation that the latency win wins. We can swap
// this for a config-driven choice once Settings is wired on mobile.

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
    private const val MODEL = "whisper-large-v3-turbo"
    private const val BOUNDARY = "----BulbulMultipartBoundary"
    private const val CRLF = "\r\n"

    /// Posts the WAV bytes to Groq Whisper and returns the transcript.
    /// Returns null on any failure — caller logs + decides what to do
    /// with the audio (fall back to disk write so the user doesn't
    /// lose their dictation).
    fun transcribe(apiKey: String, wav: ByteArray): String? {
        if (apiKey.isBlank()) {
            Log.w(TAG, "no API key set; not transcribing")
            return null
        }
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
                writeFormField(out, "model", MODEL)
                writeFormField(out, "response_format", "json")
                writeFileField(out, "file", "audio.wav", "audio/wav", wav)
                out.writeBytes("--$BOUNDARY--$CRLF")
            }

            val code = conn.responseCode
            val body = if (code in 200..299) {
                conn.inputStream.bufferedReader().use(BufferedReader::readText)
            } else {
                val err = conn.errorStream?.let { InputStreamReader(it).buffered().readText() } ?: ""
                Log.w(TAG, "Groq returned $code: $err")
                return null
            }
            JSONObject(body).optString("text").trim().takeIf { it.isNotEmpty() }
        } catch (t: Throwable) {
            Log.w(TAG, "Groq transcribe failed", t)
            null
        }
    }

    /// Runs a single chat completion — the transform pipeline. [systemPrompt]
    /// is the transform's instruction (see Transforms.kt), [userText] is the
    /// selected text to transform. Returns the model's output, or null on any
    /// failure (no key, network, non-2xx, empty completion) so the caller can
    /// surface a toast instead of silently replacing the selection with junk.
    fun chat(apiKey: String, systemPrompt: String, userText: String, model: String): String? {
        if (apiKey.isBlank()) {
            Log.w(TAG, "no API key set; not transforming")
            return null
        }
        return try {
            val payload = JSONObject().apply {
                put("model", model)
                put("temperature", 0.3)
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
