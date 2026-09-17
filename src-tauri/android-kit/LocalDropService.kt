package app.localdrop

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * Keeps LocalDrop's sockets alive while the app is backgrounded (AND-2).
 *
 * Android aggressively suspends background processes. Without this service a
 * transfer dies partway through and the peer reports:
 *
 * ```text
 * ERR_CONNECTION_LOST
 * An existing connection was forcibly closed by the remote host (os error 10054)
 * ```
 *
 * The desktop is only reporting the symptom — the phone stopped being scheduled.
 *
 * Three separate mechanisms are needed, and each covers a different failure:
 *
 *  - **Foreground service + ongoing notification.** Exempts the process from
 *    the background execution limits that would otherwise freeze or kill it.
 *  - **`PARTIAL_WAKE_LOCK`.** The foreground status alone does not stop the CPU
 *    suspending when the screen turns off; the wake lock does.
 *  - **Battery-optimisation exemption.** Doze can still freeze a non-exempt app
 *    between maintenance windows. This one needs the user to agree, so it is
 *    offered rather than assumed — see [requestIgnoreBatteryOptimizations].
 */
class LocalDropService : Service() {
    companion object {
        private const val TAG = "LocalDropService"
        private const val CHANNEL_ID = "localdrop_background"
        private const val NOTIFICATION_ID = 4201
        private const val WAKE_LOCK_TAG = "LocalDrop::network"

        const val EXTRA_DETAIL = "detail"
        const val ACTION_STOP = "app.localdrop.STOP_SERVICE"

        private const val DEFAULT_DETAIL = "Listening for LAN shares…"

        /**
         * Start the service, or update its notification if already running.
         *
         * Safe to call repeatedly; `startForegroundService` on an existing
         * service just delivers a new intent.
         */
        @JvmStatic
        fun start(context: Context, detail: String?) {
            val intent =
                Intent(context, LocalDropService::class.java)
                    .putExtra(EXTRA_DETAIL, detail ?: DEFAULT_DETAIL)
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    context.startForegroundService(intent)
                } else {
                    context.startService(intent)
                }
            } catch (e: Exception) {
                // Transfers must still work if the service cannot start, e.g.
                // when launched from the background on API 31+.
                Log.w(TAG, "could not start foreground service", e)
            }
        }

        @JvmStatic
        fun stop(context: Context) {
            try {
                context.startService(
                    Intent(context, LocalDropService::class.java).setAction(ACTION_STOP)
                )
            } catch (e: Exception) {
                Log.w(TAG, "could not stop foreground service", e)
            }
        }

        /** Whether the app is already exempt from Doze restrictions. */
        @JvmStatic
        fun isIgnoringBatteryOptimizations(context: Context): Boolean {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return true
            val power = context.getSystemService(Context.POWER_SERVICE) as PowerManager
            return power.isIgnoringBatteryOptimizations(context.packageName)
        }

        /**
         * Open the system prompt asking to exempt LocalDrop from battery
         * optimisation.
         *
         * Only worth calling from a user-initiated action: the dialog is
         * disruptive, and Google Play restricts apps that request it without a
         * qualifying use case.
         */
        @JvmStatic
        fun requestIgnoreBatteryOptimizations(context: Context) {
            if (isIgnoringBatteryOptimizations(context)) return

            val direct =
                Intent(
                        Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
                        Uri.parse("package:${context.packageName}"),
                    )
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

            try {
                context.startActivity(direct)
            } catch (e: Exception) {
                // Some OEM builds block the direct intent; fall back to the list.
                Log.w(TAG, "direct battery-optimisation prompt unavailable", e)
                try {
                    context.startActivity(
                        Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    )
                } catch (e2: Exception) {
                    Log.e(TAG, "cannot open battery optimisation settings", e2)
                }
            }
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        acquireWakeLock()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }

        val detail = intent?.getStringExtra(EXTRA_DETAIL) ?: DEFAULT_DETAIL
        ensureChannel()

        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                // API 34+ requires the type here as well as in the manifest.
                startForeground(
                    NOTIFICATION_ID,
                    buildNotification(detail),
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
                )
            } else {
                startForeground(NOTIFICATION_ID, buildNotification(detail))
            }
        } catch (e: Exception) {
            Log.e(TAG, "startForeground rejected", e)
            stopSelf()
            return START_NOT_STICKY
        }

        // Sticky: if the process is killed while listening, come back so the
        // device stays discoverable.
        return START_STICKY
    }

    override fun onDestroy() {
        releaseWakeLock()
        super.onDestroy()
    }

    /**
     * Keeps the CPU scheduled with the screen off.
     *
     * No timeout: the lock's lifetime is the service's, and the service is
     * visible to the user in the shade, so it cannot silently drain the battery.
     * It is released in [onDestroy], including when the system kills us.
     */
    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        try {
            val power = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock =
                power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_LOCK_TAG).apply {
                    setReferenceCounted(false)
                    acquire()
                }
            Log.i(TAG, "partial wake lock acquired")
        } catch (e: Exception) {
            Log.e(TAG, "could not acquire wake lock", e)
        }
    }

    private fun releaseWakeLock() {
        try {
            wakeLock?.takeIf { it.isHeld }?.release()
            Log.i(TAG, "partial wake lock released")
        } catch (e: Exception) {
            Log.w(TAG, "could not release wake lock", e)
        } finally {
            wakeLock = null
        }
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return

        val manager = getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL_ID) != null) return

        manager.createNotificationChannel(
            NotificationChannel(
                    CHANNEL_ID,
                    "Background transfers",
                    // LOW: permanently visible, so it must not make noise.
                    NotificationManager.IMPORTANCE_LOW,
                )
                .apply {
                    description = "Keeps LocalDrop reachable while it is in the background"
                    setShowBadge(false)
                }
        )
    }

    private fun buildNotification(detail: String): Notification {
        val open =
            packageManager.getLaunchIntentForPackage(packageName)?.let {
                PendingIntent.getActivity(
                    this,
                    0,
                    it,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
            }

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("LocalDrop active")
            .setContentText(detail)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setContentIntent(open)
            .build()
    }
}
