package dev.miku.link

import dev.miku.link.ui.CoucouColors
import dev.miku.link.ui.CoucouFloatingBottomBar
import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.BatteryManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.speech.RecognizerIntent
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import dev.miku.link.ui.AgentFeedScreen
import dev.miku.link.ui.AlertsScreen
import dev.miku.link.ui.PairingScreen
import dev.miku.link.ui.PhoneLinkDashboard
import okhttp3.WebSocket
import org.json.JSONObject

private val MikuObsidianColors = darkColorScheme(
    primary = Color(0xFF39C5BB),
    onPrimary = Color(0xFF000000),
    secondary = CoucouColors.Text,
    background = CoucouColors.Bg,
    surface = CoucouColors.Panel,
    onBackground = CoucouColors.Text,
    onSurface = CoucouColors.Text,
)

class MainActivity : FragmentActivity() {
    private val pendingCards = mutableStateListOf<PermissionCardModel>()
    private val feedItems = mutableStateListOf<AgentFeedItem>()
    private val notifications = mutableStateListOf<LinkNotification>()
    private val offlineQueue = mutableStateListOf<String>()
    private val agentsList = mutableStateListOf(
        "miku" to "Miku",
        "antigravity" to "Antigravity",
        "github" to "GitHub",
    )

    private val activeTab = mutableStateOf(0)
    private val connected = mutableStateOf(false)
    private val wsConnected = mutableStateOf(false)
    private val killSwitchArmed = mutableStateOf(false)
    private val permissionMode = mutableStateOf("sandbox")
    private val selectedAgentId = mutableStateOf("miku")
    private val pairingEndpoint = mutableStateOf("http://127.0.0.1:47400")
    private val desktopName = mutableStateOf("Miku PC")
    private val transportMode = mutableStateOf("wifi")
    private val useUsbLoopback = mutableStateOf(false)
    private val gatewayStatusNote = mutableStateOf("Sin conectar · Escanea QR o activa USB ADB")
    private val actionNote = mutableStateOf("")
    private val tokensSaved = mutableStateOf(0L)
    private val activeModelLabel = mutableStateOf("deepseek-chat")
    private val busyChat = mutableStateOf(false)
    private val unreadCount = mutableStateOf(0)
    private val nowPlaying = mutableStateOf(NowPlayingState())
    private val hasCameraPerm = mutableStateOf(false)
    private val voiceSttEnabled = mutableStateOf(false)
    private val voiceSttEngine = mutableStateOf("off")
    private val showVoiceOptInDialog = mutableStateOf(false)
    private val voiceAutoSend = mutableStateOf(false)
    private val externalVoiceDraft = mutableStateOf("")

    private var activeCfg = PairingConfig()
    private var liveSocket: WebSocket? = null
    private val seenNotifIds = HashSet<String>()
    private val mainHandler = Handler(Looper.getMainLooper())
    private var heartbeatPosted = false

    private val heartbeatTask = object : Runnable {
        override fun run() {
            if (connected.value && activeCfg.token.isNotBlank()) {
                val (pct, chg) = readBatteryInfo()
                MikuLinkClient.sendHeartbeat(activeCfg, pct, chg)
                MikuLinkClient.fetchNowPlaying(activeCfg) { r -> r.onSuccess { nowPlaying.value = it } }
                if (!wsConnected.value) {
                    refreshNotifications(notifyOsOnNew = true)
                }
            }
            mainHandler.postDelayed(this, 30_000L)
        }
    }

