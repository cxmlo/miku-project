package dev.miku.link.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.miku.link.AgentFeedItem

@Composable
fun AgentFeedScreen(
    connected: Boolean,
    tokensSaved: Long,
    activeModel: String,
    agents: List<Pair<String, String>>,
    selectedAgentId: String,
    busyChat: Boolean,
    pendingCardsCount: Int,
    feedItems: List<AgentFeedItem>,
    offlineQueuedCount: Int,
    externalDraftPrompt: String = "",
    voiceAutoSend: Boolean = false,
    onToggleVoiceAutoSend: () -> Unit = {},
    onSelectAgent: (String) -> Unit,
    onOpenAlertsTab: () -> Unit,
    onSendPrompt: (String) -> Unit,
    onRemotePtt: () -> Unit,
    onClearExternalDraft: () -> Unit = {},
) {
    var draftPrompt by remember { mutableStateOf("") }

    LaunchedEffect(externalDraftPrompt) {
        if (externalDraftPrompt.isNotBlank()) {
            draftPrompt = externalDraftPrompt
            onClearExternalDraft()
        }
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .imePadding(),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        // Minimal status header
        CoucouPanel(modifier = Modifier.fillMaxWidth()) {
            Row(
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        text = if (connected) "Chat con Agentes" else "Cola local (offline)",
                        color = CoucouColors.Text,
                        fontWeight = FontWeight.SemiBold,
                        fontSize = 15.sp,
                    )
                    Text(
                        text = "$activeModel · $tokensSaved tok ahorrados",
                        color = CoucouColors.TextDim,
                        fontSize = 12.sp,
                    )
                }
                Row(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    MinimalPillChip(
                        label = if (voiceAutoSend) "Voz Auto" else "Voz Manual",
                        selected = voiceAutoSend,
                        onClick = onToggleVoiceAutoSend,
                    )
                    if (pendingCardsCount > 0) {
                        MinimalPillChip(
                            label = "$pendingCardsCount",
                            selected = true,
                            accentColor = CoucouColors.Danger,
                            onClick = onOpenAlertsTab,
                        )
                    }
                }
            }
        }

        // Agent selector pills
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            agents.forEach { (id, name) ->
                MinimalPillChip(
                    label = name,
                    selected = id == selectedAgentId,
                    onClick = { onSelectAgent(id) },
                )
            }
        }

        // Feed messages
        LazyColumn(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (feedItems.isEmpty()) {
                item {
                    CoucouPanel(modifier = Modifier.fillMaxWidth()) {
                        Column(
                            modifier = Modifier.padding(20.dp),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Text(
                                text = "Conversa con $selectedAgentId",
                                color = CoucouColors.Text,
                                fontWeight = FontWeight.SemiBold,
                                fontSize = 15.sp,
                            )
                            Text(
                                text = "Envía instrucciones de código, consulta Antigravity o revisa diffs de GitHub desde aquí.",
                                color = CoucouColors.TextDim,
                                fontSize = 13.sp,
                                lineHeight = 18.sp,
                            )
                        }
                    }
                }
            }
            items(feedItems) { item ->
                val isUser = item.kind == "prompt"
                val dotColor = when (item.kind) {
                    "error", "emergency", "pc_only_blocked" -> CoucouColors.Danger
                    "0-tokens · local" -> CoucouColors.Online
                    "prompt" -> CoucouColors.TextFaint
                    else -> CoucouColors.MikuAccent
                }
                Surface(
                    shape = RoundedCornerShape(20.dp),
                    color = if (isUser) Color(0xFF1C1E24) else CoucouColors.Panel,
                    border = BorderStroke(1.dp, CoucouColors.Line),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 13.dp),
                        verticalArrangement = Arrangement.spacedBy(5.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Box(
                                modifier = Modifier
                                    .size(7.dp)
                                    .clip(CircleShape)
                                    .background(dotColor),
                            )
                            Spacer(Modifier.width(7.dp))
                            Text(
                                text = if (isUser) "Tú · ${item.agent}" else "${item.agent} · ${item.kind}",
                                color = CoucouColors.TextDim,
                                fontSize = 11.5.sp,
                                fontWeight = FontWeight.Medium,
                            )
                        }
                        Text(
                            text = item.title,
                            color = CoucouColors.Text,
                            fontSize = 14.sp,
                            lineHeight = 20.sp,
                        )
                    }
                }
            }
        }

        if (offlineQueuedCount > 0) {
            Text(
                text = "Cola offline pendiente: $offlineQueuedCount mensajes",
                color = Color(0xFFFFB86C),
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 4.dp),
            )
        }

        // Minimal floating composer bar
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(26.dp))
                .background(CoucouColors.Panel)
                .border(1.dp, CoucouColors.Line, RoundedCornerShape(26.dp))
                .padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(
                modifier = Modifier
                    .weight(1f)
                    .heightIn(min = 36.dp)
                    .padding(horizontal = 4.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (draftPrompt.isEmpty()) {
                    Text(
                        text = if (busyChat) "Pensando..." else "Mensaje a $selectedAgentId...",
                        color = CoucouColors.TextFaint,
                        fontSize = 14.sp,
                    )
                }
                BasicTextField(
                    value = draftPrompt,
                    onValueChange = { draftPrompt = it },
                    textStyle = TextStyle(color = CoucouColors.Text, fontSize = 14.sp),
                    cursorBrush = SolidColor(CoucouColors.MikuAccent),
                    maxLines = 4,
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            Box(
                modifier = Modifier
                    .heightIn(min = 38.dp)
                    .clip(CircleShape)
                    .background(Color(0xFF1C1E24))
                    .border(1.dp, CoucouColors.Line, CircleShape)
                    .clickable(role = Role.Button, onClick = onRemotePtt)
                    .padding(horizontal = 13.dp, vertical = 9.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Voz", color = CoucouColors.Text, fontSize = 12.5.sp, fontWeight = FontWeight.Medium)
            }

            val canSend = !busyChat && draftPrompt.isNotBlank()
            Box(
                modifier = Modifier
                    .heightIn(min = 38.dp)
                    .clip(CircleShape)
                    .background(if (canSend) CoucouColors.Text else Color(0xFF23262F))
                    .clickable(enabled = !busyChat, role = Role.Button) {
                        if (draftPrompt.isNotBlank()) {
                            onSendPrompt(draftPrompt.trim())
                            draftPrompt = ""
                        }
                    }
                    .padding(horizontal = 16.dp, vertical = 9.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = if (busyChat) "..." else "Enviar",
                    color = if (canSend) CoucouColors.Bg else CoucouColors.TextDim,
                    fontSize = 12.5.sp,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}

@Composable
private fun MinimalPillChip(
    label: String,
    selected: Boolean,
    accentColor: Color = CoucouColors.Text,
    onClick: () -> Unit,
) {
    Box(
        modifier = Modifier
            .heightIn(min = 36.dp)
            .clip(CircleShape)
            .background(if (selected) accentColor else CoucouColors.Panel)
            .border(1.dp, if (selected) accentColor else CoucouColors.Line, CircleShape)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 8.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            color = if (selected) CoucouColors.Bg else CoucouColors.TextDim,
            fontSize = 12.5.sp,
            fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Medium,
        )
    }
}
