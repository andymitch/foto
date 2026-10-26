package com.andymitch.foto

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import androidx.core.app.NotificationCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import kotlinx.coroutines.delay
import java.util.concurrent.TimeUnit

class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = applicationContext.app
        repeat(60) {
            if (app.foto.value != null) return@repeat
            delay(500)
        }
        val foto = app.foto.value ?: return Result.retry()
        foto.networkChanged()
        foto.scan()
        delay(20_000)
        val deadline = System.currentTimeMillis() + 9 * 60_000
        var announced = false
        while (System.currentTimeMillis() < deadline && app.busy()) {
            if (!announced) {
                runCatching { setForeground(foregroundInfo()) }
                announced = true
            }
            delay(5_000)
        }
        return Result.success()
    }

    override suspend fun getForegroundInfo(): ForegroundInfo = foregroundInfo()

    private fun foregroundInfo(): ForegroundInfo {
        val nm = applicationContext.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Sync", NotificationManager.IMPORTANCE_LOW))
        val s = applicationContext.app.status.value
        val text = when {
            s == null -> "Syncing"
            s.indexing > 0u -> "Importing ${s.indexing} files"
            s.thumbsPending > 0u -> "Fetching ${s.thumbsPending} previews"
            s.originalsPending > 0u -> "Downloading ${s.originalsPending} originals"
            else -> "Syncing"
        }
        val n = NotificationCompat.Builder(applicationContext, CHANNEL)
            .setSmallIcon(R.drawable.ic_foto)
            .setContentTitle("foto")
            .setContentText(text)
            .setOngoing(true)
            .build()
        return ForegroundInfo(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    }

    companion object {
        private const val CHANNEL = "sync"
        private val network = Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

        fun schedule(context: Context) {
            val periodic = PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES).setConstraints(network).build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork("sync", ExistingPeriodicWorkPolicy.KEEP, periodic)
        }

        fun now(context: Context) {
            val once = OneTimeWorkRequestBuilder<SyncWorker>()
                .setConstraints(network)
                .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork("sync-now", ExistingWorkPolicy.REPLACE, once)
        }
    }
}
