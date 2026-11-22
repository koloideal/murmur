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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.murmur.ui.theme.MurmurTheme
import dev.murmur.ui.widgets.homeWidgets

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
        setContent {
            var about by remember { mutableStateOf(false) }
            MurmurTheme {
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
