package dev.murmur.ui.widgets

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Cloud
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.KeyboardArrowUp
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material.icons.outlined.Wifi
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.graphics.vector.ImageVector
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
 * Everything an unpaired phone sees — the entire screen. First the
 * literal split: two halves, serverful vs serverless. Then the guided
 * pairing: QR is the primary path, the typeable XXXX-XXXX code is the
 * manual fallback. Both converge on the six-digit SAS comparison.
 * Once the phone is provisioned and the welcome burst has played,
 * MainActivity swaps this for the dashboard.
 */
@Composable
fun PairingScreen() {
    val ctx = LocalContext.current
    val secrets = remember { Secrets(ctx) }
    val scope = rememberCoroutineScope()

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
    var burstFamily by remember { mutableStateOf<String?>(null) }
    var lanOnly by remember { mutableStateOf(false) }
    var lanCode by remember { mutableStateOf(false) }
    /** Family kind the user picked on the split screen — null = the
     *  choice itself is what's showing. The invite that arrives from
     *  the laptop is the real truth; this only shapes which fields
     *  the phone even offers (a serverless world has no server field). */
    var kind by remember { mutableStateOf<String?>(null) }

    val codeReady = code.length == 8

    fun runPairing(input: String, short: Boolean = false, byCode: Boolean = false) {
        phase = "connecting"
        error = null
        hint = null
        lanCode = byCode
        // The invite itself declares the family's kind — a serverless
        // one dials the laptop directly and never sees a server.
        lanOnly = byCode ||
            (!short && dev.murmur.core.Invite.decode(input)?.kind == "serverless")
        job = scope.launch {
            try {
                val onStage: (PairStage) -> Unit = { st ->
                    when (st) {
                        is PairStage.AwaitingAccept -> phase = "accept"
                        is PairStage.AwaitingConfirm -> phase = "confirm"
                    }
                }
                val onSas: suspend (String) -> Boolean = { s ->
                    sas = s
                    phase = "sas"
                    val answer = CompletableDeferred<Boolean>().also { sasAnswer = it }
                    answer.await()
                }
                val prov = if (byCode) {
                    dev.murmur.core.pairLanCode(input, secrets.deviceName, onStage, onSas)
                } else {
                    pair(
                        input,
                        secrets.deviceName,
                        serverUrl = if (short) server.trim() else null,
                        onStage = onStage,
                        onSas = onSas,
                    )
                }
                secrets.save(prov.deviceId, prov.tokenHex, prov.familyKey, prov.serverUrl, prov.keyEpoch, prov.familyName, prov.familyKind)
                burstFamily = prov.familyName
                phase = "done"
                // Service starts only after the burst finishes — it
                // constructs a Secrets whose init flips settledFlow,
                // which would collapse this card before frame one.
                celebrating = true
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
        lanCode = false
        phase = "idle"
    }

    if (kind == null) {
        FamilyKindSplit { picked ->
            kind = picked
            if (picked == "serverless") lanOnly = true
        }
        return
    }

    // A bare Column leaves LocalContentColor black — unreadable on
    // the gruvbox background. Surface owns both for everything inside.
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(24.dp),
            verticalArrangement = Arrangement.Center,
        ) {
        FeatureCard("pair this phone", Icons.Outlined.QrCodeScanner) {
            when {
                celebrating -> Column(
                    Modifier.fillMaxWidth(),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    PairDoneBurst(familyName = burstFamily, onFinished = {
                        celebrating = false
                        phase = "idle"
                        code = ""
                        invite = ""
                        Secrets.settledFlow.value = true
                        MurmurService.start(ctx)
                    })
                }

                phase == "connecting" -> Row(verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.size(12.dp))
                    Text(
                        when {
                            lanCode -> "listening for the laptop on the lan…"
                            lanOnly -> "connecting to the laptop…"
                            else -> "connecting to the relay…"
                        },
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
                    if (lanOnly) {
                        Text(
                            "lan-only family — devices talk directly on one network",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Spacer(Modifier.height(8.dp))
                    }
                    StepList(laptopSteps, current = if (phase == "accept") 0 else 2)
                    Spacer(Modifier.height(12.dp))
                    OutlinedButton(
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
                        OutlinedButton(onClick = {
                            phase = "idle"
                            sasAnswer?.complete(false)
                        }) { Text("don't match") }
                    }
                }

                else -> {
                    Text(
                        if (kind == "serverless") "on a laptop in your lan-only family, run"
                        else "on a laptop that's already in the family, run",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    CommandLine("murmurctl invite", Modifier.padding(top = 8.dp))
                    Spacer(Modifier.height(12.dp))
                    Button(onClick = ::scan, modifier = Modifier.fillMaxWidth()) {
                        Text("scan QR code", textAlign = TextAlign.Center)
                    }
                    OutlinedButton(
                        onClick = { manual = !manual },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("enter code manually", textAlign = TextAlign.Center)
                        Icon(
                            if (manual) Icons.Outlined.KeyboardArrowUp
                            else Icons.Outlined.KeyboardArrowDown,
                            contentDescription = null,
                            modifier = Modifier.size(18.dp),
                        )
                    }
                    if (manual) {
                        if (kind == "serverless") {
                            // The laptop's LAN pair beacon resolves the
                            // typed code to its `host:port` — a serverless
                            // family still needs no server field and no
                            // untypeable full-invite blob.
                            ShortCodeField(
                                code = code,
                                onCode = { code = it },
                            )
                            Spacer(Modifier.height(8.dp))
                            Text(
                                "the laptop prints the code under the QR",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        } else if (inviteMode) {
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
                        Spacer(Modifier.height(8.dp))
                        val ready = when {
                            kind == "serverless" -> codeReady
                            inviteMode -> invite.isNotBlank()
                            else -> codeReady && server.isNotBlank()
                        }
                        Button(
                            onClick = {
                                when {
                                    kind == "serverless" -> runPairing("$code", byCode = true)
                                    inviteMode -> runPairing(invite.trim())
                                    else -> runPairing("$code", short = true)
                                }
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
                    OutlinedButton(
                        onClick = {
                            kind = null
                            lanOnly = false
                            manual = false
                            error = null
                            hint = null
                        },
                        modifier = Modifier.align(Alignment.End),
                    ) { Text("‹ family kind") }
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
 *  text field drives input; the cells are pure presentation. Tapping a
 *  cell moves the caret there — the focused cell gets the accent ring. */
@Composable
private fun ShortCodeField(code: String, onCode: (String) -> Unit) {
    val focus = remember { FocusRequester() }
    var focused by remember { mutableStateOf(false) }
    // The field's own value carries the caret; `code` is the parent copy.
    var tfv by remember { mutableStateOf(TextFieldValue(code, TextRange(code.length))) }
    if (tfv.text != code) { // parent reset (pair failed / left the family)
        tfv = TextFieldValue(code, TextRange(code.length))
    }
    BasicTextField(
        value = tfv,
        onValueChange = { v ->
            // Only the invite alphabet counts. Cyrillic homoglyphs map to
            // their Latin lookalikes — a Cyrillic Е hashes to a different
            // session id and the relay rejects it as an unknown invite.
            val clean = v.text
                .map { CYRILLIC_TO_LATIN[it] ?: it.uppercaseChar() }
                .filter { it in CODE_ALPHABET }
                .take(8)
                .joinToString("")
            val caret = v.text
                .take(v.selection.start)
                .map { CYRILLIC_TO_LATIN[it] ?: it.uppercaseChar() }
                .filter { it in CODE_ALPHABET }
                .take(8)
                .size
            tfv = TextFieldValue(clean, TextRange(caret))
            onCode(clean)
        },
        modifier = Modifier
            .fillMaxWidth()
            .focusRequester(focus)
            .onFocusChanged { focused = it.isFocused },
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
                    val caret = tfv.selection.start
                    repeat(4) { i ->
                        Cell(code.getOrNull(i), focused && i == caret, Modifier.weight(1f)) {
                            tfv = tfv.copy(selection = TextRange(i))
                            focus.requestFocus()
                        }
                    }
                    Text(
                        "–",
                        style = MaterialTheme.typography.headlineSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    repeat(4) { i ->
                        val idx = i + 4
                        Cell(code.getOrNull(idx), focused && idx == caret, Modifier.weight(1f)) {
                            tfv = tfv.copy(selection = TextRange(idx))
                            focus.requestFocus()
                        }
                    }
                }
            }
        },
    )
}

@Composable
private fun Cell(ch: Char?, active: Boolean, modifier: Modifier = Modifier, onTap: () -> Unit = {}) {
    Box(
        modifier
            .height(58.dp)
            .clickable(
                interactionSource = remember { MutableInteractionSource() },
                indication = null,
                onClick = onTap,
            )
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

/** The first thing an unpaired phone ever sees: the screen literally
 *  split in two. The family's transport is the one decision nothing
 *  else can be built on — the invite still carries the truth, this
 *  only decides which fields the phone will offer at all. */
@Composable
private fun FamilyKindSplit(onPick: (String) -> Unit) {
    // Surface, not a bare Box: it owns the background and the default
    // text color — outside one, LocalContentColor is black.
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            KindHalf(
                Modifier.weight(1f),
                Icons.Outlined.Cloud,
                "serverful",
                "through the relay — works from anywhere",
            ) { onPick("serverful") }
            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            KindHalf(
                Modifier.weight(1f),
                Icons.Outlined.Wifi,
                "serverless",
                "direct on one network — no server, ever",
            ) { onPick("serverless") }
        }
        Surface(
            Modifier.align(Alignment.Center),
            shape = MaterialTheme.shapes.extraLarge,
            color = MaterialTheme.colorScheme.surface,
            tonalElevation = 3.dp,
            shadowElevation = 3.dp,
        ) {
            Text(
                "what kind of family?",
                Modifier.padding(horizontal = 18.dp, vertical = 8.dp),
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    }
}

@Composable
private fun KindHalf(
    modifier: Modifier,
    icon: ImageVector,
    title: String,
    sub: String,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            Modifier.padding(horizontal = 32.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(
                icon,
                contentDescription = null,
                modifier = Modifier.size(72.dp),
                tint = MaterialTheme.colorScheme.primary,
            )
            Spacer(Modifier.height(16.dp))
            Text(title, style = MaterialTheme.typography.displaySmall)
            Spacer(Modifier.height(6.dp))
            Text(
                sub,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.Center,
            )
        }
    }
}
