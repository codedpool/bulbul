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
                MediaRecorder.AudioSource.MIC,
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

        val pcmBytes = synchronized(pcm) { pcm.toByteArray() }
        if (pcmBytes.isEmpty()) {
            Log.w(TAG, "recording produced no audio")
            return null
        }
        Log.i(TAG, "recording stopped, captured ${pcmBytes.size} PCM bytes")
        return wrapAsWav(pcmBytes)
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
