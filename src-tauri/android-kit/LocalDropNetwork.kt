package app.localdrop

import android.content.Context
import android.net.wifi.WifiManager
import android.util.Log

/**
 * AND-1 — Wi-Fi multicast lock.
 *
 * Android power management drops inbound broadcast and multicast packets unless
 * a MulticastLock is held. Without this, the UDP discovery listener receives
 * nothing and the radar stays empty with no visible error — the most confusing
 * failure mode on this platform.
 *
 * Wire into the generated MainActivity:
 *
 *     override fun onCreate(savedInstanceState: Bundle?) {
 *         super.onCreate(savedInstanceState)
 *         LocalDropNetwork.acquire(this)
 *     }
 *
 *     override fun onDestroy() {
 *         LocalDropNetwork.release()
 *         super.onDestroy()
 *     }
 */
object LocalDropNetwork {
    private const val TAG = "LocalDropNetwork"
    private const val LOCK_TAG = "localdrop-discovery"

    private var multicastLock: WifiManager.MulticastLock? = null

    /** Surfaced in Settings → diagnostics (FR-6.4). */
    val isHeld: Boolean
        get() = multicastLock?.isHeld == true

    fun acquire(context: Context) {
        if (isHeld) return

        try {
            val wifi = context.applicationContext
                .getSystemService(Context.WIFI_SERVICE) as WifiManager

            multicastLock = wifi.createMulticastLock(LOCK_TAG).apply {
                // Reference counting off: acquire and release are explicit and
                // paired with the activity lifecycle.
                setReferenceCounted(false)
                acquire()
            }
            Log.i(TAG, "multicast lock acquired")
        } catch (e: Exception) {
            // Discovery will not work, but the app must still start so the user
            // can at least send.
            Log.e(TAG, "failed to acquire multicast lock", e)
        }
    }

    fun release() {
        try {
            multicastLock?.takeIf { it.isHeld }?.release()
            Log.i(TAG, "multicast lock released")
        } catch (e: Exception) {
            Log.w(TAG, "failed to release multicast lock", e)
        } finally {
            multicastLock = null
        }
    }
}
