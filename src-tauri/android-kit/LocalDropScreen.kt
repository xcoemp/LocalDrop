package app.localdrop

import android.app.Activity
import android.util.Log
import android.view.WindowManager

/**
 * Keeps the screen on while a transfer is running.
 *
 * A large transfer easily outlasts the display timeout, and once the screen
 * sleeps the app can be suspended mid-stream — the socket dies and the peer
 * reports `ERR_CONNECTION_LOST` with no indication of why.
 *
 * `FLAG_KEEP_SCREEN_ON` is the right tool here rather than a `PowerManager`
 * wake lock: it needs no permission, is scoped to this window, and is released
 * automatically if the app is killed, so it cannot leak and flatten the battery.
 *
 * Called from Rust over JNI. The UI-thread hop lives here because window flags
 * may only be touched on the UI thread, and the caller is a Tokio worker.
 */
object LocalDropScreen {
    private const val TAG = "LocalDropScreen"

    @JvmStatic
    fun keepAwake(activity: Activity, on: Boolean) {
        try {
            activity.runOnUiThread {
                if (on) {
                    activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                } else {
                    activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
            }
        } catch (e: Exception) {
            // A transfer must not fail because the screen hint failed.
            Log.w(TAG, "could not toggle keep-screen-on", e)
        }
    }
}
