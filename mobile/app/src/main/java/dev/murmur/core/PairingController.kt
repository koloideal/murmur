package dev.murmur.core

import android.content.Context
import android.util.Log
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch

/**
 * Runs the pairing handshake outside any composition. The screen that
 * drives it is recreated on rotation, theme change, process restore —
 * but the handshake is a multi-minute socket dialogue with a human in
 * the middle: if it dies with the UI, the inviter waits out its TTL
 * and (worse) the family can register a member that never comes back.
 * A process-scoped singleton scope is the only owner that matches the
 * handshake's real lifetime. The UI is a pure observer + answerer.
 */
object PairingController {
    private const val TAG = "PairingController"

    data class State(
        val phase: String = "idle",
        val sas: String = "",
        val error: String? = null,
        val hint: String? = null,
        val lanOnly: Boolean = false,
        val lanCode: Boolean = false,
        val celebrating: Boolean = false,
        val burstFamily: String? = null,
        /** Set once per handshake — a stale screen must not answer a
         *  SAS from a previous attempt. */
        val attempt: Int = 0,
    )

    val state = MutableStateFlow(State())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var job: Job? = null
    private var sasAnswer: CompletableDeferred<Boolean>? = null
    private var attempts = 0

    val active: Boolean get() = job?.isActive == true

    fun start(
        ctx: Context,
        input: String,
        short: Boolean = false,
        byCode: Boolean = false,
        server: String = "",
    ) {
        if (active) {
            Log.w(TAG, "pairing already in flight — ignoring start")
            return
        }
        attempts += 1
        val appCtx = ctx.applicationContext
        val secrets = Secrets(appCtx)
        val lanOnly = byCode ||
            (!short && Invite.decode(input)?.kind == "serverless")
        state.value = State(
            phase = "connecting",
            lanOnly = lanOnly,
            lanCode = byCode,
            attempt = attempts,
        )
        job = scope.launch {
            try {
                val onStage: (PairStage) -> Unit = { st ->
                    state.value = state.value.copy(
                        phase = when (st) {
                            is PairStage.AwaitingAccept -> "accept"
                            is PairStage.AwaitingConfirm -> "confirm"
                        },
                    )
                }
                val onSas: suspend (String) -> Boolean = { s ->
                    state.value = state.value.copy(phase = "sas", sas = s)
                    val answer = CompletableDeferred<Boolean>().also { sasAnswer = it }
                    answer.await()
                }
                val prov = if (byCode) {
                    pairLanCode(input, secrets.deviceName, onStage, onSas)
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
                state.value = state.value.copy(
                    phase = "done",
                    celebrating = true,
                    burstFamily = prov.familyName,
                    error = null,
                    hint = null,
                )
            } catch (e: CancellationException) {
                state.value = State(attempt = attempts)
                throw e
            } catch (e: PairingException) {
                state.value = State(
                    attempt = attempts,
                    error = e.message,
                    hint = when {
                        byCode -> "check the code — it expires 10 minutes after the invite was made"
                        short -> "check the code and the server address, then try again"
                        lanOnly -> "are the phone and the laptop on the same network?"
                        else -> null
                    },
                    lanOnly = lanOnly,
                )
            } catch (e: Exception) {
                state.value = State(
                    attempt = attempts,
                    error = e.message ?: "pairing failed",
                    hint = when {
                        byCode || lanOnly ->
                            "can't reach the laptop — same wi-fi? invite still open?"
                        short -> "can't reach the server — is the address right and the port open?"
                        else -> null
                    },
                    lanOnly = lanOnly,
                )
            }
        }
    }

    fun answerSas(answer: Boolean) {
        if (answer) {
            state.value = state.value.copy(phase = "confirm")
        } else {
            state.value = State(attempt = attempts)
        }
        sasAnswer?.complete(answer)
        sasAnswer = null
    }

    fun cancel() {
        sasAnswer?.complete(false)
        sasAnswer = null
        job?.cancel()
        job = null
        state.value = State(attempt = attempts)
    }

    /** The burst played: flip the world. The caller (UI layer) starts
     *  the service — core doesn't depend on it. */
    fun finishCelebration() {
        if (!state.value.celebrating) return
        state.value = State(attempt = attempts)
        Secrets.settledFlow.value = true
    }
}
