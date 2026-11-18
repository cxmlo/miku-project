package dev.miku.link.ui

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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.miku.link.NowPlayingState

@Composable
fun PhoneLinkDashboard(
    connected: Boolean,
    desktopName: String,
    transportLabel: String,
    permissionMode: String,
    killSwitchArmed: Boolean,
    tokensSaved: Long,
    nowPlaying: NowPlayingState,
    actionNote: String,
    onKillSwitch: () -> Unit,
    onSetPermissionMode: (String) -> Unit,
    onPcCommand: (String, Map<String, Any>) -> Unit,
    onRefreshMedia: () -> Unit,
) {
    var clipDraft by remember { mutableStateOf("") }

    val washColor = when {
        killSwitchArmed -> CoucouColors.Danger
        connected -> CoucouColors.Online
        else -> CoucouColors.TextFaint
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(CoucouColors.Bg)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        CoucouHeroPanel(wash = washColor) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(
                        modifier = Modifier
                            .size(10.dp)
                            .clip(CircleShape)
                            .background(washColor),
                    )
                    Column {
                        Text(
                            text = if (connected) desktopName.ifBlank { "Miku PC" } else "PC desconectada",
                            color = CoucouColors.Text,
                            fontSize = 18.sp,
                            fontWeight = FontWeight.Bold,
                        )
                        Text(
                            text = "${transportLabel.uppercase()} · Ahorro: $tokensSaved tokens",
                            color = CoucouColors.TextDim,
                            fontSize = 12.sp,
                        )
                    }
                }
                CoucouPillButton(
                    label = if (killSwitchArmed) "Reanudar" else "Kill Switch",
                    onClick = onKillSwitch,
                    danger = !killSwitchArmed,
                    primary = killSwitchArmed,
                )
            }

            if (actionNote.isNotBlank()) {
                Spacer(Modifier.height(10.dp))
                Text(
                    text = actionNote,
                    color = CoucouColors.MikuAccent,
                    fontSize = 12.sp,
                )
            }
        }

        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            // Modo de permisos Gatekeeper (píldoras minimalistas estilo Coucou)
            CoucouPanel {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        text = "Modo Gatekeeper",
                        color = CoucouColors.Text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        listOf("sandbox" to "Sandbox", "ask" to "Preguntar", "full" to "Full").forEach { (mode, label) ->
                            val on = permissionMode == mode
                            Box(
                                modifier = Modifier
                                    .weight(1f)
                                    .clip(CircleShape)
                                    .background(if (on) CoucouColors.PrimaryBtn else CoucouColors.Panel2)
                                    .border(1.dp, if (on) CoucouColors.PrimaryBtn else CoucouColors.Line, CircleShape)
                                    .clickable(role = Role.Button) { onSetPermissionMode(mode) }
                                    .padding(vertical = 10.dp),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(
                                    text = label,
                                    color = if (on) CoucouColors.OnPrimaryBtn else CoucouColors.TextDim,
                                    fontSize = 12.sp,
                                    fontWeight = FontWeight.SemiBold,
                                )
                            }
                        }
                    }
                }
            }

            // Multimedia PC
            CoucouPanel {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column(Modifier.weight(1f)) {
                            Text(
                                text = nowPlaying.title.ifBlank { "Sin reproducción activa" },
                                color = CoucouColors.Text,
                                fontSize = 14.sp,
                                fontWeight = FontWeight.SemiBold,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(
                                text = "${nowPlaying.artist.ifBlank { "Multimedia PC" }} · Vol ${nowPlaying.volume}%",
                                color = CoucouColors.TextDim,
                                fontSize = 12.sp,
                            )
                        }
                        CoucouPillButton(label = "↻", onClick = onRefreshMedia)
                    }
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        CoucouPillButton(
                            label = "⏮",
                            onClick = { onPcCommand("media_control", mapOf("action" to "prev")) },
                            modifier = Modifier.weight(1f),
                        )
                        CoucouPillButton(
                            label = if (nowPlaying.playing) "Pausa" else "Play",
                            onClick = { onPcCommand("media_control", mapOf("action" to "play_pause")) },
                            modifier = Modifier.weight(1.4f),
                            primary = true,
                        )
                        CoucouPillButton(
                            label = "⏭",
                            onClick = { onPcCommand("media_control", mapOf("action" to "next")) },
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }

            // Portapapeles rápido PC
            CoucouPanel {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        text = "Enviar al portapapeles de PC",
                        color = CoucouColors.Text,
                        fontSize = 14.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                    OutlinedTextField(
                        value = clipDraft,
                        onValueChange = { clipDraft = it },
                        placeholder = { Text("Texto o comando para pegar en PC...", color = CoucouColors.TextFaint, fontSize = 13.sp) },
                        singleLine = true,
                        shape = RoundedCornerShape(14.dp),
                        modifier = Modifier.fillMaxWidth(),
                        colors = OutlinedTextFieldDefaults.colors(
                            focusedBorderColor = CoucouColors.MikuAccent,
                            unfocusedBorderColor = CoucouColors.Line,
                            focusedTextColor = CoucouColors.Text,
                            unfocusedTextColor = CoucouColors.Text,
                        ),
                    )
                    CoucouPillButton(
                        label = "Enviar a PC",
                        onClick = {
                            if (clipDraft.isNotBlank()) {
                                onPcCommand("clipboard_write", mapOf("text" to clipDraft))
                                clipDraft = ""
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                        enabled = clipDraft.isNotBlank(),
                    )
                }
            }

            Spacer(Modifier.height(84.dp))
        }
    }
}
