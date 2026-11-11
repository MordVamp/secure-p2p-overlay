package org.p2poverlay.android

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.fragment.app.FragmentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Chat
import androidx.compose.material.icons.outlined.FolderShared
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material.icons.outlined.SettingsInputAntenna
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.core.content.IntentCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import org.p2poverlay.android.ui.ChatsList
import org.p2poverlay.android.ui.Conversation
import org.p2poverlay.android.ui.FilesScreen
import org.p2poverlay.android.ui.InviteDialog
import org.p2poverlay.android.ui.LockScreen
import org.p2poverlay.android.ui.NetworkScreen
import org.p2poverlay.android.ui.NodeScreen
import org.p2poverlay.android.ui.P
import org.p2poverlay.android.ui.P2PTheme
import org.p2poverlay.android.ui.Pill

enum class Tab(val title: String) { Chats("Чаты"), Files("Файлы"), Network("Сеть"), Node("Узел") }

class MainActivity : FragmentActivity() {
    private val vm: AppViewModel by viewModels()
    private val invite = mutableStateOf<Invite?>(null)
    private val sharedUri = mutableStateOf<Uri?>(null)
    private val openPeer = mutableStateOf<String?>(null)
    private val forceTab = mutableStateOf<Tab?>(null)
    /** Блокировка: при запуске и после 60 с в фоне, если включена в настройках. */
    private val locked = mutableStateOf(false)
    private var stoppedAt = 0L

    private val scanner = registerForActivityResult(ScanContract()) { r ->
        val text = r.contents ?: return@registerForActivityResult
        Invite.parse(text)?.let { invite.value = it } ?: vm.toast("Это не ссылка p2p-overlay://")
    }

