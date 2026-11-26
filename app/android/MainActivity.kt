package org.goofyfiles.chat

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.OnBackPressedCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
    companion object {
        /** Read by ChatService: no notifications while the app is on screen. */
        @Volatile var visible = false
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Keep the page clear of the system bars and the keyboard.
        val root = findViewById<View>(android.R.id.content)
        root.setBackgroundColor(0xFF191923.toInt())
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime()
            )
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }

    override fun onWebViewCreate(webView: WebView) {
        webView.settings.mediaPlaybackRequiresUserGesture = false
        webView.addJavascriptInterface(Bridge(), "GoofyNative")
        // Back on the first screen must not destroy the process and its service.
        webView.post {
            onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
                override fun handleOnBackPressed() {
                    if (webView.canGoBack()) webView.goBack() else moveTaskToBack(true)
                }
            })
        }
    }

    private val updater by lazy { Updater(this) }

    override fun onResume() {
        super.onResume()
        visible = true
        updater.resume()
    }

    override fun onPause() {
        visible = false
        super.onPause()
    }

    /** Without it Android delays notifications while the phone sleeps. Asked once. */
    private fun askBatteryExemption() {
        val prefs = getSharedPreferences("ui", MODE_PRIVATE)
        val power = getSystemService(PowerManager::class.java)
        if (prefs.getBoolean("askedBattery", false) || power.isIgnoringBatteryOptimizations(packageName)) return
        prefs.edit().putBoolean("askedBattery", true).apply()
        runCatching {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
            )
        }
    }

    /** Called from the web page. */
    inner class Bridge {
        @JavascriptInterface
        fun start(server: String, token: String, userId: String) {
            getSharedPreferences(ChatService.PREFS, MODE_PRIVATE).edit()
                .putString("server", server)
                .putString("token", token)
                .putString("userId", userId)
                .apply()
            val intent = Intent(this@MainActivity, ChatService::class.java)
            runOnUiThread {
                startForegroundService(intent)
                askBatteryExemption()
            }
        }

        @JavascriptInterface
        fun call(active: Boolean, video: Boolean) {
            val intent = Intent(this@MainActivity, ChatService::class.java)
                .putExtra("call", active)
                .putExtra("video", video)
            runOnUiThread { startForegroundService(intent) }
        }

        @JavascriptInterface
        fun stop() {
            getSharedPreferences(ChatService.PREFS, MODE_PRIVATE).edit().clear().apply()
            stopService(Intent(this@MainActivity, ChatService::class.java))
        }
    }
}
