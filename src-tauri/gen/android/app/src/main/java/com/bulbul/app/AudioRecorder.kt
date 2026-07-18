// Captures mic audio for the dictation pipeline.
//
// AudioRecord (not MediaRecorder) because we want raw PCM frames we
// can stream to Groq's Whisper endpoint — MediaRecorder would compress
// to AAC/3gp and require a server-side decode hop.
//
// Settings match what Whisper expects on the input side:
//   - 16 kHz sample rate (matches Whisper's native model rate)
//   - mono channel
//   - PCM 16-bit signed little-endian
//
// On stop() we wrap the accumulated frames in a minimal WAV header so
// the result is a self-describing file the Groq endpoint accepts via
// multipart/form-data without any client-side re-encoding.
//
// The recording thread copies straight from AudioRecord into an
// in-memory ByteArrayOutputStream — fine for the short clips we expect
// (typical bubble dictation is 1–10 s). If we ever want to dictate a
// monologue, this needs to move to a chunked uploader so memory
// doesn't blow up.

package com.bulbul.app

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.audiofx.AutomaticGainControl
import android.media.audiofx.NoiseSuppressor
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.concurrent.thread

class AudioRecorder(private val context: Context) {

    @Volatile private var recording = false
    private var thread: Thread? = null
    private var record: AudioRecord? = null
    private var noiseSuppressor: NoiseSuppressor? = null
    private var agc: AutomaticGainControl? = null
    private var pcm = ByteArrayOutputStream()

    fun isRecording(): Boolean = recording

    /// Starts capturing into an internal buffer. Returns false if
    /// RECORD_AUDIO is not granted or the AudioRecord init failed —
    /// caller is responsible for surfacing that to the user (the
    /// runtime permission request lives one layer up, in the
    /// foreground service).
    fun start(): Boolean {
        if (recording) return true
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED) {
            Log.w(TAG, "RECORD_AUDIO not granted, refusing to start")
            return false
        }

        val minBuf = AudioRecord.getMinBufferSize(
            SAMPLE_RATE_HZ,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minBuf <= 0) {
            Log.w(TAG, "getMinBufferSize returned $minBuf — device cannot support requested config")
            return false
        }
        val bufSize = maxOf(minBuf, SAMPLE_RATE_HZ * 2) // ~1s buffer headroom

        val r = try {
            @Suppress("MissingPermission") // checked above
            AudioRecord(
                // VOICE_RECOGNITION requests the ASR-tuned capture path,
                // which the OS keeps free of the aggressive "communication"
                // voice processing that can smear speech for a transcription
                // model. Better default than MIC for dictation.
                MediaRecorder.AudioSource.VOICE_RECOGNITION,
                SAMPLE_RATE_HZ,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                bufSize,
            )
        } catch (t: Throwable) {
            Log.w(TAG, "AudioRecord ctor threw", t)
            return false
        }
        if (r.state != AudioRecord.STATE_INITIALIZED) {
            Log.w(TAG, "AudioRecord did not initialize (state=${r.state})")
            r.release()
            return false
        }

        record = r
        attachEffects(r.audioSessionId)
        pcm = ByteArrayOutputStream()
        recording = true
        r.startRecording()

