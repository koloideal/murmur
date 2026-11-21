package dev.murmur.ui.widgets

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.murmur.MurmurService
import dev.murmur.core.PairingException
import dev.murmur.core.Secrets
import dev.murmur.core.pair
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch

private val SHORT_CODE = Regex("[0-9A-Za-z]{4}-?[0-9A-Za-z]{4}")

/**
 * Guided pairing: QR is the primary path, the typeable XXXX-XXXX code is
 * the manual fallback. Both converge on the six-digit SAS comparison.
 */
@Composable
fun PairingCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val scope = rememberCoroutineScope()

    var phase by remember { mutableStateOf(if (secrets.provisioned) "done" else "idle") }
    var manual by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") }
    var server by remember { mutableStateOf(secrets.serverUrl.ifEmpty { "ws://" }) }
    var sas by remember { mutableStateOf("") }
    var sasAnswer by remember { mutableStateOf<CompletableDeferred<Boolean>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    val isShortCode = code.trim().matches(SHORT_CODE)

    fun runPairing(input: String, short: Boolean = false) {
        phase = "connecting"
        error = null
        scope.launch {
            try {
                val prov = pair(
                    input,
                    Build.MODEL,
                    serverUrl = if (short) server.trim() else null,
                ) { s ->
                    sas = s
                    phase = "sas"
                    val answer = CompletableDeferred<Boolean>().also { sasAnswer = it }
                    answer.await()
                }
                secrets.save(prov.deviceId, prov.tokenHex, prov.familyKey, prov.serverUrl)
                phase = "done"
                MurmurService.start(ctx)
            } catch (e: PairingException) {
                error = e.message
                phase = "idle"
            } catch (e: Exception) {
                error = e.message ?: "pairing failed"
                phase = "idle"
            }
        }
    }

    val scanner = rememberLauncherForActivityResult(ScanContract()) { res ->
        res.contents?.let { runPairing(it) }
    }
    val cameraPerm = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) scanner.launch(qrOptions())
    }
    fun scan() {
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA)
            == PackageManager.PERMISSION_GRANTED
        ) {
            scanner.launch(qrOptions())
        } else {
            cameraPerm.launch(Manifest.permission.CAMERA)
        }
    }

    FeatureCard("pair this phone", Icons.Outlined.QrCodeScanner) {
        when (phase) {
            "done" -> Text(
                "this phone is part of the family",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.primary,
            )

            "connecting" -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                Spacer(Modifier.size(12.dp))
                Text(
                    "contacting the inviter…",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            "sas" -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    "does this match the code on the laptop?",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                Surface(
                    color = MaterialTheme.colorScheme.primaryContainer,
                    shape = MaterialTheme.shapes.medium,
                ) {
                    Text(
                        sas.chunked(3).joinToString(" "),
                        modifier = Modifier.padding(horizontal = 28.dp, vertical = 14.dp),
                        textAlign = TextAlign.Center,
                        style = MaterialTheme.typography.headlineLarge,
                        fontFamily = FontFamily.Monospace,
                        color = MaterialTheme.colorScheme.onPrimaryContainer,
                    )
                }
                Spacer(Modifier.height(12.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Button(onClick = {
                        phase = "connecting"
                        sasAnswer?.complete(true)
                    }) { Text("match") }
                    TextButton(onClick = {
                        phase = "idle"
                        sasAnswer?.complete(false)
                    }) { Text("reject") }
                }
            }

            else -> {
                Text(
                    "run  murmurctl invite  on a laptop",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                Button(onClick = ::scan, modifier = Modifier.fillMaxWidth()) {
                    Text("scan QR code", textAlign = TextAlign.Center)
                }
                TextButton(
                    onClick = { manual = !manual },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("enter code manually") }
                if (manual) {
                    OutlinedTextField(
                        value = code,
                        onValueChange = { code = it },
                        modifier = Modifier.fillMaxWidth(),
                        label = { Text("invite or short code") },
                        placeholder = { Text("K7Q2-J4MN") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii),
                    )
                    if (isShortCode) {
                        Spacer(Modifier.height(8.dp))
                        OutlinedTextField(
                            value = server,
                            onValueChange = { server = it },
                            modifier = Modifier.fillMaxWidth(),
                            label = { Text("server") },
                            placeholder = { Text("ws://192.168.0.1:8770") },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                        )
                    }
                    Spacer(Modifier.height(8.dp))
                    Button(
                        onClick = { runPairing(code.trim(), isShortCode) },
                        enabled = code.isNotBlank() && (!isShortCode || server.isNotBlank()),
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("pair") }
                }
                error?.let {
                    Spacer(Modifier.height(8.dp))
                    Text(
                        it,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
        }
    }
}

private fun qrOptions() = ScanOptions().apply {
    setDesiredBarcodeFormats(ScanOptions.QR_CODE)
    setPrompt("point at the laptop QR")
    setBeepEnabled(false)
    setOrientationLocked(false)
}
