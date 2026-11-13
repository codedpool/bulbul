# sherpa-onnx's native code reaches into these classes by name (JNI), so
# release builds must not rename or strip them.
-keep class com.k2fsa.sherpa.onnx.** { *; }