    private val cameraPermLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        hasCameraPerm.value = granted
    }

    private val notifPermLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { }

    private val speechRecognizerLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK && result.data != null) {
            val matches = result.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)
            val spoken = matches?.firstOrNull()?.trim().orEmpty()
            if (spoken.isNotEmpty()) {
                if (voiceAutoSend.value) {
                    dispatchPrompt(spoken)
                } else {
                    externalVoiceDraft.value = spoken
                }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        MikuNotificationHelper.ensureChannel(this)
        hasCameraPerm.value = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.CAMERA,
        ) == PackageManager.PERMISSION_GRANTED

        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notifPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        }

        val savedCfg = loadSavedConfig()
        val deepLink = intent?.dataString
        if (!deepLink.isNullOrBlank()) {
            pairingEndpoint.value = deepLink
            activeTab.value = 3
        } else if (savedCfg != null && savedCfg.token.isNotBlank()) {
            activeCfg = savedCfg
            pairingEndpoint.value = savedCfg.baseUrl()
            desktopName.value = savedCfg.desktopName.ifBlank { "Miku PC" }
            useUsbLoopback.value = savedCfg.transport == "usb"
            transportMode.value = savedCfg.transport
        } else {
            activeTab.value = 3
        }

        val savedHistory = MikuHistoryStore.loadFeedHistory(this)
        if (savedHistory.isNotEmpty()) {
            feedItems.clear()
            feedItems.addAll(savedHistory)
        } else {
            feedItems.add(
                AgentFeedItem(
                    agent = "miku-link",
                    kind = "ready",
                    title = "Vincula tu PC desde Ajustes → Miku-Link (QR Wi-Fi o cable USB adb reverse).",
                    receivedAtMs = System.currentTimeMillis(),
                )
            )
            saveFeedHistory()
        }

        if (!deepLink.isNullOrBlank()) {
            connectOrRedeem(deepLink)
        } else if (activeCfg.token.isNotBlank()) {
            syncEverything(activeCfg)
        }

        setContent {
            MaterialTheme(colorScheme = MikuObsidianColors) {
                Surface(modifier = Modifier.fillMaxSize(), color = dev.miku.link.ui.CoucouColors.Bg) {
                    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
                        val isExpanded = maxWidth >= 600.dp

                        if (isExpanded) {
                            ExpandedTwoPanelScreen()
                        } else {
                            CompactPhoneScreen()
                        }

                        if (showVoiceOptInDialog.value) {
                            VoiceOptInDialog()
                        }
                    }
                }
            }
        }
    }

    @Composable
    private fun CompactPhoneScreen() {
        Scaffold(
            containerColor = dev.miku.link.ui.CoucouColors.Bg,
            topBar = {
                Surface(
                    color = dev.miku.link.ui.CoucouColors.Bg,
                    modifier = Modifier
                        .fillMaxWidth()
                        .statusBarsPadding(),
                ) {
                    Row(
                        modifier = Modifier.padding(horizontal = 18.dp, vertical = 12.dp),
                        horizontalArrangement = Arrangement.SpaceBetween,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Column {
                            Text(
                                text = "Miku",
                                color = dev.miku.link.ui.CoucouColors.Text,
                                fontWeight = FontWeight.SemiBold,
                                fontSize = 19.sp,
                            )
                            Text(
                                text = "${desktopName.value} · ${if (connected.value) transportMode.value.uppercase() else "Offline"}",
                                color = if (connected.value) dev.miku.link.ui.CoucouColors.MikuAccent else dev.miku.link.ui.CoucouColors.TextDim,
                                fontSize = 12.sp,
                            )
                        }
                        Text(
                            text = if (wsConnected.value) "LIVE" else if (connected.value) "SYNC" else "LOCAL",
                            color = if (connected.value) dev.miku.link.ui.CoucouColors.Online else dev.miku.link.ui.CoucouColors.TextFaint,
                            fontWeight = FontWeight.SemiBold,
                            fontSize = 11.5.sp,
                        )
                    }
                }
            },
            bottomBar = {
                CoucouFloatingBottomBar(
                    tabs = listOf("Inicio", "Chat", "Alertas", "QR"),
                    selectedIndex = activeTab.value,
                    onSelect = { idx ->
                        activeTab.value = idx
                        if (idx == 2 && connected.value) {
                            refreshNotifications(notifyOsOnNew = false)
                        }
                    },
                    alertDotIndex = if (unreadCount.value + pendingCards.size > 0) 2 else null,
                    modifier = Modifier.navigationBarsPadding(),
                )
            },
        ) { innerPadding ->
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(innerPadding)
                    .padding(12.dp)
                    .imePadding(),
            ) {
                when (activeTab.value) {
                    0 -> PhoneLinkDashboard(
                        connected = connected.value,
                        desktopName = desktopName.value,
                        transportLabel = transportMode.value.uppercase(),
                        permissionMode = permissionMode.value,
                        killSwitchArmed = killSwitchArmed.value,
                        tokensSaved = tokensSaved.value,
                        nowPlaying = nowPlaying.value,
                        actionNote = actionNote.value,
                        onKillSwitch = { handleKillSwitch() },
                        onSetPermissionMode = { mode -> handleSetPermissionMode(mode) },
                        onPcCommand = { action, extras -> handlePcCommand(action, extras) },
                        onRefreshMedia = {
                            MikuLinkClient.fetchNowPlaying(activeCfg) { r ->
                                r.onSuccess { nowPlaying.value = it }
                            }
                        },
                    )

                    1 -> AgentFeedScreen(
                        connected = connected.value,
                        tokensSaved = tokensSaved.value,
                        activeModel = activeModelLabel.value,
                        agents = agentsList,
                        selectedAgentId = selectedAgentId.value,
                        busyChat = busyChat.value,
                        pendingCardsCount = pendingCards.size,
                        feedItems = feedItems,
                        offlineQueuedCount = offlineQueue.size,
                        externalDraftPrompt = externalVoiceDraft.value,
                        voiceAutoSend = voiceAutoSend.value,
                        onToggleVoiceAutoSend = {
                            voiceAutoSend.value = !voiceAutoSend.value
                            saveConfig(activeCfg)
                        },
                        onSelectAgent = { selectedAgentId.value = it },
                        onOpenAlertsTab = { activeTab.value = 2 },
                        onSendPrompt = { prompt -> dispatchPrompt(prompt) },
                        onRemotePtt = { launchVoicePtt() },
                        onClearExternalDraft = { externalVoiceDraft.value = "" },
                    )

                    2 -> AlertsScreen(
                        pendingCards = pendingCards,
                        notifications = notifications,
                        unreadCount = unreadCount.value,
                        onAllowWithBiometrics = { card -> launchBiometricForCard(card) },
                        onDeny = { card -> submitCardDecision(card, DecisionOutcome.DENY, null) },
                        onRefreshAlerts = { refreshNotifications(notifyOsOnNew = false) },
                        onMarkAllRead = {
                            MikuLinkClient.ackNotifications(activeCfg, "all") { r ->
                                r.onSuccess { cnt ->
                                    unreadCount.value = cnt
                                    val updated = notifications.map { it.copy(read = true) }
                                    notifications.clear()
                                    notifications.addAll(updated)
                                }
                            }
                        },
                    )

                    else -> PairingScreen(
                        endpointText = pairingEndpoint.value,
                        connected = connected.value,
                        wsConnected = wsConnected.value,
                        desktopName = desktopName.value,
                        statusNote = gatewayStatusNote.value,
                        useUsbLoopback = useUsbLoopback.value,
                        hasCameraPermission = hasCameraPerm.value,
                        hasToken = activeCfg.token.isNotBlank(),
                        onEndpointChange = { pairingEndpoint.value = it },
                        onToggleUsbLoopback = { usb ->
                            useUsbLoopback.value = usb
                            if (usb && pairingEndpoint.value.startsWith("http://192.")) {
                                pairingEndpoint.value = "http://127.0.0.1:47400"
                            }
                        },
                        onRequestCameraPermission = {
                            cameraPermLauncher.launch(Manifest.permission.CAMERA)
                        },
                        onConnectOrRedeem = { raw -> connectOrRedeem(raw) },
                        onUnpairLocal = {
                            liveSocket?.close(1000, "unpair")
                            liveSocket = null
                            activeCfg = PairingConfig()
                            clearSavedConfig()
                            connected.value = false
                            wsConnected.value = false
                            gatewayStatusNote.value = "Desvinculado del PC"
                        },
                    )
                }
            }
        }
    }

    @Composable
    private fun ExpandedTwoPanelScreen() {
        Row(
            modifier = Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .navigationBarsPadding(),
        ) {
            NavigationRail(
                containerColor = Color(0xFF0B0D12),
                modifier = Modifier.fillMaxHeight(),
                header = {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier.padding(vertical = 12.dp),
                    ) {
                        Text(
                            text = "MIKU",
                            color = Color(0xFF39C5BB),
                            fontWeight = FontWeight.Bold,
                            fontSize = 14.sp,
                        )
                        Text(
                            text = if (wsConnected.value) "LIVE" else if (connected.value) "SYNC" else "LOCAL",
                            color = if (connected.value) Color(0xFF30D158) else Color(0xFF7D8596),
                            fontSize = 10.sp,
                            fontWeight = FontWeight.Bold,
                        )
                    }
                },
            ) {
                NavigationRailItem(
                    selected = activeTab.value == 0,
                    onClick = { activeTab.value = 0 },
                    icon = { Text("PC", fontWeight = FontWeight.Bold, fontSize = 12.sp) },
                    label = { Text("Control") },
                )
                NavigationRailItem(
                    selected = activeTab.value == 1,
                    onClick = { activeTab.value = 1 },
                    icon = { Text("AI", fontWeight = FontWeight.Bold, fontSize = 12.sp) },
                    label = { Text("Agentes") },
                )
                NavigationRailItem(
                    selected = activeTab.value == 2,
                    onClick = {
                        activeTab.value = 2
                        if (connected.value) refreshNotifications(notifyOsOnNew = false)
                    },
                    icon = {
                        val badge = unreadCount.value + pendingCards.size
                        Text(
                            if (badge > 0) "$badge" else "!",
                            fontWeight = FontWeight.Bold,
                            fontSize = 12.sp,
                            color = if (badge > 0) Color(0xFF39C5BB) else Color(0xFF9EA4B3),
                        )
                    },
                    label = { Text("Alertas") },
                )
                NavigationRailItem(
                    selected = activeTab.value == 3,
                    onClick = { activeTab.value = 3 },
                    icon = { Text("QR", fontWeight = FontWeight.Bold, fontSize = 12.sp) },
                    label = { Text("Vincular") },
                )
            }

            Row(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(12.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                // Left Panel: Navigation & Feed List Panel
                Surface(
                    shape = RoundedCornerShape(14.dp),
                    color = Color(0xFF0E1117),
                    border = BorderStroke(1.dp, Color(0xFF202532)),
                    modifier = Modifier
                        .weight(0.38f)
                        .fillMaxHeight(),
                ) {
                    Column(
                        modifier = Modifier
                            .fillMaxSize()
                            .padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column {
                                Text(
                                    text = desktopName.value,
                                    color = Color(0xFFF5F5F7),
                                    fontWeight = FontWeight.Bold,
                                    fontSize = 15.sp,
                                )
                                Text(
                                    text = if (connected.value) "${transportMode.value.uppercase()} · ${activeModelLabel.value}" else "Sin conexión",
                                    color = if (connected.value) Color(0xFF39C5BB) else Color(0xFF9EA4B3),
                                    fontSize = 11.sp,
                                )
                            }
                            FilterChip(
                                selected = voiceAutoSend.value,
                                onClick = {
                                    voiceAutoSend.value = !voiceAutoSend.value
                                    saveConfig(activeCfg)
                                },
                                label = { Text(if (voiceAutoSend.value) "Voz: Auto" else "Voz: Manual", fontSize = 11.sp) },
                            )
                        }

                        Text(
                            text = "Agentes Disponibles",
                            color = Color(0xFF9EA4B3),
                            fontSize = 12.sp,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .horizontalScroll(rememberScrollState()),
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            agentsList.forEach { (id, name) ->
                                FilterChip(
                                    selected = id == selectedAgentId.value,
                                    onClick = { selectedAgentId.value = id },
                                    label = { Text(name, fontSize = 11.5.sp) },
                                )
                            }
                        }

                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.SpaceBetween,
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                text = "Historial y Feed",
                                color = Color(0xFFF5F5F7),
                                fontWeight = FontWeight.SemiBold,
                                fontSize = 13.sp,
                            )
                            Text(
                                text = "${feedItems.size} eventos",
                                color = Color(0xFF39C5BB),
                                fontSize = 11.sp,
                            )
                        }

                        LazyColumn(
                            modifier = Modifier.weight(1f),
                            verticalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            items(feedItems) { item ->
                                val tagColor = when (item.kind) {
                                    "error", "emergency", "pc_only_blocked" -> Color(0xFFFF6B6B)
                                    "0-tokens · local" -> Color(0xFF30D158)
                                    "prompt" -> Color(0xFF9EA4B3)
                                    else -> Color(0xFF39C5BB)
                                }
                                Surface(
                                    shape = RoundedCornerShape(10.dp),
                                    color = Color(0xFF141822),
                                    border = BorderStroke(1.dp, Color(0xFF262C3A)),
                                    modifier = Modifier.fillMaxWidth(),
                                ) {
                                    Column(modifier = Modifier.padding(8.dp)) {
                                        Text(
                                            text = "${item.agent} · ${item.kind}",
                                            color = tagColor,
                                            fontSize = 10.5.sp,
                                            fontWeight = FontWeight.Bold,
                                        )
                                        Text(
                                            text = item.title,
                                            color = Color(0xFFF5F5F7),
                                            fontSize = 12.sp,
                                            maxLines = 2,
                                        )
                                    }
                                }
                            }
                        }
                    }
                }

                // Right Panel: Detail / Chat Panel
                Box(
                    modifier = Modifier
                        .weight(0.62f)
                        .fillMaxHeight()
                        .imePadding(),
                ) {
                    when (activeTab.value) {
                        0 -> PhoneLinkDashboard(
                            connected = connected.value,
                            desktopName = desktopName.value,
                            transportLabel = transportMode.value.uppercase(),
                            permissionMode = permissionMode.value,
                            killSwitchArmed = killSwitchArmed.value,
                            tokensSaved = tokensSaved.value,
                            nowPlaying = nowPlaying.value,
                            actionNote = actionNote.value,
                            onKillSwitch = { handleKillSwitch() },
                            onSetPermissionMode = { mode -> handleSetPermissionMode(mode) },
                            onPcCommand = { action, extras -> handlePcCommand(action, extras) },
                            onRefreshMedia = {
                                MikuLinkClient.fetchNowPlaying(activeCfg) { r ->
                                    r.onSuccess { nowPlaying.value = it }
                                }
                            },
                        )

                        1 -> AgentFeedScreen(
                            connected = connected.value,
                            tokensSaved = tokensSaved.value,
                            activeModel = activeModelLabel.value,
                            agents = agentsList,
                            selectedAgentId = selectedAgentId.value,
                            busyChat = busyChat.value,
                            pendingCardsCount = pendingCards.size,
                            feedItems = feedItems,
                            offlineQueuedCount = offlineQueue.size,
                            externalDraftPrompt = externalVoiceDraft.value,
                            voiceAutoSend = voiceAutoSend.value,
                            onToggleVoiceAutoSend = {
                                voiceAutoSend.value = !voiceAutoSend.value
                                saveConfig(activeCfg)
                            },
                            onSelectAgent = { selectedAgentId.value = it },
                            onOpenAlertsTab = { activeTab.value = 2 },
                            onSendPrompt = { prompt -> dispatchPrompt(prompt) },
                            onRemotePtt = { launchVoicePtt() },
                            onClearExternalDraft = { externalVoiceDraft.value = "" },
                        )

                        2 -> AlertsScreen(
                            pendingCards = pendingCards,
                            notifications = notifications,
                            unreadCount = unreadCount.value,
                            onAllowWithBiometrics = { card -> launchBiometricForCard(card) },
                            onDeny = { card -> submitCardDecision(card, DecisionOutcome.DENY, null) },
                            onRefreshAlerts = { refreshNotifications(notifyOsOnNew = false) },
                            onMarkAllRead = {
                                MikuLinkClient.ackNotifications(activeCfg, "all") { r ->
                                    r.onSuccess { cnt ->
                                        unreadCount.value = cnt
                                        val updated = notifications.map { it.copy(read = true) }
                                        notifications.clear()
                                        notifications.addAll(updated)
                                    }
                                }
                            },
                        )

                        else -> PairingScreen(
                            endpointText = pairingEndpoint.value,
                            connected = connected.value,
                            wsConnected = wsConnected.value,
                            desktopName = desktopName.value,
                            statusNote = gatewayStatusNote.value,
                            useUsbLoopback = useUsbLoopback.value,
                            hasCameraPermission = hasCameraPerm.value,
                            hasToken = activeCfg.token.isNotBlank(),
                            onEndpointChange = { pairingEndpoint.value = it },
                            onToggleUsbLoopback = { usb ->
                                useUsbLoopback.value = usb
                                if (usb && pairingEndpoint.value.startsWith("http://192.")) {
                                    pairingEndpoint.value = "http://127.0.0.1:47400"
                                }
                            },
                            onRequestCameraPermission = {
                                cameraPermLauncher.launch(Manifest.permission.CAMERA)
                            },
                            onConnectOrRedeem = { raw -> connectOrRedeem(raw) },
                            onUnpairLocal = {
                                liveSocket?.close(1000, "unpair")
                                liveSocket = null
                                activeCfg = PairingConfig()
                                clearSavedConfig()
                                connected.value = false
                                wsConnected.value = false
                                gatewayStatusNote.value = "Desvinculado del PC"
                            },
                        )
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        if (!heartbeatPosted) {
            heartbeatPosted = true
            mainHandler.postDelayed(heartbeatTask, 15_000L)
        }
    }

    override fun onDestroy() {
        mainHandler.removeCallbacks(heartbeatTask)
        liveSocket?.close(1001, "destroy")
        super.onDestroy()
    }

    private fun launchVoicePtt() {
        if (!MikuVoiceOptInStore.canLaunchSpeechCapture(voiceSttEnabled.value, voiceSttEngine.value)) {
            showVoiceOptInDialog.value = true
            return
        }
        launchAndroidSpeechRecognizerIntent()
    }

    private fun launchAndroidSpeechRecognizerIntent() {
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_PROMPT, "Habla para dictar a ${selectedAgentId.value}...")
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 3)
        }
        try {
            speechRecognizerLauncher.launch(intent)
        } catch (e: Exception) {
            actionNote.value = "Reconocimiento de voz no disponible"
            feedItems.add(
                0,
                AgentFeedItem(
                    agent = "voz",
                    kind = "error",
                    title = "Servicio de voz no disponible en el sistema: ${e.message}",
                    receivedAtMs = System.currentTimeMillis(),
                ),
            )
            saveFeedHistory()
        }
    }

    @Composable
    private fun VoiceOptInDialog() {
        AlertDialog(
            onDismissRequest = { showVoiceOptInDialog.value = false },
            containerColor = Color(0xFF0E1117),
            titleContentColor = Color(0xFFF5F5F7),
            textContentColor = Color(0xFF9EA4B3),
            title = { Text("Reconocimiento de Voz (Opt-In)", fontWeight = FontWeight.Bold) },
            text = {
                Text(
                    "El micrófono y dictado por voz están desactivados por defecto para proteger tu privacidad. " +
                        "Elige si deseas habilitar el reconocimiento nativo de Android (RecognizerIntent) para este dispositivo.",
                    fontSize = 13.5.sp,
                )
            },
            confirmButton = {
                Button(
                    onClick = {
                        voiceSttEnabled.value = true
                        voiceSttEngine.value = "android_recognizer"
                        showVoiceOptInDialog.value = false
                        saveVoicePrefs()
                        launchAndroidSpeechRecognizerIntent()
                    },
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF39C5BB)),
                ) {
                    Text("Activar Voz Android", color = Color(0xFF050609), fontWeight = FontWeight.Bold)
                }
            },
            dismissButton = {
                TextButton(
                    onClick = {
                        voiceSttEnabled.value = false
                        voiceSttEngine.value = "off"
                        showVoiceOptInDialog.value = false
                        saveVoicePrefs()
                        actionNote.value = "Voz desactivada (solo texto manual)"
                    },
                ) {
                    Text("No usar voz", color = Color(0xFF9EA4B3))
                }
            },
        )
    }

    private fun saveVoicePrefs() {
        MikuVoiceOptInStore.save(
            this,
            VoiceOptInPrefs(
                enabled = voiceSttEnabled.value,
                engine = voiceSttEngine.value,
                autoSend = voiceAutoSend.value,
            ),
        )
    }

    private fun saveFeedHistory() {
        MikuHistoryStore.saveFeedHistory(this, feedItems)
    }

    private fun deviceIdentity(): Pair<String, String> {
        val model = "${Build.MANUFACTURER.orEmpty()} ${Build.MODEL.orEmpty()}".trim().ifBlank { "Android Phone" }
        val prefs = getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE)
        var devId = prefs.getString("device_id", "").orEmpty()
        if (devId.isBlank()) {
            devId = "android-${Build.MODEL.orEmpty().lowercase().replace(Regex("[^a-z0-9]"), "").take(12).ifBlank { "phone" }}-${(System.currentTimeMillis() % 100000)}"
            prefs.edit().putString("device_id", devId).apply()
        }
        return devId to model
    }

    private fun connectOrRedeem(rawInput: String) {
        val (devId, devName) = deviceIdentity()
        val parsed = MikuLinkClient.parsePairingInput(rawInput, useUsbLoopback.value).copy(
            deviceId = devId,
            deviceName = devName,
        )
        val merged = if (parsed.token.isBlank() && !parsed.hasQrChallenge() && activeCfg.token.isNotBlank()) {
            parsed.copy(token = activeCfg.token)
        } else {
            parsed
        }

        if (merged.hasQrChallenge()) {
            gatewayStatusNote.value = "Canjeando QR con ${merged.baseUrl()}..."
            MikuLinkClient.redeemQrPairing(merged) { res ->
                res.onSuccess { pairedCfg ->
                    activeCfg = pairedCfg
                    saveConfig(pairedCfg)
                    pairingEndpoint.value = pairedCfg.baseUrl()
                    desktopName.value = pairedCfg.desktopName.ifBlank { "Miku PC" }
                    transportMode.value = pairedCfg.transport
                    actionNote.value = "Vinculado con ${desktopName.value}"
                    syncEverything(pairedCfg)
                    activeTab.value = 0
                }.onFailure { err ->
                    connected.value = false
                    gatewayStatusNote.value = "Error QR: ${err.message?.take(64)}"
                }
            }
        } else {
            activeCfg = merged
            syncEverything(merged)
        }
    }

    private fun syncEverything(cfg: PairingConfig) {
        gatewayStatusNote.value = "Sincronizando con ${cfg.baseUrl()}..."
        MikuLinkClient.fetchSnapshot(cfg) { result ->
            result.onSuccess { snap ->
                connected.value = true
                killSwitchArmed.value = snap.killSwitch
                permissionMode.value = snap.permissionMode
                tokensSaved.value = snap.estimatedTokensSaved
                activeModelLabel.value = "${snap.provider}/${snap.model}"
                gatewayStatusNote.value = "Conectado · ${snap.permissionMode} · ${snap.agents.size} agentes"
                saveConfig(cfg)
                if (snap.agents.isNotEmpty()) {
                    agentsList.clear()
                    agentsList.addAll(snap.agents)
                    if (agentsList.none { it.first == selectedAgentId.value }) {
                        selectedAgentId.value = agentsList.first().first
                    }
                }
                val (pct, chg) = readBatteryInfo()
                MikuLinkClient.sendHeartbeat(cfg, pct, chg)
                MikuLinkClient.fetchLinkStatus(cfg) { s ->
                    s.onSuccess { st ->
                        desktopName.value = st.desktopName.ifBlank { desktopName.value }
                        unreadCount.value = st.unreadNotifications
                    }
                }
                MikuLinkClient.fetchNowPlaying(cfg) { r -> r.onSuccess { nowPlaying.value = it } }
                refreshNotifications(notifyOsOnNew = false)
                openLiveSocket(cfg)

                if (offlineQueue.isNotEmpty()) {
                    val queued = offlineQueue.toList()
                    offlineQueue.clear()
                    queued.forEach { dispatchPrompt(it) }
                }
            }.onFailure { err ->
                connected.value = false
                gatewayStatusNote.value = "Sin conexión (${err.message?.take(56) ?: "revisa IP / adb reverse"})"
            }
        }
    }

    private fun openLiveSocket(cfg: PairingConfig) {
        liveSocket?.close(1000, "reconnect")
        liveSocket = MikuLinkClient.connectWebSocket(
            cfg = cfg,
            onEvent = { ev -> handleGatewayEvent(ev) },
            onStateChange = { ok -> wsConnected.value = ok },
        )
    }

    private fun handleGatewayEvent(ev: JSONObject) {
        when (ev.optString("type")) {
            "link_notification" -> {
                val nObj = ev.optJSONObject("notification")
                if (nObj != null) {
                    val item = LinkNotification(
                        id = nObj.optString("id", "ln-${System.currentTimeMillis()}"),
                        tsMs = nObj.optLong("ts_ms", System.currentTimeMillis()),
                        level = nObj.optString("level", "info"),
                        category = nObj.optString("category", "general"),
                        title = nObj.optString("title", "Miku PC"),
                        body = nObj.optString("body", ""),
                        source = nObj.optString("source", "pc"),
                        read = nObj.optBoolean("read", false),
                    )
                    if (notifications.none { it.id == item.id }) {
                        notifications.add(0, item)
                        if (seenNotifIds.add(item.id) && !item.read) {
                            MikuNotificationHelper.showAlert(
                                this,
                                item.id.hashCode(),
                                item.title,
                                item.body,
                                highPriority = item.level == "error",
                            )
                        }
                    }
                }
                if (ev.has("unread_count")) {
                    unreadCount.value = ev.optInt("unread_count", unreadCount.value)
                }
            }

            "permission_ask" -> {
                val reqHash = ev.optString("req_hash", "").trim()
                if (reqHash.isNotEmpty() && pendingCards.none { it.reqHash == reqHash }) {
                    val card = PermissionCardModel(
                        reqHash = reqHash,
                        agent = ev.optString("agent", "miku"),
                        capability = ev.optString("capability", "fs_write"),
                        target = ev.optString("target", ""),
                        args = ev.optString("args", ""),
                        ttlSec = ev.optInt("ttl_sec", 45),
                        receivedAtMs = System.currentTimeMillis(),
                        pcOnly = ev.optBoolean("pc_only", false),
                    )
                    pendingCards.add(0, card)
                    MikuNotificationHelper.showAlert(
                        this,
                        reqHash.hashCode(),
                        "Aprobación requerida (${card.agent})",
                        "${card.capability}: ${card.target}",
                        highPriority = true,
                    )
                }
            }

            "state_sync" -> {
                val state = ev.optJSONObject("state") ?: ev
                if (state.has("kill_switch")) killSwitchArmed.value = state.optBoolean("kill_switch", killSwitchArmed.value)
                if (state.has("permission_mode")) permissionMode.value = state.optString("permission_mode", permissionMode.value)
            }

            "link_unpaired" -> {
                connected.value = false
                wsConnected.value = false
                gatewayStatusNote.value = "Desvinculado desde PC"
            }
        }
    }

    private fun refreshNotifications(notifyOsOnNew: Boolean) {
        if (activeCfg.token.isBlank()) return
        MikuLinkClient.fetchNotifications(activeCfg) { res ->
            res.onSuccess { (list, unread) ->
                val fresh = list.filter { !it.read && seenNotifIds.add(it.id) }
                notifications.clear()
                notifications.addAll(list)
                unreadCount.value = unread
                if (notifyOsOnNew) {
                    fresh.firstOrNull()?.let { latest ->
                        MikuNotificationHelper.showAlert(
                            this,
                            latest.id.hashCode(),
                            latest.title,
                            latest.body,
                            highPriority = latest.level == "error",
                        )
                    }
                }
            }
        }
    }

    private fun handleKillSwitch() {
        val targetArmed = !killSwitchArmed.value
        pendingCards.clear()
        if (connected.value) {
            MikuLinkClient.toggleKillSwitch(activeCfg, targetArmed) { res ->
                res.onSuccess { armedNow ->
                    killSwitchArmed.value = armedNow
                    actionNote.value = if (armedNow) "Kill Switch ARMADO en PC" else "Kill Switch liberado"
                }.onFailure { err ->
                    actionNote.value = "Error Kill Switch: ${err.message}"
                }
            }
        } else {
            killSwitchArmed.value = targetArmed
            actionNote.value = "Kill Switch local ${if (targetArmed) "activo" else "liberado"}"
        }
    }

    private fun handleSetPermissionMode(mode: String) {
        if (!connected.value) {
            actionNote.value = "Conecta el PC primero en Vincular"
            return
        }
        MikuLinkClient.sendCommand(activeCfg, "set_permission_mode", mapOf("mode" to mode)) { res ->
            res.onSuccess {
                permissionMode.value = mode
                actionNote.value = "Modo Gatekeeper actualizado: $mode"
            }.onFailure { err ->
                actionNote.value = "No se pudo cambiar modo: ${err.message}"
            }
        }
    }

    private fun handlePcCommand(action: String, extras: Map<String, Any>) {
        if (!connected.value) {
            actionNote.value = "Sin conexión con PC"
            return
        }
        MikuLinkClient.sendCommand(activeCfg, action, extras) { res ->
            res.onSuccess { obj ->
                actionNote.value = "Ejecutado en PC: $action"
                if (action.startsWith("media_") || action.startsWith("volume_")) {
                    MikuLinkClient.fetchNowPlaying(activeCfg) { r -> r.onSuccess { nowPlaying.value = it } }
                }
                if (action == "set_permission_mode") {
                    permissionMode.value = obj.optString("permission_mode", permissionMode.value)
                }
            }.onFailure { err ->
                actionNote.value = "Error $action: ${err.message}"
            }
        }
    }

    private fun dispatchPrompt(prompt: String) {
        MikuMobileBridge.assertNoProviderApiKeys(prompt)
        val agentId = selectedAgentId.value.ifBlank { "miku" }
        if (!connected.value) {
            offlineQueue.add(prompt)
            feedItems.add(0, AgentFeedItem("móvil (cola)", "queued", prompt, System.currentTimeMillis()))
            saveFeedHistory()
            return
        }
        busyChat.value = true
        feedItems.add(0, AgentFeedItem("tú -> $agentId", "prompt", prompt, System.currentTimeMillis()))
        saveFeedHistory()
        MikuLinkClient.sendChat(activeCfg, prompt, agentId) { res ->
            busyChat.value = false
            if (res.ok && res.replyText.isNotBlank()) {
                if (res.tokensSaved > 0L) {
                    tokensSaved.value = tokensSaved.value + res.tokensSaved
                }
                val tag = if (res.localRoute) "0-tokens · local" else "reply"
                feedItems.add(0, AgentFeedItem(agentId, tag, res.replyText, System.currentTimeMillis()))
            } else {
                feedItems.add(0, AgentFeedItem(agentId, "error", res.error.ifBlank { "Sin respuesta del Gateway" }, System.currentTimeMillis()))
            }
            saveFeedHistory()
        }
    }

    private fun launchBiometricForCard(card: PermissionCardModel) {
        if (card.pcOnly) {
            actionNote.value = "Acción PC-only: debe ser aprobada en el Dynamic Island del PC"
            feedItems.add(
                0,
                AgentFeedItem(
                    agent = card.agent,
                    kind = "pc_only_blocked",
                    title = "Acción de alto riesgo '${card.capability}' marcada como pc_only. Aprobación móvil bloqueada por seguridad. Aprueba directamente en el PC.",
                    receivedAtMs = System.currentTimeMillis(),
                ),
            )
            saveFeedHistory()
            return
        }
        val executor = ContextCompat.getMainExecutor(this)
        val prompt = BiometricPrompt(
            this,
            executor,
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val proof = BiometricProofModel(
                        verified = true,
                        authMethod = "androidx_biometric_strong",
                        boundReqHash = card.reqHash,
                        verifiedAtMs = System.currentTimeMillis(),
                    )
                    submitCardDecision(card, DecisionOutcome.ALLOW, proof)
                }

                override fun onAuthenticationFailed() {
                    // Fail-closed: never issue Allow on failed biometric match.
                }
            },
        )
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle("Aprobar ${card.agent} (${card.capability})")
            .setSubtitle("req_hash ${card.reqHash.take(20)}...")
            .setNegativeButtonText("Denegar")
            .setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG)
            .build()
        prompt.authenticate(info)
    }

    private fun submitCardDecision(
        card: PermissionCardModel,
        requested: DecisionOutcome,
        proof: BiometricProofModel?,
    ) {
        if (card.pcOnly && requested == DecisionOutcome.ALLOW) {
            actionNote.value = "Acción PC-only: no puede aprobarse en el móvil"
            feedItems.add(
                0,
                AgentFeedItem(
                    agent = card.agent,
                    kind = "pc_only_blocked",
                    title = "Acción '${card.capability}' es pc_only. No puede ser aprobada desde el dispositivo móvil.",
                    receivedAtMs = System.currentTimeMillis(),
                ),
            )
            saveFeedHistory()
            return
        }

        val outcome = MikuMobileBridge.evaluateDecision(
            card = card,
            requested = requested,
            biometric = proof,
            nowMs = System.currentTimeMillis(),
        )
        pendingCards.removeAll { it.reqHash == card.reqHash }
        feedItems.add(
            0,
            AgentFeedItem(
                agent = card.agent,
                kind = "permission_${outcome.name.lowercase()}",
                title = "${card.capability} sobre ${card.target}",
                receivedAtMs = System.currentTimeMillis(),
            ),
        )
        saveFeedHistory()

        // Transmit decision back to PC gateway over WebSocket
        val wsPayload = MikuLinkClient.buildPermissionDecisionJson(card, outcome, proof)
        val wsSent = liveSocket?.send(wsPayload.toString()) == true
        if (!wsSent && connected.value) {
            // Fallback over HTTP link command
            MikuLinkClient.sendPermissionDecision(activeCfg, card, outcome, proof) { res ->
                res.onFailure { err ->
                    actionNote.value = "Error enviando decisión al PC: ${err.message}"
                }
            }
        }
    }

    private fun readBatteryInfo(): Pair<Int, Boolean> {
        val intent = registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = intent?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = intent?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
        val status = intent?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
        val pct = if (level >= 0 && scale > 0) ((level * 100f) / scale).toInt().coerceIn(0, 100) else 100
        return pct to charging
    }

    private fun saveConfig(cfg: PairingConfig) {
        if (cfg.token.isNotBlank()) MikuMobileBridge.assertNoProviderApiKeys(cfg.token)
        getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE).edit()
            .putString("host", cfg.host)
            .putInt("port", cfg.port)
            .putString("token", cfg.token)
            .putString("transport", cfg.transport)
            .putString("desktop_name", cfg.desktopName)
            .putBoolean(MikuVoiceOptInStore.KEY_VOICE_STT_ENABLED, voiceSttEnabled.value)
            .putString(
                MikuVoiceOptInStore.KEY_VOICE_STT_ENGINE,
                MikuVoiceOptInStore.normalizeEngine(voiceSttEnabled.value, voiceSttEngine.value),
            )
            .putBoolean(MikuVoiceOptInStore.KEY_VOICE_AUTO_SEND, voiceAutoSend.value)
            .apply()
    }

    private fun loadSavedConfig(): PairingConfig? {
        val prefs = getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE)
        val voicePrefs = MikuVoiceOptInStore.load(this)
        voiceSttEnabled.value = voicePrefs.enabled
        voiceSttEngine.value = voicePrefs.engine
        voiceAutoSend.value = voicePrefs.autoSend
        val token = prefs.getString("token", "").orEmpty()
        if (token.isBlank()) return null
        val (devId, devName) = deviceIdentity()
        return PairingConfig(
            host = prefs.getString("host", "127.0.0.1").orEmpty().ifBlank { "127.0.0.1" },
            port = prefs.getInt("port", 47400),
            token = token,
            transport = prefs.getString("transport", "wifi").orEmpty().ifBlank { "wifi" },
            desktopName = prefs.getString("desktop_name", "Miku PC").orEmpty(),
            deviceId = devId,
            deviceName = devName,
        )
    }

    private fun clearSavedConfig() {
        getSharedPreferences("miku_link_prefs", Context.MODE_PRIVATE).edit()
            .remove("token")
            .remove("desktop_name")
            .apply()
    }
}
