package dev.murmur.ui.widgets

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.draw.alpha
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import dev.murmur.MurmurService
import dev.murmur.core.PairStage
import dev.murmur.core.PairingException
import dev.murmur.core.Secrets
import dev.murmur.core.pair
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch

/**
 * Guided pairing: QR is the primary path, the typeable XXXX-XXXX code is
 * the manual fallback. Both converge on the six-digit SAS comparison.
 * Once the phone is provisioned and the welcome burst has played, this
 * card folds away and the dashboard takes over.
 */
@Composable
fun PairingCard() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val scope = rememberCoroutineScope()
    val settled by Secrets.settledFlow.collectAsState()

    var phase by remember { mutableStateOf("idle") }
    var manual by remember { mutableStateOf(false) }
    var inviteMode by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") }
    var invite by remember { mutableStateOf("") }
    var server by remember { mutableStateOf(secrets.serverUrl.ifEmpty { "wss://" }) }
    var job by remember { mutableStateOf<Job?>(null) }
    var scanning by remember { mutableStateOf(false) }
    var sas by remember { mutableStateOf("") }
    var sasAnswer by remember { mutableStateOf<CompletableDeferred<Boolean>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var hint by remember { mutableStateOf<String?>(null) }
    var celebrating by remember { mutableStateOf(false) }

    val codeReady = code.length == 8

    fun runPairing(input: String, short: Boolean = false) {
        phase = "connecting"
        error = null
        hint = null
        job = scope.launch {
            try {
                val prov = pair(
                    input,
                    secrets.deviceName,
                    serverUrl = if (short) server.trim() else null,
                    onStage = { st ->
                        when (st) {
                            is PairStage.AwaitingAccept -> phase = "accept"
                            is PairStage.AwaitingConfirm -> phase = "confirm"
                        }
                    },
                ) { s ->
                    sas = s
                    phase = "sas"
                    val answer = CompletableDeferred<Boolean>().also { sasAnswer = it }
                    answer.await()
                }
                secrets.save(prov.deviceId, prov.tokenHex, prov.familyKey, prov.serverUrl, prov.keyEpoch)
                phase = "done"
                celebrating = true
                MurmurService.start(ctx)
            } catch (e: CancellationException) {
                phase = "idle"
                throw e
            } catch (e: PairingException) {
                error = e.message
                if (short) hint = "check the code and the server address, then try again"
                phase = "idle"
            } catch (e: Exception) {
                error = e.message ?: "pairing failed"
                if (short) hint = "can't reach the server — is the address right and the port open?"
                phase = "idle"
            }
        }
    }

    val cameraPerm = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        if (granted) scanning = true
        else {
            manual = true
            hint = "no camera access — enter the code manually instead"
        }
    }
    fun scan() {
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA)
            == PackageManager.PERMISSION_GRANTED
        ) {
            scanning = true
        } else {
            cameraPerm.launch(Manifest.permission.CAMERA)
        }
    }

    if (scanning) {
        QrScanner(
            onResult = {
                scanning = false
                runPairing(it)
            },
            onDismiss = { scanning = false },
        )
    }

    // The laptop side is one interactive command: `murmurctl invite`
    // keeps waiting after the QR and walks accept → code → confirm.
    val laptopSteps = listOf(
        Step(
            "accept on the laptop — answer y to “accept it?”. " +
                "Closed the invite already? Run",
            "murmurctl pair",
        ),
        Step("compare the six digits here and on the laptop"),
        Step("confirm on the laptop — answer y to “codes match?”"),
    )
    fun cancel() {
        job?.cancel()
        sasAnswer?.complete(false)
        phase = "idle"
    }

    AnimatedVisibility(
        visible = !settled,
        exit = fadeOut() + shrinkVertically(),
        enter = fadeIn() + expandVertically(),
    ) {
        FeatureCard("pair this phone", Icons.Outlined.QrCodeScanner) {
            when {
                celebrating -> Column(
                    Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    PairDoneBurst(onFinished = {
                        celebrating = false
                        Secrets.settledFlow.value = true
                    })
                }

                phase == "connecting" -> Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(12.dp))
                    Text(
                        "connecting to the relay…",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                phase == "accept" || phase == "confirm" -> Column(Modifier.fillMaxWidth()) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        Spacer(Modifier.size(12.dp))
                        Text(
                            if (phase == "accept") "waiting for the laptop to accept…"
                            else "waiting for the laptop to confirm…",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Spacer(Modifier.height(18.dp))
                    StepList(laptopSteps, current = if (phase == "accept") 0 else 2)
                    Spacer(Modifier.height(12.dp))
                    TextButton(
                        onClick = ::cancel,
                        modifier = Modifier.align(Alignment.End),
                    ) { Text("cancel") }
                }

                phase == "sas" -> Column(
                    Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(
                        "does this match the code on the laptop?",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        "the laptop shows them under “verification code”",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.height(12.dp))
                    Surface(
                        color = MaterialTheme.colorScheme.primaryContainer,
                        shape = MaterialTheme.shapes.medium,
                    ) {
                        Box(
                            Modifier
                                .fillMaxWidth()
                                .height(86.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                sas.chunked(3).joinToString(" "),
                                style = MaterialTheme.typography.displaySmall,
                                fontFamily = FontFamily.Monospace,
                                color = MaterialTheme.colorScheme.onPrimaryContainer,
                            )
                        }
                    }
                    Spacer(Modifier.height(12.dp))
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Button(onClick = {
                            phase = "confirm"
                            sasAnswer?.complete(true)
                        }) { Text("match") }
                        TextButton(onClick = {
                            phase = "idle"
                            sasAnswer?.complete(false)
                        }) { Text("don't match") }
                    }
                }

                else -> {
                    Text(
                        "on a laptop that's already in the family, run",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    CommandLine("murmurctl invite", Modifier.padding(top = 8.dp))
                    Spacer(Modifier.height(12.dp))
                    Button(onClick = ::scan, modifier = Modifier.fillMaxWidth()) {
                        Text("scan QR code", textAlign = TextAlign.Center)
                    }
                    TextButton(
                        onClick = { manual = !manual },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("enter code manually") }
                    if (manual) {
                        if (inviteMode) {
                            OutlinedTextField(
                                value = invite,
                                onValueChange = { invite = it },
                                modifier = Modifier.fillMaxWidth(),
                                label = { Text("full invite") },
                                singleLine = true,
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii),
                            )
                            TextButton(
                                onClick = { inviteMode = false },
                                modifier = Modifier.align(Alignment.End),
                            ) { Text("have a short code?") }
                        } else {
                            ShortCodeField(
                                code = code,
                                onCode = { code = it },
                            )
                            Spacer(Modifier.height(8.dp))
                            OutlinedTextField(
                                value = server,
                                onValueChange = { server = it },
                                modifier = Modifier.fillMaxWidth(),
                                label = { Text("server") },
                                placeholder = { Text("wss://relay.example.com") },
                                singleLine = true,
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                            )
                            TextButton(
                                onClick = { inviteMode = true },
                                modifier = Modifier.align(Alignment.End),
                            ) { Text("paste a full invite instead") }
                        }
                        Spacer(Modifier.height(4.dp))
                        val ready = if (inviteMode) invite.isNotBlank()
                        else codeReady && server.isNotBlank()
                        Button(
                            onClick = {
                                if (inviteMode) runPairing(invite.trim())
                                else runPairing("$code", short = true)
                            },
                            enabled = ready,
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
                    hint?.let {
                        Spacer(Modifier.height(4.dp))
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

/** The laptop's invite alphabet (murmur_proto CODE_ALPHABET) — typing
 *  anything outside it can never resolve to a real session. */
private val CODE_ALPHABET = "23456789ABCDEFGHJKMNPQRSTUVWXYZ".toSet()

/** Cyrillic letters that look exactly like Latin invite characters. */
private val CYRILLIC_TO_LATIN = mapOf(
    'а' to 'A', 'А' to 'A',
    'в' to 'B', 'В' to 'B',
    'е' to 'E', 'Е' to 'E',
    'к' to 'K', 'К' to 'K',
    'м' to 'M', 'М' to 'M',
    'н' to 'H', 'Н' to 'H',
    'о' to 'O', 'О' to 'O',
    'р' to 'P', 'Р' to 'P',
    'с' to 'C', 'С' to 'C',
    'т' to 'T', 'Т' to 'T',
    'у' to 'Y', 'У' to 'Y',
    'х' to 'X', 'Х' to 'X',
)

/** Eight cells with the dash fixed in the middle — the XXXX-XXXX
 *  shape is the form, not something the user types. A single invisible
 *  text field drives input; the cells are pure presentation. */
@Composable
private fun ShortCodeField(code: String, onCode: (String) -> Unit) {
    BasicTextField(
        value = code,
        onValueChange = { v ->
            // Only the invite alphabet counts. Cyrillic homoglyphs map to
            // their Latin lookalikes — a Cyrillic Е hashes to a different
            // session id and the relay rejects it as an unknown invite.
            onCode(
                v.map { CYRILLIC_TO_LATIN[it] ?: it.uppercaseChar() }
                    .filter { it in CODE_ALPHABET }
                    .take(8)
                    .joinToString(""),
            )
        },
        modifier = Modifier.fillMaxWidth(),
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.Characters,
            keyboardType = KeyboardType.Ascii,
        ),
        decorationBox = { inner ->
            Box(Modifier.fillMaxWidth()) {
                // Keep the editable element alive but invisible — the
                // cells do the talking.
                Box(Modifier.alpha(0f)) { inner() }
                Row(
                    Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(5.dp),
                ) {
                    repeat(4) { i -> Cell(code.getOrNull(i), i == code.length, Modifier.weight(1f)) }
                    Text(
                        "–",
                        style = MaterialTheme.typography.headlineSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    repeat(4) { i ->
                        Cell(code.getOrNull(i + 4), i + 4 == code.length, Modifier.weight(1f))
                    }
                }
            }
        },
    )
}

@Composable
private fun Cell(ch: Char?, active: Boolean, modifier: Modifier = Modifier) {
    Box(
        modifier
            .height(58.dp)
            .background(
                MaterialTheme.colorScheme.surfaceContainerHigh,
                MaterialTheme.shapes.medium,
            )
            .then(
                if (active) Modifier.border(
                    1.dp,
                    MaterialTheme.colorScheme.primary,
                    MaterialTheme.shapes.medium,
                ) else Modifier,
            ),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            ch?.toString() ?: "·",
            style = MaterialTheme.typography.headlineSmall,
            fontFamily = FontFamily.Monospace,
            color = if (ch != null) MaterialTheme.colorScheme.onSurface
            else MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
