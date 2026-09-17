package app.localdrop

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.Settings
import android.util.Log

/**
 * All-files access (`MANAGE_EXTERNAL_STORAGE`).
 *
 * # Why this is needed
 *
 * Under scoped storage the app can *list* directories but cannot *read* files
 * it did not create:
 *
 * ```text
 * list  /storage/emulated/0/DCIM         -> OK
 * read  /storage/emulated/0/DCIM/x.png   -> denied
 * ```
 *
 * Folder sending therefore fails in a way that looks like a bug rather than a
 * permission problem: the tree walks fine, a manifest is built, and then every
 * single file fails to open and is skipped. Single-file picks work only because
 * the picker grants a per-URI permission.
 *
 * # Why it is not a runtime dialog
 *
 * `MANAGE_EXTERNAL_STORAGE` cannot be requested with `requestPermissions()`.
 * The user has to enable it on a system Settings screen, so the flow is:
 * check -> send the user to Settings -> re-check on resume.
 *
 * # Play Store
 *
 * This permission requires written justification and may be rejected. If the
 * app is ever published, this is the piece to replace with SAF
 * (`DocumentFile`) traversal.
 */
object LocalDropStorage {
    private const val TAG = "LocalDropStorage"

    /** Whether the app can currently read arbitrary user files. */
    @JvmStatic
    fun hasAllFilesAccess(): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            // Pre-API 30 the legacy READ_EXTERNAL_STORAGE grant is enough, and
            // it is requested through the ordinary runtime flow.
            true
        }

    /**
     * Open the system screen where all-files access is granted.
     *
     * Falls back to the global list if the per-app screen is unavailable, which
     * happens on some OEM builds.
     */
    @JvmStatic
    fun requestAllFilesAccess(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R || hasAllFilesAccess()) {
            return
        }

        val perApp =
            Intent(
                Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                Uri.parse("package:${context.packageName}"),
            )
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

        try {
            context.startActivity(perApp)
        } catch (e: Exception) {
            Log.w(TAG, "per-app all-files screen unavailable, falling back", e)
            try {
                context.startActivity(
                    Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                )
            } catch (e2: Exception) {
                Log.e(TAG, "cannot open all-files settings", e2)
            }
        }
    }
}
