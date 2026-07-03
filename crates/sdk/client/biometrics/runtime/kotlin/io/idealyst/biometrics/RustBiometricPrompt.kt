package io.idealyst.biometrics

import android.content.Context
import android.content.DialogInterface
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper

/**
 * Bridges the framework `android.hardware.biometrics.BiometricPrompt`
 * (API 28+, no androidx) to Rust.
 *
 * Rust calls [authenticate] with a `token` that identifies the awaiting
 * oneshot sender on the native side. The prompt is built and shown on the
 * main (UI) thread; terminal outcomes trampoline back through
 * [nativeResult].
 *
 * `code` is `0` on success, otherwise the raw Android
 * `BiometricPrompt.BIOMETRIC_ERROR_*` value (which Rust maps to a typed
 * `BioError`). `onAuthenticationFailed` — a single non-terminal miss — is
 * deliberately NOT reported: the system prompt stays up for another try.
 *
 * Shipped from the `biometrics` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`; the `nativeResult`
 * symbol is the `#[no_mangle]` export in `android.rs`.
 */
object RustBiometricPrompt {
    @JvmStatic
    fun authenticate(
        context: Context,
        title: String,
        subtitle: String,
        negative: String,
        allowCredential: Boolean,
        token: Long,
    ) {
        Handler(Looper.getMainLooper()).post {
            try {
                show(context, title, subtitle, negative, allowCredential, token)
            } catch (t: Throwable) {
                // Negative sentinel; Rust maps unknown codes to BioError::Backend.
                nativeResult(token, -1, t.message ?: t.toString())
            }
        }
    }

    private fun show(
        context: Context,
        title: String,
        subtitle: String,
        negative: String,
        allowCredential: Boolean,
        token: Long,
    ) {
        val executor = context.mainExecutor
        val builder = BiometricPrompt.Builder(context).setTitle(title)
        if (subtitle.isNotEmpty()) {
            builder.setSubtitle(subtitle)
        }

        // `setAllowedAuthenticators` (API 30+) and a negative button are
        // mutually exclusive: opting into device-credential fallback removes
        // the negative button, since the system supplies a "use PIN" path.
        if (allowCredential && Build.VERSION.SDK_INT >= 30) {
            builder.setAllowedAuthenticators(
                BiometricManager.Authenticators.BIOMETRIC_STRONG or
                    BiometricManager.Authenticators.DEVICE_CREDENTIAL
            )
        } else {
            builder.setNegativeButton(
                negative,
                executor,
                DialogInterface.OnClickListener { _, _ -> }
            )
        }

        val prompt = builder.build()
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                nativeResult(token, 0, null)
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                nativeResult(token, errorCode, errString.toString())
            }
            // onAuthenticationFailed(): non-terminal single miss — ignored.
        }
        prompt.authenticate(CancellationSignal(), executor, callback)
    }

    @JvmStatic
    private external fun nativeResult(token: Long, code: Int, message: String?)
}
