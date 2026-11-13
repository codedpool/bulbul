// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// Runs one on-device speech model with sherpa-onnx. Lives in the ":stt"
// process only (see SttService); nothing in the main process touches the
// native libraries.
//
// Two model families are supported here, the two that are fast enough on a
// phone (measured on a moto g62: ~0.25 s for 5 s of audio):
//   - Moonshine (encoder + merged decoder + tokens)
//   - Transducer / Parakeet (encoder + decoder + joiner + tokens)
// The file names are exactly the ones the model catalog installs
// (src-tauri/crates/bulbul-local-stt/catalog.json).

package com.bulbul.app

import android.util.Log
import com.k2fsa.sherpa.onnx.FeatureConfig
import com.k2fsa.sherpa.onnx.OfflineModelConfig
import com.k2fsa.sherpa.onnx.OfflineMoonshineModelConfig
import com.k2fsa.sherpa.onnx.OfflineRecognizer
import com.k2fsa.sherpa.onnx.OfflineRecognizerConfig
import com.k2fsa.sherpa.onnx.OfflineTransducerModelConfig
import java.io.File
import java.io.RandomAccessFile
import java.nio.ByteBuffer
import java.nio.ByteOrder

object SttEngine {
    private const val TAG = "BulbulSttEngine"

    /// What a model id needs, and how long one decode may run on audio.
    private enum class Kind(val maxChunkSeconds: Int) {
        MOONSHINE(28),
        TRANSDUCER(55),
    }

    private fun kindOf(modelId: String): Kind? = when (modelId) {
        "moonshine-tiny-en" -> Kind.MOONSHINE
        "parakeet-tdt-110m-en" -> Kind.TRANSDUCER
        else -> null
    }

    private var recognizer: OfflineRecognizer? = null
    private var loadedId: String? = null

    @Synchronized
    fun transcribe(wavPath: String, modelId: String, modelDir: String): String {
        val kind = kindOf(modelId) ?: error("This model isn't available on Android.")
        val dir = File(modelDir)
        if (!File(dir, ".complete").exists()) error("The model isn't fully downloaded.")

        val (samples, rate) = readWav(File(wavPath))
        val rec = load(kind, modelId, dir)

        val parts = ArrayList<String>()
        for (chunk in split(samples, rate, kind.maxChunkSeconds)) {
            val stream = rec.createStream()
            try {
                stream.acceptWaveform(chunk, rate)
                rec.decode(stream)
                val text = rec.getResult(stream).text.trim()
                if (text.isNotEmpty()) parts.add(text)
            } finally {
                stream.release()
            }
        }
        return parts.joinToString(" ")
    }

    private fun load(kind: Kind, modelId: String, dir: File): OfflineRecognizer {
        recognizer?.let { if (loadedId == modelId) return it }
        release()

        fun path(name: String): String {
            val f = File(dir, name)
            require(f.isFile) { "$name is missing from the model folder." }
            return f.absolutePath
        }

        val model = OfflineModelConfig(numThreads = threads(), tokens = path("tokens.txt"))
        when (kind) {
            Kind.MOONSHINE -> model.moonshine = OfflineMoonshineModelConfig(
                encoder = path("encoder_model.ort"),
                mergedDecoder = path("decoder_model_merged.ort"),
            )
            Kind.TRANSDUCER -> {
                model.transducer = OfflineTransducerModelConfig(
                    encoder = path("encoder.int8.onnx"),
                    decoder = path("decoder.int8.onnx"),
                    joiner = path("joiner.int8.onnx"),
                )
                model.modelType = "nemo_transducer"
            }
        }
        val started = System.currentTimeMillis()
        val rec = OfflineRecognizer(
            config = OfflineRecognizerConfig(featConfig = FeatureConfig(sampleRate = 16000, featureDim = 80), modelConfig = model),
        )
        Log.i(TAG, "loaded $modelId in ${System.currentTimeMillis() - started} ms")
        recognizer = rec
        loadedId = modelId
        return rec
    }

    @Synchronized
    fun release() {
        recognizer?.release()
        recognizer = null
        loadedId = null
    }

    /// Four threads is where the phone stops getting faster; leave a core
    /// for the rest of the system on smaller chips.
    private fun threads(): Int = Runtime.getRuntime().availableProcessors().coerceIn(1, 4)

    // ---------- audio ----------

    /// Reads the 16-bit PCM WAV that AudioRecorder writes: (samples in
    /// -1..1, sample rate). Mono; extra channels are averaged.
    private fun readWav(file: File): Pair<FloatArray, Int> {
        RandomAccessFile(file, "r").use { f ->
            val bytes = ByteArray(f.length().toInt())
            f.readFully(bytes)
            val bb = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)
            require(bytes.size > 44 && String(bytes, 0, 4) == "RIFF") { "That isn't a WAV file." }
            var channels = 1
            var rate = 16000
            var pos = 12
            var dataStart = -1
            var dataLen = 0
            while (pos + 8 <= bytes.size) {
                val id = String(bytes, pos, 4)
                val len = bb.getInt(pos + 4)
                if (id == "fmt ") {
                    channels = bb.getShort(pos + 10).toInt()
                    rate = bb.getInt(pos + 12)
                    require(bb.getShort(pos + 22).toInt() == 16) { "Only 16-bit audio is supported." }
                } else if (id == "data") {
                    dataStart = pos + 8
                    dataLen = minOf(len.toLong() and 0xFFFFFFFFL, (bytes.size - dataStart).toLong()).toInt()
                    break
                }
                pos += 8 + len + (len and 1)
            }
            require(dataStart >= 0) { "The WAV file has no audio." }
            val frames = dataLen / (2 * channels)
            val out = FloatArray(frames)
            for (i in 0 until frames) {
                var sum = 0
                for (c in 0 until channels) sum += bb.getShort(dataStart + (i * channels + c) * 2).toInt()
                out[i] = sum / (channels * 32768f)
            }
            return out to rate
        }
    }

    /// Cuts audio longer than a model can take in one go at the quietest
    /// moment near the limit, so words aren't split in half.
    private fun split(samples: FloatArray, rate: Int, maxSeconds: Int): List<FloatArray> {
        val max = maxSeconds * rate
        if (samples.size <= max) return listOf(samples)
        val frame = rate / 10
        val search = 4 * rate
        val out = ArrayList<FloatArray>()
        var start = 0
        while (samples.size - start > max) {
            val lo = start + max - search
            var best = start + max
            var bestEnergy = Float.MAX_VALUE
            var p = lo
            while (p + frame <= start + max) {
                var e = 0f
                for (i in p until p + frame) e += samples[i] * samples[i]
                if (e < bestEnergy) {
                    bestEnergy = e
                    best = p + frame / 2
                }
                p += frame
            }
            out.add(samples.copyOfRange(start, best))
            start = best
        }
        if (start < samples.size) out.add(samples.copyOfRange(start, samples.size))
        return out
    }
}
