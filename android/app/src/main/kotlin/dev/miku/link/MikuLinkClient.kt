package dev.miku.link

import android.os.Handler
import android.os.Looper
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONArray
import org.json.JSONObject
import java.net.URI
import java.net.URLDecoder
import java.net.URLEncoder
import java.nio.charset.StandardCharsets
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

data class PairingConfig(
    val host: String = "127.0.0.1",
    val port: Int = 47400,
    val token: String = "",
    val protocol: String = "http",
    val nonce: String = "",
    val pin: String = "",
    val expMs: Long = 0L,
    val transport: String = "wifi",
    val desktopName: String = "",
    val deviceId: String = "android-phone",
    val deviceName: String = "Android Phone",
) {
    fun baseUrl(): String = "$protocol://$host:$port"

    fun wsUrl(): String {
        val wsProto = if (protocol == "https") "wss" else "ws"
        val base = "$wsProto://$host:$port/ws"
        if (token.isBlank()) return base
        val enc = URLEncoder.encode(token, StandardCharsets.UTF_8.name())
        return "$base?token=$enc"
    }

    fun hasQrChallenge(): Boolean = nonce.isNotBlank() && pin.isNotBlank()
}

data class GatewaySnapshot(
    val connected: Boolean = false,
    val killSwitch: Boolean = false,
    val permissionMode: String = "sandbox",
    val provider: String = "deepseek",
    val model: String = "deepseek-chat",
    val totalRequests: Int = 0,
    val localRoutedRequests: Long = 0,
    val estimatedTokensSaved: Long = 0,
    val agents: List<Pair<String, String>> = emptyList(),
)

data class LinkStatusSnapshot(
    val linkConnected: Boolean = false,
    val lanEnabled: Boolean = true,
    val lanIp: String = "127.0.0.1",
    val port: Int = 47400,
    val desktopName: String = "Miku PC",
    val killSwitch: Boolean = false,
    val permissionMode: String = "sandbox",
    val unreadNotifications: Int = 0,
    val pairedCount: Int = 0,
)

data class LinkNotification(
    val id: String,
    val tsMs: Long,
    val level: String,
    val category: String,
    val title: String,
    val body: String,
    val source: String,
    val read: Boolean,
)

data class NowPlayingState(
    val title: String = "Sin multimedia activo",
    val artist: String = "Windows GSMTC",
    val playing: Boolean = false,
    val volume: Int = 50,
    val hasSession: Boolean = false,
)

data class ChatDispatchResult(
    val ok: Boolean,
    val replyText: String,
    val localRoute: Boolean = false,
    val tokensSaved: Long = 0,
    val error: String = "",
)

object MikuLinkClient {
    private val jsonType = "application/json; charset=utf-8".toMediaType()
    private val ioPool = Executors.newCachedThreadPool()
    private val mainHandler by lazy { Handler(Looper.getMainLooper()) }

