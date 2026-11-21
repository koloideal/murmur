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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
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

/**
 * Join the family: scan the QR (or paste the invite), compare the SAS code
 * with the laptop, confirm — credentials land in Keystore storage and the
 * service starts itself.
 */
@Composable
fun PairingCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val scope = rememberCoroutineScope()

    var phase by remember { mutableStateOf(if (secrets.provisioned) "done" else "idle") }
    var invite by remember { mutableStateOf("") }
    var sas by remember { mutableStateOf("") }
    var sasAnswer by remember { mutableStateOf<CompletableDeferred<Boolean>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    fun runPairing(code: String) {
        invite = ""
        phase = "connecting"
        error = null
        scope.launch {
            try {
                val prov = pair(code, Build.MODEL) { code ->
                    sas = code
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
        res.contents?.let(::runPairing)
    }
    val cameraPerm = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) scanner.launch(qrOptions())
    }

    FeatureCard("pair a device", Icons.Outlined.QrCodeScanner) {
        when (phase) {
            "done" -> Text(
                "this device is part of the family",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )

            "connecting" -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                Spacer(Modifier.size(12.dp))
                Text("contacting the inviter…", style = MaterialTheme.typography.bodyMedium)
            }

            "sas" -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    "check the code on the laptop",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(8.dp))
                Text(
                    sas.chunked(3).joinToString(" "),
                    style = MaterialTheme.typography.displayMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
                Spacer(Modifier.height(12.dp))
                Row {
                    Button(onClick = {
                        phase = "connecting"
                        sasAnswer?.complete(true)
                    }) { Text("match") }
                    Spacer(Modifier.size(12.dp))
                    OutlinedButton(onClick = {
                        phase = "idle"
                        sasAnswer?.complete(false)
                    }) { Text("reject") }
                }
            }

            else -> Column {
                if (error != null) {
                    Text(
                        error!!,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    Spacer(Modifier.height(8.dp))
                }
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    OutlinedTextField(
                        value = invite,
                        onValueChange = { invite = it },
                        modifier = Modifier.weight(1f),
                        placeholder = { Text("paste invite") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii),
                    )
                    Button(
                        onClick = { runPairing(invite.trim()) },
                        enabled = invite.isNotBlank(),
                    ) { Text("pair") }
                }
                Spacer(Modifier.height(8.dp))
                OutlinedButton(
                    onClick = {
                        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA)
                            == PackageManager.PERMISSION_GRANTED
                        ) {
                            scanner.launch(qrOptions())
                        } else {
                            cameraPerm.launch(Manifest.permission.CAMERA)
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        "scan QR invite",
                        textAlign = TextAlign.Center,
                        modifier = Modifier.fillMaxWidth(),
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
