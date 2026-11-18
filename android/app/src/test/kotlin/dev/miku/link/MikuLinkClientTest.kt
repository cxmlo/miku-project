package dev.miku.link

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MikuLinkClientTest {

    @Test
    fun parsePairingInputSupportsDeepLinkQrNonceAndUsbLoopback() {
        val qr = MikuLinkClient.parsePairingInput(
            "miku-link://pair?host=192.168.1.44&port=47400&token=abc123",
        )
        assertEquals("192.168.1.44", qr.host)
        assertEquals(47400, qr.port)
        assertEquals("abc123", qr.token)
        assertEquals("http://192.168.1.44:47400", qr.baseUrl())
        assertEquals("wifi", qr.transport)

        val v1Qr = MikuLinkClient.parsePairingInput(
            "miku-link://pair?v=1&host=192.168.1.88&port=47400&nonce=n999&pin=sha256:abcd1234&exp=1999999999000",
        )
        assertTrue(v1Qr.hasQrChallenge())
        assertEquals("n999", v1Qr.nonce)
        assertEquals("sha256:abcd1234", v1Qr.pin)
        assertEquals(1999999999000L, v1Qr.expMs)

        val usbFromQr = MikuLinkClient.parsePairingInput(
            "miku-link://pair?v=1&host=192.168.1.88&port=47400&nonce=n999&pin=sha256:abcd1234",
            useUsbLoopback = true,
        )
        assertEquals("127.0.0.1", usbFromQr.host)
        assertEquals("usb", usbFromQr.transport)

        val usb = MikuLinkClient.parsePairingInput("127.0.0.1:47400")
        assertEquals("127.0.0.1", usb.host)
        assertEquals(47400, usb.port)
        assertEquals("http://127.0.0.1:47400", usb.baseUrl())
    }

    @Test
    fun parseRedeemAndNotificationsAndRejectLeakedKeys() {
        val initial = MikuLinkClient.parsePairingInput(
            "miku-link://pair?v=1&host=192.168.1.50&port=47400&nonce=abc&pin=sha256:ff",
        )
        val redeemed = MikuLinkClient.parseRedeemJson(
            """
            {
              "ok": true,
              "device_token": "ml_dev_tok_789",
              "device_id": "pixel-8",
              "device_name": "Pixel 8",
              "desktop_name": "MIKU-WORKSTATION",
              "lan_ip": "192.168.1.50",
              "port": 47400
            }
            """.trimIndent(),
            initial,
        )
        assertEquals("ml_dev_tok_789", redeemed.token)
        assertEquals("MIKU-WORKSTATION", redeemed.desktopName)
        assertFalse(redeemed.hasQrChallenge())

        val (list, unread) = MikuLinkClient.parseNotificationsJson(
            """
            {
              "ok": true,
              "unread_count": 1,
              "notifications": [
                { "id": "ln-1", "ts_ms": 1000, "level": "info", "category": "link", "title": "Conectado", "body": "OK", "source": "pc", "read": true },
                { "id": "ln-2", "ts_ms": 2000, "level": "info", "category": "agent", "title": "Miku respondió", "body": "Listo", "source": "pc-gateway", "read": false }
              ]
            }
            """.trimIndent(),
        )
        assertEquals(1, unread)
        assertEquals(2, list.size)
        assertEquals("ln-2", list.first().id)

        val status = MikuLinkClient.parseLinkStatusJson(
            """{"ok":true,"link_connected":true,"desktop_name":"MIKU-PC","permission_mode":"restricted","unread_notifications":3,"paired_count":1}""",
        )
        assertTrue(status.linkConnected)
        assertEquals("restricted", status.permissionMode)
        assertEquals(3, status.unreadNotifications)
    }

    @Test
    fun parseSnapshotJsonParsesActiveAgentsAndRejectsLeakedSecrets() {
        val valid = """
            {
              "kill_switch": false,
              "permission_mode": "sandbox",
              "settings": { "provider": "deepseek", "model": "deepseek-chat" },
              "telemetry": { "total_requests": 4, "local_routed_requests": 2, "estimated_tokens_saved": 840 },
              "agents": [
                { "id": "miku", "name": "Miku", "active": true },
                { "id": "antigravity", "name": "Antigravity", "active": true },
                { "id": "cursor", "name": "Cursor", "active": false }
              ]
            }
        """.trimIndent()
        val snap = MikuLinkClient.parseSnapshotJson(valid)
        assertTrue(snap.connected)
        assertFalse(snap.killSwitch)
        assertEquals("sandbox", snap.permissionMode)
        assertEquals(840L, snap.estimatedTokensSaved)
        assertEquals(listOf("miku" to "Miku", "antigravity" to "Antigravity"), snap.agents)

        var rejected = false
        try {
            MikuLinkClient.parseSnapshotJson("""{"api_key":"sk-ant-123456"}""")
        } catch (_: MobileGuardError.ProviderApiKeyForbidden) {
            rejected = true
        }
        assertTrue("Provider secret markers in Gateway payload must throw", rejected)
    }

    @Test
    fun parseChatResponseJsonHandlesRoutedLocalAndBooleanError() {
        val okLocal = MikuLinkClient.parseChatResponseJson(
            """{"agent_id":"miku","error":false,"routed_local":true,"tokens_saved":420,"text":"Hola!"}""",
            200,
            true,
        )
        assertTrue(okLocal.ok)
        assertTrue(okLocal.localRoute)
        assertEquals(420L, okLocal.tokensSaved)
        assertEquals("Hola!", okLocal.replyText)

        val errReply = MikuLinkClient.parseChatResponseJson(
            """{"agent_id":"miku","error":true,"text":"Bloqueado por Gatekeeper"}""",
            200,
            true,
        )
        assertFalse(errReply.ok)
        assertEquals("Bloqueado por Gatekeeper", errReply.error)
    }

    @Test
    fun buildPermissionDecisionJsonCreatesCanonicalPayloadWithoutSecrets() {
        val reqHash = MikuMobileBridge.computeReqHashV1("miku", "shell", "cargo build", "")
        val card = PermissionCardModel(
            reqHash = reqHash,
            agent = "miku",
            capability = "shell",
            target = "cargo build",
            args = "",
            ttlSec = 45,
            receivedAtMs = 1000L,
            pcOnly = false,
        )
        val proof = BiometricProofModel(
            verified = true,
            authMethod = "androidx_biometric_strong",
            boundReqHash = reqHash,
            verifiedAtMs = 1500L,
        )
        val json = MikuLinkClient.buildPermissionDecisionJson(card, DecisionOutcome.ALLOW, proof)
        assertEquals("permission_decision", json.getString("type"))
        assertEquals(reqHash, json.getString("req_hash"))
        assertEquals("allow", json.getString("decision"))
        assertEquals("miku", json.getString("agent"))
        assertEquals("shell", json.getString("capability"))
        assertEquals("cargo build", json.getString("target"))

        val proofObj = json.getJSONObject("biometric_proof")
        assertTrue(proofObj.getBoolean("verified"))
        assertEquals("androidx_biometric_strong", proofObj.getString("auth_method"))
        assertEquals(reqHash, proofObj.getString("bound_req_hash"))
    }

    @Test
    fun historyStoreSerializesAndDeserializesAcrossRestarts() {
        val original = listOf(
            AgentFeedItem("miku", "reply", "Hola, estoy lista para ayudarte.", 1700000001000L),
            AgentFeedItem("user", "prompt", "Estado del sistema", 1700000000000L),
            AgentFeedItem("claude-code", "permission_allow", "fs_write sobre src/main.rs", 1700000002000L),
        )
        val serialized = MikuHistoryStore.serializeFeedHistory(original)
        assertFalse(serialized.isBlank())

        val restored = MikuHistoryStore.deserializeFeedHistory(serialized)
        assertEquals(3, restored.size)
        assertEquals("miku", restored[0].agent)
        assertEquals("reply", restored[0].kind)
        assertEquals("Hola, estoy lista para ayudarte.", restored[0].title)
        assertEquals(1700000001000L, restored[0].receivedAtMs)
        assertEquals("user", restored[1].agent)
        assertEquals("claude-code", restored[2].agent)

        // Corrupted JSON returns empty list safely
        val emptyOnCorrupted = MikuHistoryStore.deserializeFeedHistory("{not valid json array")
        assertTrue(emptyOnCorrupted.isEmpty())

        val emptyOnBlank = MikuHistoryStore.deserializeFeedHistory("")
        assertTrue(emptyOnBlank.isEmpty())
    }

    @Test
    fun voiceOptInStoreBlocksCaptureByDefaultUntilExplicitOptIn() {
        val defaultPrefs = VoiceOptInPrefs()
        assertFalse(defaultPrefs.enabled)
        assertEquals("off", defaultPrefs.engine)
        assertFalse(defaultPrefs.autoSend)
        assertFalse(
            MikuVoiceOptInStore.canLaunchSpeechCapture(
                defaultPrefs.enabled,
                defaultPrefs.engine,
            ),
        )
        assertFalse(MikuVoiceOptInStore.canLaunchSpeechCapture(true, "off"))
        assertEquals("off", MikuVoiceOptInStore.normalizeEngine(false, "android_recognizer"))
        assertEquals(
            "android_recognizer",
            MikuVoiceOptInStore.normalizeEngine(true, "unknown_engine"),
        )
        assertTrue(MikuVoiceOptInStore.canLaunchSpeechCapture(true, "android_recognizer"))
    }
}