        thread = thread(name = "BulbulAudioReader", isDaemon = true) {
            val buf = ByteArray(READ_CHUNK_BYTES)
            while (recording) {
                val n = try {
                    r.read(buf, 0, buf.size)
                } catch (t: Throwable) {
                    Log.w(TAG, "AudioRecord.read threw", t)
                    break
                }
                if (n > 0) {
                    synchronized(pcm) { pcm.write(buf, 0, n) }
                } else if (n < 0) {
                    Log.w(TAG, "AudioRecord.read returned error $n")
                    break
                }
            }
        }
        Log.i(TAG, "recording started")
        return true
    }

    /// Stops the recorder and returns the captured audio wrapped in a
    /// 44-byte WAV header (PCM, 16 kHz mono, 16-bit). Returns null if
    /// nothing was captured.
    fun stop(): ByteArray? {
        if (!recording && record == null) return null
        recording = false
        try {
            thread?.join(500)
        } catch (_: InterruptedException) {}
        thread = null

        try {
            record?.stop()
        } catch (t: Throwable) {
            Log.w(TAG, "AudioRecord.stop threw", t)
        }
        record?.release()
        record = null
        releaseEffects()

        val pcmBytes = synchronized(pcm) { pcm.toByteArray() }
        if (pcmBytes.isEmpty()) {
            Log.w(TAG, "recording produced no audio")
            return null
        }
        Log.i(TAG, "recording stopped, captured ${pcmBytes.size} PCM bytes")
        return wrapAsWav(normalize(pcmBytes))
    }

    /// Ask the platform to clean the capture the way Windows' mic
    /// enhancement does for the desktop build: framework noise suppression
    /// and auto-gain, when the chipset offers them (availability varies;
    /// absent effects simply no-op). This is the biggest single lever for
    /// matching desktop transcription quality on-device.
    private fun attachEffects(sessionId: Int) {
        try {
            if (NoiseSuppressor.isAvailable()) {
                noiseSuppressor = NoiseSuppressor.create(sessionId)?.also { it.setEnabled(true) }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "NoiseSuppressor attach failed", t)
        }
        try {
            if (AutomaticGainControl.isAvailable()) {
                agc = AutomaticGainControl.create(sessionId)?.also { it.setEnabled(true) }
            }
        } catch (t: Throwable) {
            Log.w(TAG, "AutomaticGainControl attach failed", t)
        }
    }

    private fun releaseEffects() {
        try { noiseSuppressor?.release() } catch (_: Throwable) {}
        try { agc?.release() } catch (_: Throwable) {}
        noiseSuppressor = null
        agc = null
    }

    /// Peak-normalize the 16-bit little-endian PCM toward -3 dBFS so soft
    /// speech reaches a level Whisper transcribes reliably — a software
    /// backstop for phones whose AutomaticGainControl effect is
    /// unavailable. Mirrors the desktop AGC's floor: only ever boosts
    /// (gain >= 1), capped at 20x so near-silence can't turn to hiss.
    /// Mutates and returns the same array.
    private fun normalize(pcm: ByteArray): ByteArray {
        val n = pcm.size / 2
        if (n == 0) return pcm
        val buf = ByteBuffer.wrap(pcm).order(ByteOrder.LITTLE_ENDIAN)
        var peak = 0
        for (i in 0 until n) {
            val s = buf.getShort(i * 2).toInt()
            val a = if (s < 0) -s else s
            if (a > peak) peak = a
        }
        if (peak == 0) return pcm
        val targetAmp = 0.708f * Short.MAX_VALUE.toInt() // -3 dBFS
        var gain = targetAmp / peak
        if (gain < 1f) gain = 1f
        if (gain > 20f) gain = 20f
        if (gain <= 1.01f) return pcm
        for (i in 0 until n) {
            val v = buf.getShort(i * 2) * gain
            val clamped = when {
                v > Short.MAX_VALUE.toFloat() -> Short.MAX_VALUE
                v < Short.MIN_VALUE.toFloat() -> Short.MIN_VALUE
                else -> v.toInt().toShort()
            }
            buf.putShort(i * 2, clamped)
        }
        Log.i(TAG, "normalized: peak=$peak gain=$gain")
        return pcm
    }

    /// Builds a canonical PCM WAV file in memory: 44-byte RIFF header
    /// followed by the raw little-endian samples we already captured.
    private fun wrapAsWav(pcm: ByteArray): ByteArray {
        val totalDataLen = pcm.size + 36
        val byteRate = SAMPLE_RATE_HZ * NUM_CHANNELS * BITS_PER_SAMPLE / 8
        val header = ByteBuffer.allocate(44).order(ByteOrder.LITTLE_ENDIAN)

        header.put("RIFF".toByteArray(Charsets.US_ASCII))
        header.putInt(totalDataLen)
        header.put("WAVE".toByteArray(Charsets.US_ASCII))
        header.put("fmt ".toByteArray(Charsets.US_ASCII))
        header.putInt(16)                                // PCM fmt chunk size
        header.putShort(1)                               // PCM format
        header.putShort(NUM_CHANNELS.toShort())
        header.putInt(SAMPLE_RATE_HZ)
        header.putInt(byteRate)
        header.putShort((NUM_CHANNELS * BITS_PER_SAMPLE / 8).toShort()) // block align
        header.putShort(BITS_PER_SAMPLE.toShort())
        header.put("data".toByteArray(Charsets.US_ASCII))
        header.putInt(pcm.size)

        val out = ByteArray(44 + pcm.size)
        System.arraycopy(header.array(), 0, out, 0, 44)
        System.arraycopy(pcm, 0, out, 44, pcm.size)
        return out
    }

    companion object {
        private const val TAG = "BulbulAudio"
        const val SAMPLE_RATE_HZ = 16_000
        private const val NUM_CHANNELS = 1
        private const val BITS_PER_SAMPLE = 16
        private const val READ_CHUNK_BYTES = 4096
    }
}
