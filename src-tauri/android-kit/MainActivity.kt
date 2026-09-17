package app.localdrop

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  /**
   * Guards against a Settings loop: without it, a user who declines bounces
   * straight back to the Settings screen on every resume and cannot reach the
   * app at all.
   */
  private var requestedAllFilesAccess = false

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    // LocalDrop is a dark-only surface (#0F131C). Tell the system to draw
    // *light* status- and navigation-bar icons; the default assumes a light app
    // and renders dark icons plus a light scrim, which reads as a white bar
    // pasted over the UI.
    //
    // The webview still has to keep its content clear of these bars - see the
    // safe-area insets in src/style.css. enableEdgeToEdge() alone puts the
    // bottom nav underneath the system nav bar, where it cannot be tapped.
    WindowInsetsControllerCompat(window, window.decorView).apply {
      isAppearanceLightStatusBars = false
      isAppearanceLightNavigationBars = false
    }

    // AND-1: hold the Wi-Fi multicast lock for as long as the app is alive.
    // Without it the Rust discovery listener receives nothing, and the radar
    // stays empty with no error to explain why.
    LocalDropNetwork.acquire(this)
  }

  override fun onResume() {
    super.onResume()

    // All-files access cannot be requested with a runtime dialog; the user has
    // to enable it on a Settings screen. Checking here rather than in onCreate
    // means returning from that screen re-checks automatically.
    //
    // Without it, sending a folder silently transfers nothing: the directory
    // walk succeeds and then every file fails to open (see LocalDropStorage).
    if (!requestedAllFilesAccess && !LocalDropStorage.hasAllFilesAccess()) {
      requestedAllFilesAccess = true
      LocalDropStorage.requestAllFilesAccess(this)
    }
  }

  override fun onDestroy() {
    LocalDropNetwork.release()
    super.onDestroy()
  }
}
