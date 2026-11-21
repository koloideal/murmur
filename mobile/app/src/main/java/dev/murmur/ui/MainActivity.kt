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
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.murmur.ui.theme.MurmurTheme
import dev.murmur.ui.theme.Unbounded
import dev.murmur.ui.widgets.homeWidgets

class MainActivity : ComponentActivity() {

    @OptIn(ExperimentalMaterial3Api::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
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
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text(
                                        "powered by kolo",
                                        style = MaterialTheme.typography.labelSmall,
                                        fontFamily = Unbounded,
                                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                                    )
                                    Spacer(Modifier.width(4.dp))
                                    Text("🦀", style = MaterialTheme.typography.labelSmall)
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
            }
        }
    }
}

/** A column of feature cards — each lives in `ui/widgets/` and owns its
 *  own state; the screen itself knows nothing about the features. The
 *  footer explains what murmur is on every screen state. */
@Composable
private fun HomeScreen(modifier: Modifier = Modifier) {
    Column(
        modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(bottom = 32.dp),
        verticalArrangement = Arrangement.Top,
    ) {
        homeWidgets.forEach { it() }
        Column(
            Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp, vertical = 4.dp),
        ) {
            Text(
                "murmur quietly links this phone to your laptops: " +
                    "notifications land on the desktop, the clipboard is shared, " +
                    "and laptop media keys control playback here.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                "your own relay · end-to-end encrypted",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
                modifier = Modifier
                    .padding(top = 10.dp)
                    .fillMaxWidth(),
            )
        }
    }
}