    private val httpClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(5, TimeUnit.SECONDS)
            .readTimeout(25, TimeUnit.SECONDS)
            .writeTimeout(10, TimeUnit.SECONDS)
            .build()
    }

    /**
     * Parses `miku-link://pair?v=1&host=...&port=...&nonce=...&pin=sha256:...&exp=...`,
     * `miku-link://pair?host=...&port=...&token=...`, raw URLs (`http://host:port`),
     * or bare `host:port` strings while rejecting empty/invalid hosts safely.
     */
    fun parsePairingInput(rawInput: String, useUsbLoopback: Boolean = false): PairingConfig {
        val trimmed = rawInput.trim()
        if (trimmed.isEmpty()) {
            val host = if (useUsbLoopback) "127.0.0.1" else "127.0.0.1"
            return PairingConfig(host = host, transport = if (useUsbLoopback) "usb" else "wifi")
        }
        val normalized = if (trimmed.contains("://")) trimmed else "http://$trimmed"
        val uri = runCatching { URI(normalized) }.getOrNull() ?: return PairingConfig()

        val queryMap = parseQuery(uri.rawQuery.orEmpty())
        val scheme = uri.scheme?.lowercase().orEmpty()
        val rawHost = if (scheme == "miku-link") {
            queryMap["host"]?.takeIf { it.isNotBlank() } ?: "127.0.0.1"
        } else {
            uri.host?.takeIf { it.isNotBlank() } ?: "127.0.0.1"
        }
        val port = if (scheme == "miku-link") {
            queryMap["port"]?.toIntOrNull()?.takeIf { it in 1..65535 } ?: 47400
        } else {
            uri.port.takeIf { it in 1..65535 } ?: 47400
        }
        val proto = when {
            scheme == "miku-link" && queryMap["proto"] == "https" -> "https"
            scheme == "https" -> "https"
            else -> "http"
        }
        val finalHost = if (useUsbLoopback) "127.0.0.1" else rawHost
        val transport = if (useUsbLoopback || finalHost == "127.0.0.1") "usb" else "wifi"
        return PairingConfig(
            host = finalHost,
            port = port,
            token = queryMap["token"].orEmpty(),
            protocol = proto,
            nonce = queryMap["nonce"].orEmpty(),
            pin = queryMap["pin"].orEmpty(),
            expMs = queryMap["exp"]?.toLongOrNull() ?: 0L,
            transport = transport,
        )
    }

    private fun parseQuery(rawQuery: String): Map<String, String> {
        if (rawQuery.isBlank()) return emptyMap()
        val out = LinkedHashMap<String, String>()
        for (pair in rawQuery.split('&')) {
            val idx = pair.indexOf('=')
            if (idx <= 0) continue
            val k = URLDecoder.decode(pair.substring(0, idx), StandardCharsets.UTF_8.name())
            val v = URLDecoder.decode(pair.substring(idx + 1), StandardCharsets.UTF_8.name())
            out[k] = v
        }
        return out
    }

    private fun withOptionalAuth(builder: Request.Builder, cfg: PairingConfig): Request.Builder {
        if (cfg.token.isNotBlank()) {
            builder.header("X-Miku-Token", cfg.token)
            builder.header("Authorization", "Bearer ${cfg.token}")
        }
        return builder
    }

    fun parseRedeemJson(rawBody: String, currentCfg: PairingConfig): PairingConfig {
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = JSONObject(rawBody)
        val deviceToken = obj.optString("device_token", "").trim()
        require(deviceToken.isNotEmpty()) {
            obj.optString("error", "El Gateway no emitió device_token")
        }
        val resolvedHost = if (currentCfg.transport == "usb" || currentCfg.host == "127.0.0.1") {
            "127.0.0.1"
        } else {
            obj.optString("lan_ip", "").trim().ifEmpty { currentCfg.host }
        }
        val resolvedPort = obj.optInt("port", currentCfg.port).takeIf { it in 1..65535 } ?: currentCfg.port
        return currentCfg.copy(
            host = resolvedHost,
            port = resolvedPort,
            token = deviceToken,
            nonce = "",
            pin = "",
            expMs = 0L,
            desktopName = obj.optString("desktop_name", currentCfg.desktopName),
            deviceId = obj.optString("device_id", currentCfg.deviceId),
            deviceName = obj.optString("device_name", currentCfg.deviceName),
        )
    }

    fun parseLinkStatusJson(rawBody: String): LinkStatusSnapshot {
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = JSONObject(rawBody)
        return LinkStatusSnapshot(
            linkConnected = obj.optBoolean("link_connected", false),
            lanEnabled = obj.optBoolean("lan_enabled", true),
            lanIp = obj.optString("lan_ip", "127.0.0.1"),
            port = obj.optInt("port", 47400),
            desktopName = obj.optString("desktop_name", "Miku PC"),
            killSwitch = obj.optBoolean("kill_switch", false),
            permissionMode = obj.optString("permission_mode", "sandbox"),
            unreadNotifications = obj.optInt("unread_notifications", 0),
            pairedCount = obj.optInt("paired_count", 0),
        )
    }

    fun parseNotificationsJson(rawBody: String): Pair<List<LinkNotification>, Int> {
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = JSONObject(rawBody)
        val arr = obj.optJSONArray("notifications") ?: JSONArray()
        val list = ArrayList<LinkNotification>(arr.length())
        for (i in 0 until arr.length()) {
            val item = arr.optJSONObject(i) ?: continue
            val id = item.optString("id", "").trim()
            if (id.isEmpty()) continue
            list.add(
                LinkNotification(
                    id = id,
                    tsMs = item.optLong("ts_ms", 0L),
                    level = item.optString("level", "info"),
                    category = item.optString("category", "general"),
                    title = item.optString("title", "Miku Desktop"),
                    body = item.optString("body", ""),
                    source = item.optString("source", "desktop"),
                    read = item.optBoolean("read", false),
                )
            )
        }
        val sorted = list.sortedByDescending { it.tsMs }
        val unread = obj.optInt("unread_count", sorted.count { !it.read })
        return sorted to unread
    }

    fun parseNowPlayingJson(rawBody: String): NowPlayingState {
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = JSONObject(rawBody)
        val np = obj.optJSONObject("now_playing") ?: obj
        return NowPlayingState(
            title = np.optString("title", "").ifBlank { "Sin reproducción activa" },
            artist = np.optString("artist", "").ifBlank { "Windows GSMTC" },
            playing = np.optBoolean("playing", false),
            volume = obj.optInt("volume", np.optInt("volume", 50)).coerceIn(0, 100),
            hasSession = np.optBoolean("has_session", false),
        )
    }

    fun parseSnapshotJson(rawBody: String): GatewaySnapshot {
        // Security invariant: companion app never accepts raw provider keys over the wire.
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = JSONObject(rawBody)
        val settings = obj.optJSONObject("settings")
        val telemetry = obj.optJSONObject("telemetry")
        val agentsArr = obj.optJSONArray("agents") ?: JSONArray()
        val agents = ArrayList<Pair<String, String>>(agentsArr.length())
        for (i in 0 until agentsArr.length()) {
            val item = agentsArr.optJSONObject(i) ?: continue
            if (!item.optBoolean("active", true)) continue
            val id = item.optString("id", "").trim()
            val name = item.optString("name", id).trim()
            if (id.isNotEmpty()) {
                agents.add(id to name.ifEmpty { id })
            }
        }
        return GatewaySnapshot(
            connected = true,
            killSwitch = obj.optBoolean("kill_switch", false),
            permissionMode = obj.optString("permission_mode", settings?.optString("permission_mode", "sandbox") ?: "sandbox"),
            provider = settings?.optString("provider", "deepseek") ?: "deepseek",
            model = settings?.optString("model", "deepseek-chat") ?: "deepseek-chat",
            totalRequests = telemetry?.optInt("total_requests", 0) ?: 0,
            localRoutedRequests = telemetry?.optLong("local_routed_requests", 0L) ?: 0L,
            estimatedTokensSaved = telemetry?.optLong("estimated_tokens_saved", 0L) ?: 0L,
            agents = agents,
        )
    }

    fun redeemQrPairing(cfg: PairingConfig, onResult: (Result<PairingConfig>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject()
                    .put("nonce", cfg.nonce)
                    .put("pin", cfg.pin)
                    .put("device_id", cfg.deviceId)
                    .put("device_name", cfg.deviceName)
                    .put("platform", "android")
                    .put("app_version", "0.2.0")
                    .put("transport", cfg.transport)
                    .toString()
                val req = Request.Builder()
                    .url("${cfg.baseUrl()}/api/link/redeem")
                    .post(payload.toRequestBody(jsonType))
                    .build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) {
                        val errMsg = runCatching { JSONObject(body).optString("error") }.getOrNull()
                        error("HTTP ${resp.code}: ${errMsg?.ifBlank { body.take(120) } ?: body.take(120)}")
                    }
                    parseRedeemJson(body, cfg)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun fetchSnapshot(cfg: PairingConfig, onResult: (Result<GatewaySnapshot>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val req = withOptionalAuth(
                    Request.Builder().url("${cfg.baseUrl()}/api/state").get(),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) {
                        error("HTTP ${resp.code}: ${body.take(160)}")
                    }
                    parseSnapshotJson(body)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun fetchLinkStatus(cfg: PairingConfig, onResult: (Result<LinkStatusSnapshot>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val req = withOptionalAuth(
                    Request.Builder().url("${cfg.baseUrl()}/api/link/status").get(),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                    parseLinkStatusJson(body)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun sendHeartbeat(
        cfg: PairingConfig,
        batteryPct: Int,
        charging: Boolean,
        onResult: (Result<Unit>) -> Unit = {},
    ) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject()
                    .put("device_id", cfg.deviceId)
                    .put("battery_pct", batteryPct.coerceIn(0, 100))
                    .put("battery_charging", charging)
                    .put("transport", cfg.transport)
                    .toString()
                val req = withOptionalAuth(
                    Request.Builder()
                        .url("${cfg.baseUrl()}/api/link/heartbeat")
                        .post(payload.toRequestBody(jsonType)),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun sendCommand(
        cfg: PairingConfig,
        action: String,
        extras: Map<String, Any> = emptyMap(),
        onResult: (Result<JSONObject>) -> Unit,
    ) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject().put("action", action)
                for ((k, v) in extras) {
                    payload.put(k, v)
                }
                val req = withOptionalAuth(
                    Request.Builder()
                        .url("${cfg.baseUrl()}/api/link/command")
                        .post(payload.toString().toRequestBody(jsonType)),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    MikuMobileBridge.assertNoProviderApiKeys(body)
                    val obj = runCatching { JSONObject(body) }.getOrElse { JSONObject() }
                    if (!resp.isSuccessful || !obj.optBoolean("ok", true)) {
                        error(obj.optString("error", "HTTP ${resp.code}"))
                    }
                    obj
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun buildPermissionDecisionJson(
        card: PermissionCardModel,
        outcome: DecisionOutcome,
        proof: BiometricProofModel?,
    ): JSONObject {
        val payload = JSONObject().apply {
            put("type", "permission_decision")
            put("req_hash", card.reqHash)
            put("decision", outcome.name.lowercase())
            put("agent", card.agent)
            put("capability", card.capability)
            put("target", card.target)
            if (proof != null) {
                val proofObj = JSONObject().apply {
                    put("verified", proof.verified)
                    put("auth_method", proof.authMethod)
                    put("bound_req_hash", proof.boundReqHash)
                    put("verified_at_ms", proof.verifiedAtMs)
                }
                put("biometric_proof", proofObj)
            }
        }
        MikuMobileBridge.assertNoProviderApiKeys(payload.toString())
        return payload
    }

    fun sendPermissionDecision(
        cfg: PairingConfig,
        card: PermissionCardModel,
        outcome: DecisionOutcome,
        proof: BiometricProofModel?,
        onResult: (Result<JSONObject>) -> Unit = {},
    ) {
        val extras = mutableMapOf<String, Any>(
            "req_hash" to card.reqHash,
            "decision" to outcome.name.lowercase(),
            "agent" to card.agent,
            "capability" to card.capability,
            "target" to card.target,
        )
        if (proof != null) {
            extras["biometric_verified"] = proof.verified
            extras["auth_method"] = proof.authMethod
            extras["bound_req_hash"] = proof.boundReqHash
            extras["verified_at_ms"] = proof.verifiedAtMs
        }
        sendCommand(cfg, "permission_decision", extras, onResult)
    }

    fun fetchNotifications(
        cfg: PairingConfig,
        onResult: (Result<Pair<List<LinkNotification>, Int>>) -> Unit,
    ) {
        ioPool.execute {
            val res = runCatching {
                val req = withOptionalAuth(
                    Request.Builder().url("${cfg.baseUrl()}/api/link/notifications").get(),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                    parseNotificationsJson(body)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun ackNotifications(cfg: PairingConfig, id: String = "all", onResult: (Result<Int>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject().put("id", id).toString()
                val req = withOptionalAuth(
                    Request.Builder()
                        .url("${cfg.baseUrl()}/api/link/notifications/ack")
                        .post(payload.toRequestBody(jsonType)),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                    JSONObject(body).optInt("unread_count", 0)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun fetchNowPlaying(cfg: PairingConfig, onResult: (Result<NowPlayingState>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val req = withOptionalAuth(
                    Request.Builder().url("${cfg.baseUrl()}/api/media/now_playing").get(),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                    parseNowPlayingJson(body)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun parseChatResponseJson(
        rawBody: String,
        httpCode: Int,
        isSuccessful: Boolean,
    ): ChatDispatchResult {
        MikuMobileBridge.assertNoProviderApiKeys(rawBody)
        val obj = runCatching { JSONObject(rawBody) }.getOrElse { JSONObject() }
        val errField = obj.opt("error")
        val hasErrBool = (errField as? Boolean) == true
        val errString = (errField as? String).orEmpty().trim()
        val text = obj.optString("text", obj.optString("reply", "")).trim()
        if (!isSuccessful || hasErrBool || errString.isNotEmpty()) {
            val detail = when {
                errString.isNotEmpty() -> errString
                text.isNotEmpty() -> text
                else -> "HTTP $httpCode"
            }
            return ChatDispatchResult(
                ok = false,
                replyText = text,
                error = detail,
            )
        }
        return ChatDispatchResult(
            ok = obj.optBoolean("ok", true),
            replyText = text.ifEmpty { "Solicitud enviada al Gateway" },
            localRoute = obj.optBoolean("routed_local", obj.optBoolean("local_route", false)),
            tokensSaved = obj.optLong("tokens_saved", 0L),
            error = "",
        )
    }

    fun sendChat(
        cfg: PairingConfig,
        prompt: String,
        agentId: String,
        onResult: (ChatDispatchResult) -> Unit,
    ) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject()
                    .put("text", prompt)
                    .put("prompt", prompt)
                    .put("agent_id", agentId)
                    .toString()
                val req = withOptionalAuth(
                    Request.Builder()
                        .url("${cfg.baseUrl()}/api/chat")
                        .post(payload.toRequestBody(jsonType)),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    parseChatResponseJson(body, resp.code, resp.isSuccessful)
                }
            }.getOrElse { err ->
                ChatDispatchResult(
                    ok = false,
                    replyText = "",
                    error = err.message ?: "Error de conexión con Gateway",
                )
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun toggleKillSwitch(cfg: PairingConfig, armed: Boolean, onResult: (Result<Boolean>) -> Unit) {
        ioPool.execute {
            val res = runCatching {
                val payload = JSONObject().put("active", armed).toString()
                val req = withOptionalAuth(
                    Request.Builder()
                        .url("${cfg.baseUrl()}/api/gatekeeper/kill_switch")
                        .post(payload.toRequestBody(jsonType)),
                    cfg,
                ).build()
                httpClient.newCall(req).execute().use { resp ->
                    val body = resp.body?.string().orEmpty()
                    if (!resp.isSuccessful) error("HTTP ${resp.code}")
                    val obj = runCatching { JSONObject(body) }.getOrElse { JSONObject() }
                    obj.optBoolean("kill_switch", armed)
                }
            }
            mainHandler.post { onResult(res) }
        }
    }

    fun connectWebSocket(
        cfg: PairingConfig,
        onEvent: (JSONObject) -> Unit,
        onStateChange: (Boolean) -> Unit,
    ): WebSocket? {
        if (cfg.token.isBlank()) return null
        val req = withOptionalAuth(Request.Builder().url(cfg.wsUrl()), cfg).build()
        return httpClient.newWebSocket(
            req,
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    mainHandler.post { onStateChange(true) }
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    runCatching {
                        MikuMobileBridge.assertNoProviderApiKeys(text)
                        JSONObject(text)
                    }.getOrNull()?.let { obj ->
                        mainHandler.post { onEvent(obj) }
                    }
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    mainHandler.post { onStateChange(false) }
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    mainHandler.post { onStateChange(false) }
                }
            },
        )
    }
}
