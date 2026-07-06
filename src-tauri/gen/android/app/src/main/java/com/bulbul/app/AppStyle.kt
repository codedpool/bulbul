// Per-app dictation style — the Android side of the desktop "Style"
// feature. The desktop biases its cleanup LLM with a tone hint chosen by
// which app you're dictating into (WhatsApp → casual, Outlook → formal,
// …). Mobile has no always-on cleanup pass, and an LLM restyle proved
// both slow and prone to *answering* the dictation instead of reformatting
// it. Since the three styles differ only in capitalization/punctuation
// (see the Style page's samples), we do the reformat with plain string
// transforms — instant, offline, and impossible to "reply".
//
// Desktop keys categories on exe names / macOS bundle ids; Android hands
// us Java package names instead, so the mapping table below is Android
// packages. Anything unknown falls through to "other".

package com.bulbul.app

object AppStyle {

    /// Android package → style category. Mirrors the buckets the desktop
    /// StyleView shows (personal / work / email / other).
    private val CATEGORY: Map<String, String> = mapOf(
        // Personal messengers
        "com.whatsapp" to "personal",
        "com.whatsapp.w4b" to "personal",
        "org.telegram.messenger" to "personal",
        "org.telegram.messenger.web" to "personal",
        "org.thoughtcrime.securesms" to "personal", // Signal
        "com.facebook.orca" to "personal", // Messenger
        "com.instagram.android" to "personal",
        "com.snapchat.android" to "personal",
        "com.google.android.apps.messaging" to "personal", // Google Messages
        "com.android.mms" to "personal",
        // Work chat
        "com.Slack" to "work",
        "com.microsoft.teams" to "work",
        "com.discord" to "work",
        "com.google.android.apps.dynamite" to "work", // Google Chat
        // Email
        "com.google.android.gm" to "email", // Gmail
        "com.microsoft.office.outlook" to "email",
        "com.yahoo.mobile.client.android.mail" to "email",
        "ch.protonmail.android" to "email",
        "me.proton.android.mail" to "email",
        "com.fsck.k9" to "email", // K-9 Mail
    )

    /// Resolve an app to a style category. User overrides (from the Style
    /// page's "Custom apps") win over the built-in table; each override key
    /// is matched loosely against the package id AND the app's display name
    /// so typing "WhatsApp" or "com.whatsapp" both work. Unknown → "other".
    fun categoryForApp(
        pkg: String?,
        friendly: String?,
        overrides: List<Pair<String, String>>,
    ): String {
        val p = pkg?.lowercase().orEmpty()
        val f = friendly?.lowercase().orEmpty()
        for ((rawKey, cat) in overrides) {
            val k = rawKey.lowercase().removeSuffix(".exe").trim()
            if (k.isNotEmpty() && (p.contains(k) || f == k || f.contains(k))) return cat
        }
        CATEGORY[pkg]?.let { return it }
        return "other"
    }

    /// Reformats [text] to the tone of [styleId] deterministically — pure
    /// string transforms, NO LLM. This matches exactly what the three styles
    /// mean (they differ only in capitalization/punctuation, per the Style
    /// page's own samples), and crucially it can never be slow or "answer"
    /// the dictation the way a chat model sometimes did.
    ///
    ///   formal      → proper: leave punctuation, ensure a capital first letter
    ///   casual      → keep caps, drop commas and the trailing full stop
    ///   very_casual → lowercase, drop commas and the trailing full stop
    fun applyStyle(styleId: String, text: String): String {
        val t = text.trim()
        if (t.isEmpty()) return text
        return when (styleId) {
            "formal" -> capitalizeFirst(t)
            "casual" -> capitalizeFirst(dropTrailingPeriod(t.replace(",", "")))
            "very_casual" -> dropTrailingPeriod(t.replace(",", "").lowercase())
            else -> text
        }
    }

    private fun capitalizeFirst(s: String): String =
        if (s.isEmpty()) s else s[0].uppercaseChar() + s.substring(1)

    /// Drops a single sentence-final full stop (the "texty" signal) but leaves
    /// "?"/"!" and ellipses alone, and keeps periods between sentences.
    private fun dropTrailingPeriod(s: String): String {
        val trimmed = s.trimEnd()
        return if (trimmed.endsWith(".") && !trimmed.endsWith("..")) {
            trimmed.dropLast(1)
        } else {
            trimmed
        }
    }
}
