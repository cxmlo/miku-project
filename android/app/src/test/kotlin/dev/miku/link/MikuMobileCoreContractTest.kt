package dev.miku.link

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class MikuMobileCoreContractTest {

    @Test
    fun allowRequiresReqHashMatchAndBiometricBindingWhileDenyWorksWithOneTap() {
        val reqHash = MikuMobileBridge.computeReqHashV1(
            agent = "claude-code",
            capability = "fs_write",
            canonicalTarget = "src/main.rs",
            canonicalArgs = "patch",
        )
        val card = PermissionCardModel(
            reqHash = reqHash,
            agent = "claude-code",
            capability = "fs_write",
            target = "src/main.rs",
            args = "patch",
            ttlSec = 60,
            receivedAtMs = 1_000L,
            pcOnly = false,
        )

        // 1-tap Deny succeeds without biometric proof
        assertEquals(
            DecisionOutcome.DENY,
            MikuMobileBridge.evaluateDecision(card, DecisionOutcome.DENY, null, 2_000L),
        )

        // Allow without biometrics throws
        assertThrows(MobileGuardError.BiometricRequired::class.java) {
            MikuMobileBridge.evaluateDecision(card, DecisionOutcome.ALLOW, null, 2_000L)
        }

        // Allow with wrong bound hash throws
        assertThrows(MobileGuardError.BiometricBindingMismatch::class.java) {
            MikuMobileBridge.evaluateDecision(
                card,
                DecisionOutcome.ALLOW,
                BiometricProofModel(true, "fingerprint", "sha256:bad", 2_000L),
                2_000L,
            )
        }

        // Allow with valid biometric bound to reqHash passes
        assertEquals(
            DecisionOutcome.ALLOW,
            MikuMobileBridge.evaluateDecision(
                card,
                DecisionOutcome.ALLOW,
                BiometricProofModel(true, "fingerprint", reqHash, 2_000L),
                2_000L,
            ),
        )
    }

    @Test
    fun pcOnlyCannotBeApprovedAndTamperedParamsFailReqHashCheck() {
        val reqHash = MikuMobileBridge.computeReqHashV1("cursor", "shell", "cargo test", "")
        val pcOnlyCard = PermissionCardModel(
            reqHash = reqHash,
            agent = "cursor",
            capability = "shell",
            target = "cargo test",
            args = "",
            ttlSec = 60,
            receivedAtMs = 10_000L,
            pcOnly = true,
        )
        assertThrows(MobileGuardError.PcOnlyBlocked::class.java) {
            MikuMobileBridge.evaluateDecision(
                pcOnlyCard,
                DecisionOutcome.ALLOW,
                BiometricProofModel(true, "fingerprint", reqHash, 10_500L),
                10_600L,
            )
        }

        val tampered = pcOnlyCard.copy(pcOnly = false, target = "README.md")
        assertThrows(MobileGuardError.ReqHashMismatch::class.java) {
            MikuMobileBridge.evaluateDecision(
                tampered,
                DecisionOutcome.ALLOW,
                BiometricProofModel(true, "fingerprint", reqHash, 10_500L),
                10_600L,
            )
        }
    }

    @Test
    fun providerApiKeysRejectedAndTtlExpiryResolvesToDeny() {
        assertThrows(MobileGuardError.ProviderApiKeyForbidden::class.java) {
            MikuMobileBridge.assertNoProviderApiKeys("""{"groq":"gsk_abc123"}""")
        }

        val reqHash = MikuMobileBridge.computeReqHashV1("antigravity", "network", "https://docs.rs", "GET")
        val card = PermissionCardModel(
            reqHash = reqHash,
            agent = "antigravity",
            capability = "network",
            target = "https://docs.rs",
            args = "GET",
            ttlSec = 10,
            receivedAtMs = 5_000L,
            pcOnly = false,
        )
        assertEquals(
            DecisionOutcome.DENY,
            MikuMobileBridge.evaluateDecision(
                card,
                DecisionOutcome.ALLOW,
                BiometricProofModel(true, "fingerprint", reqHash, 20_000L),
                20_000L,
            ),
        )
    }

    @Test
    fun pcOnlyCardPermitsDenyOnMobileWhileThrowingOnAllow() {
        val reqHash = MikuMobileBridge.computeReqHashV1("antigravity", "shell", "rm -rf /", "")
        val pcOnlyCard = PermissionCardModel(
            reqHash = reqHash,
            agent = "antigravity",
            capability = "shell",
            target = "rm -rf /",
            args = "",
            ttlSec = 60,
            receivedAtMs = 50_000L,
            pcOnly = true,
        )

        // Deny is always allowed from mobile, even for pc_only cards
        val denyResult = MikuMobileBridge.evaluateDecision(
            card = pcOnlyCard,
            requested = DecisionOutcome.DENY,
            biometric = null,
            nowMs = 55_000L,
        )
        assertEquals(DecisionOutcome.DENY, denyResult)

        // Allow throws MobileGuardError.PcOnlyBlocked
        assertThrows(MobileGuardError.PcOnlyBlocked::class.java) {
            MikuMobileBridge.evaluateDecision(
                card = pcOnlyCard,
                requested = DecisionOutcome.ALLOW,
                biometric = BiometricProofModel(true, "fingerprint", reqHash, 55_000L),
                nowMs = 55_000L,
            )
        }
    }
}