    private val notifPerm = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) notifPerm.launch(Manifest.permission.POST_NOTIFICATIONS)
        handle(intent)
        locked.value = vm.prefs.biometricLock
        setContent {
            P2PTheme(vm.themePref) {
                // значки строки состояния — по теме приложения, а не системы
                val dark = org.p2poverlay.android.ui.P.dark
                androidx.compose.runtime.LaunchedEffect(dark) {
                    val t = android.graphics.Color.TRANSPARENT
                    enableEdgeToEdge(
                        statusBarStyle = if (dark) androidx.activity.SystemBarStyle.dark(t) else androidx.activity.SystemBarStyle.light(t, t),
                        navigationBarStyle = if (dark) androidx.activity.SystemBarStyle.dark(t) else androidx.activity.SystemBarStyle.light(t, t),
                    )
                }
                if (locked.value) LockScreen(onUnlock = ::unlock) else Root()
            }
        }
        if (locked.value) unlock()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onStart() {
        super.onStart()
        visible = true
        if (vm.prefs.biometricLock && stoppedAt > 0 && System.currentTimeMillis() - stoppedAt > 60_000) {
            locked.value = true
            unlock()
        }
    }
    override fun onStop() { visible = false; stoppedAt = System.currentTimeMillis(); super.onStop() }

    private fun unlock() {
        val auth = if (Build.VERSION.SDK_INT >= 30)
            BiometricManager.Authenticators.BIOMETRIC_WEAK or BiometricManager.Authenticators.DEVICE_CREDENTIAL
        else BiometricManager.Authenticators.BIOMETRIC_WEAK
        if (BiometricManager.from(this).canAuthenticate(auth) != BiometricManager.BIOMETRIC_SUCCESS) {
            // на устройстве нет ни биометрии, ни блокировки экрана — блокировать нечем
            locked.value = false
            vm.toast("Блокировка недоступна: на устройстве не настроены отпечаток или PIN")
            return
        }
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle("P2P Overlay")
            .setSubtitle("Разблокируйте, чтобы открыть переписку")
            .setAllowedAuthenticators(auth)
            .apply { if (Build.VERSION.SDK_INT < 30) setNegativeButtonText("Отмена") }
            .build()
        BiometricPrompt(this, ContextCompat.getMainExecutor(this), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { locked.value = false }
        }).authenticate(info)
    }

    private fun handle(i: Intent?) {
        i ?: return
        when {
            i.action == Intent.ACTION_VIEW && i.data?.scheme == "p2p-overlay" -> invite.value = Invite.parse(i.data.toString())
            i.action == Intent.ACTION_SEND -> IntentCompat.getParcelableExtra(i, Intent.EXTRA_STREAM, Uri::class.java)?.let {
                sharedUri.value = it; forceTab.value = Tab.Files
            }
            i.hasExtra(EXTRA_PEER) -> openPeer.value = i.getStringExtra(EXTRA_PEER)
        }
    }

    private fun scan() = scanner.launch(
        ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setPrompt("QR с экрана компьютера")
            .setBeepEnabled(false).setOrientationLocked(false)
    )

    @OptIn(ExperimentalMaterial3Api::class)
    @androidx.compose.runtime.Composable
    private fun Root() {
        var tab by rememberSaveable { mutableStateOf(if (vm.prefs.bootstrap.isEmpty()) Tab.Node else Tab.Chats) }
        val run by vm.runState.collectAsState()
        val snack = remember { SnackbarHostState() }
        LaunchedEffect(Unit) { vm.toasts.collect { snack.showSnackbar(it) } }
        forceTab.value?.let { tab = it; forceTab.value = null }

        invite.value?.let { inv ->
            InviteDialog(inv, onDismiss = { invite.value = null }) { name ->
                vm.applyInvite(inv, name); invite.value = null; tab = Tab.Chats
            }
        }

        val peer = openPeer.value
        if (peer != null) {
            Conversation(vm, peer) { openPeer.value = null }
            return
        }
        val unread = vm.chats().sumOf { it.unread }
        Scaffold(
            containerColor = P.Rail,
            snackbarHost = { SnackbarHost(snack) },
            topBar = {
                TopAppBar(
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = P.Rail),
                    title = {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(if (tab == Tab.Chats) "P2P Overlay" else tab.title, fontWeight = FontWeight.SemiBold, fontSize = 22.sp)
                            when (run) {
                                is RunState.Running -> Pill("в сети · ${vm.status?.optJSONObject("routing")?.optInt("total_contacts") ?: 0} пиров", P.Green)
                                RunState.Starting -> Pill("запуск", P.Yellow)
                                is RunState.Failed -> Pill("ошибка узла", P.Red)
                                RunState.Stopped -> Pill("не в сети", P.Muted)
                            }
                        }
                    },
                )
            },
            bottomBar = {
                NavigationBar(containerColor = P.Side) {
                    Tab.entries.forEach { t ->
                        NavigationBarItem(
                            selected = tab == t, onClick = { tab = t },
                            label = { Text(t.title) },
                            colors = NavigationBarItemDefaults.colors(indicatorColor = P.AccentDim),
                            icon = {
                                BadgedBox(badge = { if (t == Tab.Chats && unread > 0) Badge { Text("$unread") } }) {
                                    Icon(when (t) {
                                        Tab.Chats -> Icons.AutoMirrored.Outlined.Chat
                                        Tab.Files -> Icons.Outlined.FolderShared
                                        Tab.Network -> Icons.Outlined.Hub
                                        Tab.Node -> Icons.Outlined.SettingsInputAntenna
                                    }, t.title)
                                }
                            },
                        )
                    }
                }
            },
        ) { pad ->
            androidx.compose.foundation.layout.Box(Modifier.padding(pad)) {
                when (tab) {
                    Tab.Chats -> ChatsList(vm, onOpen = { openPeer.value = it }, onScan = ::scan)
                    Tab.Files -> FilesScreen(vm, sharedUri.value) { sharedUri.value = null }
                    Tab.Network -> NetworkScreen(vm)
                    Tab.Node -> NodeScreen(vm, onScan = ::scan, onInvite = { invite.value = it })
                }
            }
        }
    }

    companion object {
        const val EXTRA_PEER = "peer"
        @Volatile var visible = false
    }
}
