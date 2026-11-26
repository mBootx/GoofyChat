package org.goofyfiles.chat

import android.app.Activity
import android.app.AlertDialog
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.os.SystemClock
import android.provider.Settings
import android.widget.Toast
import okhttp3.OkHttpClient
import okhttp3.Request
import org.json.JSONObject
import kotlin.concurrent.thread

/**
 * Offers the APK of the latest GitHub release when it is newer than the app.
 * Android only installs it when it is signed with the same key as this app.
 */
class Updater(private val activity: Activity) {
    private companion object {
        const val LATEST = "https://api.github.com/repos/mBootx/GoofyChat/releases/latest"
        const val INTERVAL_MS = 6 * 3600 * 1000L
    }

    private class Release(val version: String, val url: String)

    private val client = OkHttpClient()
    private var lastCheck: Long? = null
    private var downloading = false
    /** Waiting for the user to allow installs from this app. */
    private var awaiting: Release? = null

    /** Called each time the app comes to the screen. */
    fun resume() {
        awaiting?.let {
            awaiting = null
            if (activity.packageManager.canRequestPackageInstalls()) download(it)
            return
        }
        val now = SystemClock.elapsedRealtime()
        if (downloading || lastCheck?.let { now - it < INTERVAL_MS } == true) return
        lastCheck = now
        thread {
            // Offline, no release yet or rate limited: try again another time.
            val release = runCatching { latest() }.getOrNull() ?: return@thread
            activity.runOnUiThread { offer(release) }
        }
    }

    private fun latest(): Release? {
        val request = Request.Builder().url(LATEST)
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "GoofyChat")
            .build()
        client.newCall(request).execute().use { response ->
            if (!response.isSuccessful) return null
            val json = JSONObject(response.body!!.string())
            val version = json.getString("tag_name").removePrefix("v")
            val installed = activity.packageManager.getPackageInfo(activity.packageName, 0).versionName ?: return null
            if (!newer(version, installed)) return null
            val assets = json.getJSONArray("assets")
            for (i in 0 until assets.length()) {
                val asset = assets.getJSONObject(i)
                if (asset.getString("name").endsWith(".apk")) {
                    return Release(version, asset.getString("browser_download_url"))
                }
            }
            return null
        }
    }

    /** Compares dotted numbers, e.g. 0.10.0 is newer than 0.9.1. */
    private fun newer(a: String, b: String): Boolean {
        val left = a.split(".").map { it.toIntOrNull() ?: return false }
        val right = b.split(".").map { it.toIntOrNull() ?: 0 }
        for (i in 0 until maxOf(left.size, right.size)) {
            val difference = left.getOrElse(i) { 0 } - right.getOrElse(i) { 0 }
            if (difference != 0) return difference > 0
        }
        return false
    }

    private fun offer(release: Release) {
        if (activity.isFinishing || activity.isDestroyed) return
        AlertDialog.Builder(activity)
            .setTitle("Mise à jour disponible")
            .setMessage("La version ${release.version} de Goofy Chat est disponible.")
            .setPositiveButton("Installer") { _, _ -> install(release) }
            .setNegativeButton("Plus tard", null)
            .show()
    }

    private fun install(release: Release) {
        if (activity.packageManager.canRequestPackageInstalls()) return download(release)
        awaiting = release
        toast("Autorisez Goofy Chat à installer des applications, puis revenez.")
        activity.startActivity(
            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}"))
        )
    }

    private fun toast(text: String) = Toast.makeText(activity, text, Toast.LENGTH_LONG).show()

    private fun download(release: Release) {
        if (downloading) return
        downloading = true
        toast("Téléchargement de la mise à jour…")
        thread {
            val done = runCatching { stream(release.url) }.isSuccess
            activity.runOnUiThread {
                downloading = false
                if (!done) toast("Échec du téléchargement de la mise à jour.")
            }
        }
    }

    /** Downloads straight into an install session, then asks Android to install it. */
    private fun stream(url: String) {
        val installer = activity.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setAppPackageName(activity.packageName)
        val id = installer.createSession(params)
        try {
            installer.openSession(id).use { session ->
                client.newCall(Request.Builder().url(url).header("User-Agent", "GoofyChat").build())
                    .execute().use { response ->
                        check(response.isSuccessful)
                        val body = response.body!!
                        session.openWrite("update.apk", 0, body.contentLength()).use { out ->
                            body.byteStream().copyTo(out)
                            session.fsync(out)
                        }
                    }
                // Android fills in the result, so the intent must be mutable.
                var flags = PendingIntent.FLAG_UPDATE_CURRENT
                if (Build.VERSION.SDK_INT >= 31) flags = flags or PendingIntent.FLAG_MUTABLE
                val result = PendingIntent.getBroadcast(
                    activity, id, Intent(activity, InstallReceiver::class.java), flags
                )
                session.commit(result.intentSender)
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(id) }
            throw e
        }
    }
}

/** Shows Android's confirmation screen, or reports a failed install. */
class InstallReceiver : BroadcastReceiver() {
    @Suppress("DEPRECATION")
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT) ?: return
                context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
            PackageInstaller.STATUS_SUCCESS, PackageInstaller.STATUS_FAILURE_ABORTED -> {}
            else -> Toast.makeText(context, "La mise à jour n'a pas pu être installée.", Toast.LENGTH_LONG).show()
        }
    }
}
