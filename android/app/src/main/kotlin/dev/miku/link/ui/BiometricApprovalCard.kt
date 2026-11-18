package dev.miku.link.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.miku.link.PermissionCardModel

@Composable
fun BiometricApprovalCard(
    card: PermissionCardModel,
    onRequestBiometricAllow: (PermissionCardModel) -> Unit,
    onInstantDeny: (PermissionCardModel) -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 8.dp),
        colors = CardDefaults.cardColors(containerColor = Color(0xFF161923)),
    ) {
        Column(modifier = Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                text = "${card.agent} -> ${card.capability}",
                style = MaterialTheme.typography.titleMedium,
                color = Color(0xFF8BE9FD),
            )
            Text(
                text = card.target,
                style = MaterialTheme.typography.bodyMedium,
                color = Color(0xFFF8F8F2),
                fontFamily = FontFamily.Monospace,
            )
            if (card.args.isNotBlank()) {
                Text(
                    text = "args: ${card.args}",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color(0xFFBD93F9),
                    fontFamily = FontFamily.Monospace,
                )
            }
            Text(
                text = "req_hash: ${card.reqHash.take(24)}...",
                style = MaterialTheme.typography.labelSmall,
                color = Color(0xFF9EA7B8),
                fontFamily = FontFamily.Monospace,
            )
            if (card.pcOnly) {
                Text(
                    text = "PC ONLY: Approve directly on desktop Island (mobile Allow disabled)",
                    style = MaterialTheme.typography.labelMedium,
                    color = Color(0xFFFFB86C),
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedButton(
                    onClick = { onInstantDeny(card) },
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Deny (1-tap)")
                }
                Button(
                    onClick = { onRequestBiometricAllow(card) },
                    enabled = !card.pcOnly,
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF50FA7B)),
                    modifier = Modifier.weight(1f),
                ) {
                    Text("Allow (Biometric)", color = Color(0xFF0D1117))
                }
            }
        }
    }
}
