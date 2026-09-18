package app.localdrop

import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  /**
   * Guards against a Settings loop: without it, a user who declines bounces
   * straight back to the Settings screen on every resume and cannot reach the
   * app at all.
   */
  private var requestedAllFilesAccess = false

  /**
   * Last keyboard height published to CSS, in CSS pixels.
   *
   * The inset listener fires on every frame of the keyboard's slide animation.
   * Without this the app would run an `evaluateJavascript` per frame, so the
   * value is only pushed across when it actually changes.
   */
  private var lastKeyboardCssPx = -1

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

  /**
   * Publish the soft keyboard's height to CSS as `--keyboard-h`.
   *
   * Without this a bottom-anchored sheet — the Quick text composer — sits
   * underneath the keyboard the moment the textarea takes focus, so the user
   * cannot see what they are typing or reach Paste clipboard to add more.
   *
   * `android:windowSoftInputMode="adjustResize"` does not solve it on its own.
   * `enableEdgeToEdge()` calls `setDecorFitsSystemWindows(window, false)`, and
   * from API 30 that makes the framework stop resizing the window for the IME
   * — the webview keeps its full height, never learns the keyboard exists, and
   * `window.visualViewport` inside it does not change either. So the inset has
   * to be read here and handed to the web layer explicitly.
   *
   * Note the delegation at the end, which is not optional. Setting a listener
   * *replaces* a view's own inset policy — `View.onApplyWindowInsets` is no
   * longer called for it — and the WebView's policy is exactly what Chromium
   * derives `env(safe-area-inset-*)` from. Returning `insets` directly would
   * have zeroed those, and `src/style.css` builds the header and bottom-nav
   * padding out of them, so the navigation would have slid back under the
   * system bar: AND-2's original bug, reintroduced while fixing the keyboard.
   * `ViewCompat.onApplyWindowInsets` runs that default policy explicitly and
   * hands back its result. It dispatches to the view's own implementation
   * rather than to this listener, so there is no recursion.
   */
  override fun onWebViewCreate(webView: WebView) {
    ViewCompat.setOnApplyWindowInsetsListener(webView) { view, insets ->
      val imePx = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom

      // Framework insets are physical pixels; CSS wants density-independent
      // ones. Skipping this division overshoots by the display's scale factor
      // -- roughly 3x on a modern phone -- and pushes the sheet off the top.
      val cssPx = (imePx / resources.displayMetrics.density).toInt()

      if (cssPx != lastKeyboardCssPx) {
        lastKeyboardCssPx = cssPx
        // Set as an inline style on <html>, which outranks the `:root` default
        // in style.css.
        webView.evaluateJavascript(
          "document.documentElement.style.setProperty('--keyboard-h','${cssPx}px')",
          null,
        )
      }

      ViewCompat.onApplyWindowInsets(view, insets)
    }
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
