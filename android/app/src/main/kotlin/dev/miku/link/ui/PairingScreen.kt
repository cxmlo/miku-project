package dev.miku.link.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

@Composable
fun PairingScreen(
    endpointText: String,
    connected: Boolean,
    wsConnected: Boolean,
    desktopName: String,
    statusNote: String,
    useUsbLoopback: Boolean,
    hasCameraPermission: Boolean,
    hasToken: Boolean,
    onEndpointChange: (String) -> Unit,
    onToggleUsbLoopback: (Boolean) -> Unit,
    onRequestCameraPermission: () -> Unit,
    onConnectOrRedeem: (String) -> Unit,
    onUnpairLocal: () -> Unit,
) {
    var showScanner by remember { mutableStateOf(false) }
    var pasteOpen by remember { mutableStateOf(false) }

    if (showScanner) {
        Dialog(
            onDismissRequest = { showScanner = false },
            properties = DialogProperties(
                usePlatformDefaultWidth = false,
                decorFitsSystemWindows = false,
            ),
        ) {
            QrScannerScreen(
                onScanned = { qr ->
                    showScanner = false
                    onEndpointChange(qr)
                    onConnectOrRedeem(qr)
                },
                onClose = { showScanner = false },
                onPasteLink = {
                    showScanner = false
                    pasteOpen = true
                },
            )
        }
    }

    val statusColor = when {
        connected && wsConnected -> CoucouColors.Online
        connected -> CoucouColors.MikuAccent
        else -> CoucouColors.TextFaint
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(CoucouColors.Bg)
            .windowInsetsPadding(WindowInsets.statusBars)
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 24.dp, vertical = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(28.dp))

        Box(
            modifier = Modifier
                .size(112.dp)
                .clip(CircleShape)
                .background(CoucouColors.Hero)
                .border(1.dp, CoucouColors.Line, CircleShape)
                .drawBehind { drawCoucouWash(CoucouColors.MikuAccent, 0.45f) },
            contentAlignment = Alignment.Center,
        ) {
            Box(
                modifier = Modifier
                    .size(46.dp)
                    .clip(CircleShape)
                    .background(statusColor.copy(alpha = 0.22f))
                    .border(2.dp, statusColor, CircleShape),
            )
        }

        Spacer(Modifier.height(24.dp))

        Text(
            text = if (connected) "Conectado a $desktopName" else "Vincular con Miku PC",
            color = CoucouColors.Text,
            fontSize = 24.sp,
            fontWeight = FontWeight.Bold,
            textAlign = TextAlign.Center,
        )

        Spacer(Modifier.height(8.dp))

        Text(
            text = statusNote.ifBlank {
                "Escanea el código QR en Miku PC → Ajustes → Miku-Link para controlar Antigravity y GitHub desde tu teléfono."
            },
            color = CoucouColors.TextDim,
            fontSize = 14.sp,
            lineHeight = 20.sp,
            textAlign = TextAlign.Center,
        )

        Spacer(Modifier.height(28.dp))

        CoucouPillButton(
            label = if (connected) "Re-escanear QR" else "Escanear código QR",
            onClick = { showScanner = true },
            modifier = Modifier.fillMaxWidth(),
            primary = true,
        )

        Spacer(Modifier.height(12.dp))

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            CoucouPillButton(
                label = "Pegar enlace",
                onClick = { pasteOpen = true },
                modifier = Modifier.weight(1f),
            )
            CoucouPillButton(
                label = if (useUsbLoopback) "Modo USB ✓" else "Modo Wi-Fi",
                onClick = { onToggleUsbLoopback(!useUsbLoopback) },
                modifier = Modifier.weight(1f),
            )
        }

        if (connected || hasToken) {
            Spacer(Modifier.height(16.dp))
            CoucouPanel {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(16.dp),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            text = desktopName.ifBlank { "Miku PC" },
                            color = CoucouColors.Text,
                            fontSize = 15.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(
                            text = if (wsConnected) "Enlace en vivo activo" else "Esperando reconexión",
                            color = CoucouColors.TextDim,
                            fontSize = 12.sp,
                        )
                    }
                    CoucouPillButton(
                        label = "Desvincular",
                        onClick = onUnpairLocal,
                        danger = true,
                    )
                }
            }
        }

        Spacer(Modifier.height(28.dp))
    }

    if (pasteOpen) {
        val clipboard = LocalClipboardManager.current
        var draft by remember(endpointText) { mutableStateOf(endpointText) }
        AlertDialog(
            onDismissRequest = { pasteOpen = false },
            containerColor = CoucouColors.Panel,
            title = {
                Text("Pegar enlace o IP", color = CoucouColors.Text, fontWeight = FontWeight.SemiBold)
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        text = "Pega `miku-link://pair?...` o `http://192.168.x.x:47400`.",
                        color = CoucouColors.TextDim,
                        fontSize = 13.sp,
                    )
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        singleLine = true,
                        shape = RoundedCornerShape(14.dp),
                        colors = OutlinedTextFieldDefaults.colors(
                            focusedBorderColor = CoucouColors.MikuAccent,
                            unfocusedBorderColor = CoucouColors.Line,
                            focusedTextColor = CoucouColors.Text,
                            unfocusedTextColor = CoucouColors.Text,
                        ),
                    )
                    CoucouPillButton(
                        label = "Pegar del portapapeles",
                        onClick = {
                            clipboard.getText()?.text?.let { draft = it.trim() }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                CoucouPillButton(
                    label = "Conectar",
                    onClick = {
                        pasteOpen = false
                        onEndpointChange(draft)
                        onConnectOrRedeem(draft)
                    },
                    primary = true,
                )
            },
            dismissButton = {
                CoucouPillButton(
                    label = "Cancelar",
                    onClick = { pasteOpen = false },
                )
            },
        )
    }
}
