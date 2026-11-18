package dev.miku.link.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.miku.link.LinkNotification
import dev.miku.link.PermissionCardModel
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

@Composable
fun AlertsScreen(
    pendingCards: List<PermissionCardModel>,
    notifications: List<LinkNotification>,
    unreadCount: Int,
    onAllowWithBiometrics: (PermissionCardModel) -> Unit,
    onDeny: (PermissionCardModel) -> Unit,
    onRefreshAlerts: () -> Unit,
    onMarkAllRead: () -> Unit,
) {
    Column(
        modifier = Modifier.fillMaxSize(),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        CoucouPanel(modifier = Modifier.fillMaxWidth()) {
            Row(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = "Alertas y Biometría",
                        color = CoucouColors.Text,
                        fontWeight = FontWeight.SemiBold,
                        fontSize = 16.sp,
                    )
                    Text(
                        text = "$unreadCount sin leer · ${pendingCards.size} pendientes",
                        color = CoucouColors.TextDim,
                        fontSize = 12.sp,
                    )
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(
                        modifier = Modifier
                            .heightIn(min = 36.dp)
                            .clip(CircleShape)
                            .background(Color(0xFF1C1E24))
                            .border(1.dp, CoucouColors.Line, CircleShape)
                            .clickable(role = Role.Button, onClick = onRefreshAlerts)
                            .padding(horizontal = 14.dp, vertical = 8.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text("Sync", color = CoucouColors.Text, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                    }
                    Box(
                        modifier = Modifier
                            .heightIn(min = 36.dp)
                            .clip(CircleShape)
                            .background(CoucouColors.Text)
                            .clickable(role = Role.Button, onClick = onMarkAllRead)
                            .padding(horizontal = 14.dp, vertical = 8.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text("Leídas", color = CoucouColors.Bg, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
        }

        LazyColumn(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            items(pendingCards, key = { it.reqHash }) { card ->
                BiometricApprovalCard(
                    card = card,
                    onRequestBiometricAllow = onAllowWithBiometrics,
                    onInstantDeny = onDeny,
                )
            }
            if (notifications.isEmpty() && pendingCards.isEmpty()) {
                item {
                    CoucouPanel(modifier = Modifier.fillMaxWidth()) {
                        Column(
                            modifier = Modifier.padding(20.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Text(
                                text = "Todo limpio",
                                color = CoucouColors.Text,
                                fontWeight = FontWeight.SemiBold,
                                fontSize = 15.sp,
                            )
                            Text(
                                text = "Las respuestas y aprobaciones biométricas de Miku PC aparecerán aquí.",
                                color = CoucouColors.TextDim,
                                fontSize = 13.sp,
                                lineHeight = 18.sp,
                            )
                        }
                    }
                }
            } else {
                items(notifications, key = { it.id }) { n ->
                    val border = if (!n.read) CoucouColors.MikuAccent.copy(alpha = 0.45f) else CoucouColors.Line
                    val accent = when (n.level) {
                        "error" -> CoucouColors.Danger
                        "warning" -> Color(0xFFFFB86C)
                        else -> CoucouColors.MikuAccent
                    }
                    Surface(
                        shape = RoundedCornerShape(20.dp),
                        color = CoucouColors.Panel,
                        border = BorderStroke(1.dp, border),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Column(
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                            verticalArrangement = Arrangement.spacedBy(5.dp),
                        ) {
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Box(
                                        modifier = Modifier
                                            .size(7.dp)
                                            .clip(CircleShape)
                                            .background(accent),
                                    )
                                    Spacer(Modifier.width(7.dp))
                                    Text(
                                        text = "${n.title} · ${n.source}",
                                        color = CoucouColors.TextDim,
                                        fontWeight = FontWeight.Medium,
                                        fontSize = 12.sp,
                                    )
                                }
                                Text(
                                    text = formatTime(n.tsMs),
                                    color = CoucouColors.TextFaint,
                                    fontSize = 11.sp,
                                )
                            }
                            Text(
                                text = n.body,
                                color = CoucouColors.Text,
                                fontSize = 13.5.sp,
                                lineHeight = 19.sp,
                            )
                        }
                    }
                }
            }
        }
    }
}

private fun formatTime(tsMs: Long): String {
    if (tsMs <= 0L) return "ahora"
    return runCatching {
        SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(tsMs))
    }.getOrDefault("ahora")
}
