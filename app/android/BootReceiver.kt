package org.goofyfiles.chat

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Restarts notifications after a reboot or an app update, when signed in. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED &&
            intent.action != Intent.ACTION_MY_PACKAGE_REPLACED
        ) return
        val prefs = context.getSharedPreferences(ChatService.PREFS, Context.MODE_PRIVATE)
        if (prefs.getString("token", "").isNullOrEmpty()) return
        // Some manufacturers refuse background starts; opening the app still works.
        runCatching { context.startForegroundService(Intent(context, ChatService::class.java)) }
    }
}
