package dev.murmur.ui

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.murmur.core.Secrets
import dev.murmur.ui.theme.MurmurTheme
import dev.murmur.ui.widgets.GrantGate
import dev.murmur.ui.widgets.PairingScreen
import dev.murmur.ui.widgets.homeWidgets
import dev.murmur.ui.widgets.rememberGrants

class MainActivity : ComponentActivity() {

    // The device list/ping poller runs only while this is on screen.
    override fun onResume() {
        super.onResume()
        dev.murmur.MurmurService.uiVisible.value = true
    }

    override fun onPause() {
        dev.murmur.MurmurService.uiVisible.value = false
        super.onPause()
    }

    @OptIn(ExperimentalMaterial3Api::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        // Publishes settledFlow before the first frame decides which
        // world to show.
        val secrets = Secrets(this)
        // Pairing's celebration normally starts the service — but a
        // process death in the seconds between secrets-save and the
        // burst's last frame leaves the phone paired-yet-silent. The
        // start is idempotent, so kick it unconditionally.
        if (secrets.deviceId != null && dev.murmur.MurmurService.instance == null) {
            dev.murmur.MurmurService.start(this)
        }
        setContent {
            MurmurTheme {
                val grants = rememberGrants()
                val settled by Secrets.settledFlow.collectAsState()
                when {
                    // No access, no app — the gate asks at entry and
                    // offers grant-or-quit, nothing else.
                    !grants.all -> GrantGate(grants)
                    !settled -> PairingScreen()
                    else -> {
                        var about by remember { mutableStateOf(false) }
                        Scaffold(
                            topBar = {
                                TopAppBar(
                                    title = {
                                        Text(
                                            "murmur",
                                            style = MaterialTheme.typography.headlineSmall,
                                        )
                                    },
                                    actions = {
                                        FilledTonalIconButton(onClick = { about = true }) {
                                            Icon(
                                                Icons.AutoMirrored.Outlined.HelpOutline,
                                                contentDescription = "what is murmur",
                                            )
                                        }
                                    },
                                    colors = TopAppBarDefaults.topAppBarColors(
                                        titleContentColor = MaterialTheme.colorScheme.primary,
                                    ),
                                )
                            },
                        ) { padding ->
                            HomeScreen(Modifier.padding(padding))
                        }
                        if (about) AboutSheet(onDismiss = { about = false })
                    }
                }
            }
        }
    }
}

/** A column of feature cards — each lives in `ui/widgets/` and owns its
 *  own state; the screen itself knows nothing about the features. */
@Composable
private fun HomeScreen(modifier: Modifier = Modifier) {
    Column(
        modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(bottom = 24.dp),
        verticalArrangement = Arrangement.Top,
    ) {
        homeWidgets.forEach { it() }
    }
}
