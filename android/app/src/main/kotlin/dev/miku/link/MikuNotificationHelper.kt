package dev.miku.link

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.content.ContextCompat

object MikuNotificationHelper {
    private const val CHANNEL_ID = "miku_link_alerts"
    private const val CHANNEL_NAME = "Miku Phone Link Alertas"

    fun ensureChannel(context: Context) {
        val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        if (mgr.getNotificationChannel(CHANNEL_ID) != null) return
        val ch = NotificationChannel(
            CHANNEL_ID,
            CHANNEL_NAME,
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Respuestas de agentes, aprobaciones Gatekeeper y avisos de Miku PC"
            enableVibration(true)
            vibrationPattern = longArrayOf(0L, 120L, 80L, 160L)
        }
        mgr.createNotificationChannel(ch)
    }

    fun showAlert(
        context: Context,
        notifId: Int,
        title: String,
        body: String,
        highPriority: Boolean = false,
    ) {
        if (Build.VERSION.SDK_INT >= 33) {
            val granted = ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) return
        }
        ensureChannel(context)
        val mgr = context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
        val openIntent = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val pi = PendingIntent.getActivity(
            context,
            notifId,
            openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notif = Notification.Builder(context, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(title.ifBlank { "Miku Phone Link" })
            .setContentText(body.ifBlank { "Nueva actividad en Miku Desktop" })
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(pi)
            .setAutoCancel(true)
            .setCategory(if (highPriority) Notification.CATEGORY_ALARM else Notification.CATEGORY_MESSAGE)
            .build()
        mgr.notify(notifId, notif)
    }
}
