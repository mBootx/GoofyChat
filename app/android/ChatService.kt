package org.goofyfiles.chat

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * Keeps its own connection to the server so messages and calls are announced
 * while the app is closed, without any third-party push service.
 */
class ChatService : Service() {
    companion object {
        const val PREFS = "goofychat"
        private const val STATUS_ID = 1
        private const val CALL_ID = 2
    }

    private val client = OkHttpClient.Builder().pingInterval(30, TimeUnit.SECONDS).build()
    private val handler = Handler(Looper.getMainLooper())
    private var socket: WebSocket? = null
    private var token = ""
    private var delay = 2000L
    private var inCall = false
    private var videoCall = false

    private val heartbeat = object : Runnable {
        override fun run() {
            // The server drops connections that stay silent.
            socket?.send("{\"type\":\"ping\"}")
            handler.postDelayed(this, 30_000)
        }
    }
    private val reconnect = Runnable { connect() }

    private val manager get() = getSystemService(NotificationManager::class.java)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        // Android requires this notification to keep the connection. The user can
        // switch the channel off: the service keeps running without it.
        manager.createNotificationChannel(
            NotificationChannel("status", "Connexion en arrière-plan", NotificationManager.IMPORTANCE_MIN)
        )
        manager.createNotificationChannel(
            NotificationChannel("messages", "Messages", NotificationManager.IMPORTANCE_HIGH)
        )
        manager.createNotificationChannel(
            NotificationChannel("mentions", "Mentions", NotificationManager.IMPORTANCE_HIGH)
        )
        val calls = NotificationChannel("calls", "Appels", NotificationManager.IMPORTANCE_HIGH)
        calls.setSound(
            RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
            AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE).build()
        )
        calls.enableVibration(true)
        manager.createNotificationChannel(calls)
        handler.postDelayed(heartbeat, 30_000)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Tapping opens the switch that hides this notification for good.
        val hide = PendingIntent.getActivity(
            this, 1,
            Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, packageName)
                .putExtra(Settings.EXTRA_CHANNEL_ID, "status")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE
        )
        val status = Notification.Builder(this, "status")
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle("Goofy Chat")
            .setContentText("Touchez pour masquer cette notification")
            .setContentIntent(hide)
            .setShowWhen(false)
            .build()
        if (intent?.hasExtra("call") == true) {
            inCall = intent.getBooleanExtra("call", false)
            videoCall = intent.getBooleanExtra("video", false)
        }
        if (Build.VERSION.SDK_INT >= 30) {
            var types = 0
            if (Build.VERSION.SDK_INT >= 34) types = ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
            if (inCall) {
                types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
                if (videoCall) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
            }
            runCatching { startForeground(STATUS_ID, status, types) }.onFailure {
                val fallback = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
                startForeground(STATUS_ID, status, fallback)
            }
        } else {
            startForeground(STATUS_ID, status)
        }
        val saved = getSharedPreferences(PREFS, MODE_PRIVATE).getString("token", "") ?: ""
        if (saved.isEmpty()) {
            stopSelf()
            return START_NOT_STICKY
        }
        if (saved != token || socket == null) {
            token = saved
            connect()
        }
        return START_STICKY
    }

    private fun connect() {
        handler.removeCallbacks(reconnect)
        socket?.cancel()
        val server = getSharedPreferences(PREFS, MODE_PRIVATE).getString("server", "") ?: ""
        if (server.isEmpty() || token.isEmpty()) return
        val request = Request.Builder().url("$server/api/ws").build()
        socket = client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                webSocket.send(JSONObject().put("type", "auth").put("token", token).toString())
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                handler.post {
                    if (webSocket === socket) {
                        delay = 2000L
                        runCatching { handle(JSONObject(text)) }
                    }
                }
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = lost(webSocket)

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) = lost(webSocket)
        })
    }

    private fun lost(webSocket: WebSocket) {
        handler.post {
            if (webSocket !== socket) return@post
            socket = null
            handler.removeCallbacks(reconnect)
            handler.postDelayed(reconnect, delay)
            delay = (delay * 2).coerceAtMost(60_000)
        }
    }

    private fun openApp(): PendingIntent = PendingIntent.getActivity(
        this, 0,
        Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )

    private fun handle(event: JSONObject) {
        val me = getSharedPreferences(PREFS, MODE_PRIVATE).getString("userId", "")
        when (event.optString("type")) {
            "message" -> {
                val message = event.getJSONObject("message")
                val sender = message.getJSONObject("sender")
                if (MainActivity.visible || sender.getString("id") == me) return
                val senderName = sender.getString("display_name")
                val body = message.optString("body").ifEmpty { "Pièce jointe" }
                val myId = me?.toLongOrNull()
                val mentions = message.optJSONArray("mentions")
                val mentioned = myId != null && mentions != null &&
                    (0 until mentions.length()).any { mentions.optLong(it, -1L) == myId }
                val channel = if (mentioned) "mentions" else "messages"
                val title = if (mentioned) "$senderName vous a mentionné" else senderName
                val notification = Notification.Builder(this, channel)
                    .setSmallIcon(android.R.drawable.stat_notify_chat)
                    .setContentTitle(title)
                    .setContentText(body)
                    .setContentIntent(openApp())
                    .setAutoCancel(true)
                    .build()
                val conversationId = message.getInt("conversation_id")
                if (mentioned) {
                    manager.notify("mention", conversationId, notification)
                } else {
                    manager.notify(1000 + conversationId, notification)
                }
            }
            "friends" -> {
                if (MainActivity.visible) return
                val notice = event.optJSONObject("notice") ?: return
                if (notice.optString("kind") != "request") return
                val user = notice.optJSONObject("user") ?: return
                val name = user.optString("display_name").ifEmpty { return }
                val notification = Notification.Builder(this, "messages")
                    .setSmallIcon(android.R.drawable.stat_notify_chat)
                    .setContentTitle("Demande d'ami")
                    .setContentText("$name veut devenir votre ami.")
                    .setContentIntent(openApp())
                    .setAutoCancel(true)
                    .build()
                manager.notify("friend", user.optInt("id"), notification)
            }
            "call" -> {
                val from = event.getJSONObject("from")
                when (event.getJSONObject("signal").getString("action")) {
                    "invite" -> {
                        if (MainActivity.visible) return
                        val notification = Notification.Builder(this, "calls")
                            .setSmallIcon(android.R.drawable.stat_sys_phone_call)
                            .setContentTitle(from.getString("display_name"))
                            .setContentText("Appel entrant, touchez pour répondre")
                            .setCategory(Notification.CATEGORY_CALL)
                            .setContentIntent(openApp())
                            .setAutoCancel(true)
                            .setTimeoutAfter(45_000)
                            .build()
                        notification.flags = notification.flags or Notification.FLAG_INSISTENT
                        manager.notify(CALL_ID, notification)
                    }
                    "accept", "reject", "hangup" -> manager.cancel(CALL_ID)
                }
            }
        }
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        socket?.cancel()
        socket = null
        super.onDestroy()
    }
}
