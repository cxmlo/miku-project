package dev.miku.link

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

object MikuHistoryStore {
    private const val PREFS_NAME = "miku_link_history"
    private const val KEY_FEED_ITEMS = "feed_items_json"
    private const val MAX_FEED_ITEMS = 200

    fun serializeFeedHistory(items: List<AgentFeedItem>): String {
        val arr = JSONArray()
        for (item in items.take(MAX_FEED_ITEMS)) {
            val obj = JSONObject()
            obj.put("agent", item.agent)
            obj.put("kind", item.kind)
            obj.put("title", item.title)
            obj.put("received_at_ms", item.receivedAtMs)
            arr.put(obj)
        }
        val serialized = arr.toString()
        MikuMobileBridge.assertNoProviderApiKeys(serialized)
        return serialized
    }

    fun deserializeFeedHistory(rawJson: String): List<AgentFeedItem> {
        if (rawJson.isBlank()) return emptyList()
        MikuMobileBridge.assertNoProviderApiKeys(rawJson)
        return runCatching {
            val arr = JSONArray(rawJson)
            val result = ArrayList<AgentFeedItem>(arr.length())
            for (i in 0 until arr.length()) {
                val obj = arr.optJSONObject(i) ?: continue
                val agent = obj.optString("agent", "miku")
                val kind = obj.optString("kind", "message")
                val title = obj.optString("title", "")
                val receivedAtMs = obj.optLong("received_at_ms", 0L)
                result.add(
                    AgentFeedItem(
                        agent = agent,
                        kind = kind,
                        title = title,
                        receivedAtMs = receivedAtMs,
                    )
                )
            }
            result
        }.getOrDefault(emptyList())
    }

    fun saveFeedHistory(context: Context, items: List<AgentFeedItem>) {
        val json = serializeFeedHistory(items)
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY_FEED_ITEMS, json)
            .apply()
    }

    fun loadFeedHistory(context: Context): List<AgentFeedItem> {
        val raw = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .getString(KEY_FEED_ITEMS, null)
            ?: return emptyList()
        return deserializeFeedHistory(raw)
    }

    fun clearFeedHistory(context: Context) {
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .remove(KEY_FEED_ITEMS)
            .apply()
    }
}

data class VoiceOptInPrefs(
    val enabled: Boolean = false,
    val engine: String = "off",
    val autoSend: Boolean = false,
)

object MikuVoiceOptInStore {
    const val KEY_VOICE_STT_ENABLED = "voice_stt_enabled"
    const val KEY_VOICE_STT_ENGINE = "voice_stt_engine"
    const val KEY_VOICE_AUTO_SEND = "voice_auto_send"

    fun canLaunchSpeechCapture(enabled: Boolean, engine: String): Boolean {
        val clean = engine.trim().lowercase()
        return enabled && clean.isNotEmpty() && clean != "off"
    }

    fun normalizeEngine(enabled: Boolean, engine: String?): String {
        if (!enabled) return "off"
        return when (engine?.trim()?.lowercase()) {
            "android_recognizer", "pc_remote" -> engine.trim().lowercase()
            else -> "android_recognizer"
        }
    }

    fun load(context: Context): VoiceOptInPrefs {
        val prefs = context.getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE)
        val enabled = prefs.getBoolean(KEY_VOICE_STT_ENABLED, false)
        val engine = normalizeEngine(enabled, prefs.getString(KEY_VOICE_STT_ENGINE, "off"))
        val autoSend = prefs.getBoolean(KEY_VOICE_AUTO_SEND, false)
        return VoiceOptInPrefs(enabled = enabled, engine = engine, autoSend = autoSend)
    }

    fun save(context: Context, state: VoiceOptInPrefs) {
        val normalized = normalizeEngine(state.enabled, state.engine)
        context.getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_VOICE_STT_ENABLED, state.enabled)
            .putString(KEY_VOICE_STT_ENGINE, normalized)
            .putBoolean(KEY_VOICE_AUTO_SEND, state.autoSend)
            .apply()
    }
}
