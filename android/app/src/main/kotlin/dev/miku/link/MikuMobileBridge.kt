package dev.miku.link

import java.security.MessageDigest

data class PermissionCardModel(
    val reqHash: String,
    val agent: String,
    val capability: String,
    val target: String,
    val args: String,
    val ttlSec: Int,
    val receivedAtMs: Long,
    val pcOnly: Boolean,
)

data class BiometricProofModel(
    val verified: Boolean,
    val authMethod: String,
    val boundReqHash: String,
    val verifiedAtMs: Long,
)

data class AgentFeedItem(
    val agent: String,
    val kind: String,
    val title: String,
    val receivedAtMs: Long,
)

enum class DecisionOutcome {
    ALLOW,
    DENY,
}

sealed class MobileGuardError(message: String) : IllegalStateException(message) {
    class PcOnlyBlocked(cap: String) :
        MobileGuardError("High-risk capability '$cap' is marked pc_only and cannot be approved from mobile")

    class ReqHashMismatch(expected: String, actual: String) :
        MobileGuardError("Displayed params hash ($actual) does not match signed reqHash ($expected)")

    object BiometricRequired :
        MobileGuardError("Allow requires AndroidX BiometricPrompt bound to reqHash")

    object BiometricBindingMismatch :
        MobileGuardError("Biometric proof boundReqHash does not match displayed card")

    class ProviderApiKeyForbidden(marker: String) :
        MobileGuardError("Provider API key storage on Android is forbidden (detected: $marker)")
}

object MikuMobileBridge {
    private val forbiddenMarkers = listOf(
        "gsk_",
        "sk-ant-",
        "sk-proj-",
        "sk-live-",
        "anthropic_api_key",
        "openai_api_key",
        "groq_api_key",
        "provider_api_key",
        "gemini_api_key",
    )

    /**
     * Exact canonical SHA-256 contract matching Rust `miku_gatekeeper::GatekeeperEngine::compute_request_hash`
     * and UniFFI `miku_mobile_core::compute_req_hash_v1`.
     */
    fun computeReqHashV1(
        agent: String,
        capability: String,
        canonicalTarget: String,
        canonicalArgs: String,
    ): String {
        val payload =
            "MIKU_REQ_HASH_V1|agent=${agent.trim()}|cap=${capability.trim()}|target=$canonicalTarget|args=${canonicalArgs.trim()}"
        val digest = MessageDigest.getInstance("SHA-256").digest(payload.toByteArray(Charsets.UTF_8))
        val hex = digest.joinToString("") { "%02x".format(it) }
        return "sha256:$hex"
    }

    fun assertNoProviderApiKeys(rawState: String) {
        val lower = rawState.lowercase()
        for (marker in forbiddenMarkers) {
            if (lower.contains(marker)) {
                throw MobileGuardError.ProviderApiKeyForbidden(marker)
            }
        }
    }

    fun evaluateDecision(
        card: PermissionCardModel,
        requested: DecisionOutcome,
        biometric: BiometricProofModel?,
        nowMs: Long,
    ): DecisionOutcome {
        val recomputed = computeReqHashV1(card.agent, card.capability, card.target, card.args)
        if (!constantTimeEquals(recomputed, card.reqHash)) {
            throw MobileGuardError.ReqHashMismatch(card.reqHash, recomputed)
        }

        val expired = nowMs > (card.receivedAtMs + card.ttlSec.toLong() * 1000L)
        if (expired || requested == DecisionOutcome.DENY) {
            return DecisionOutcome.DENY
        }

        if (card.pcOnly) {
            throw MobileGuardError.PcOnlyBlocked(card.capability)
        }
        if (biometric == null || !biometric.verified) {
            throw MobileGuardError.BiometricRequired
        }
        if (!constantTimeEquals(biometric.boundReqHash, recomputed)) {
            throw MobileGuardError.BiometricBindingMismatch
        }
        return DecisionOutcome.ALLOW
    }

    private fun constantTimeEquals(a: String, b: String): Boolean {
        if (a.length != b.length) return false
        var diff = 0
        for (i in a.indices) {
            diff = diff or (a[i].code xor b[i].code)
        }
        return diff == 0
    }
}
