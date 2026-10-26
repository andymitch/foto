package com.andymitch.foto

import android.Manifest
import android.content.ContentUris
import android.net.Uri
import android.os.Bundle
import android.provider.MediaStore
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File

class MainActivity : ComponentActivity() {
    private var trashing: List<String> = emptyList()

    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        app.foto.value?.scan()
    }

    private val trashRequest = registerForActivityResult(ActivityResultContracts.StartIntentSenderForResult()) { result ->
        val paths = trashing
        trashing = emptyList()
        val ok = result.resultCode == RESULT_OK
        lifecycleScope.launch(Dispatchers.IO) { app.foto.value?.trashDone(paths, ok) }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        permissions.launch(
            arrayOf(
                Manifest.permission.READ_MEDIA_IMAGES,
                Manifest.permission.READ_MEDIA_VIDEO,
                Manifest.permission.ACCESS_MEDIA_LOCATION,
                Manifest.permission.POST_NOTIFICATIONS,
            )
        )
        lifecycleScope.launch {
            app.version.collect { confirmTrash() }
        }
        setContent {
            val dark = isSystemInDarkTheme()
            val context = LocalContext.current
            val colors = if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
            MaterialTheme(colorScheme = colors) { Surface(Modifier.fillMaxSize()) { FotoUi() } }
        }
    }

    override fun onStart() {
        super.onStart()
        app.foreground(true)
    }

    override fun onStop() {
        super.onStop()
        app.foreground(false)
        if (app.busy()) SyncWorker.now(this)
    }

    private suspend fun confirmTrash() {
        if (trashing.isNotEmpty()) return
        val foto = app.foto.value ?: return
        val paths = withContext(Dispatchers.IO) { foto.pendingTrash() }
        if (paths.isEmpty()) return
        val (uris, unknown) = withContext(Dispatchers.IO) { mediaUris(paths) }
        if (unknown.isNotEmpty()) {
            withContext(Dispatchers.IO) {
                unknown.forEach { File(it).delete() }
                foto.trashDone(unknown, true)
            }
        }
        if (uris.isEmpty()) return
        trashing = uris.keys.toList()
        val request = MediaStore.createTrashRequest(contentResolver, uris.values.toList(), true)
        trashRequest.launch(IntentSenderRequest.Builder(request.intentSender).build())
    }

    private fun mediaUris(paths: List<String>): Pair<Map<String, Uri>, List<String>> {
        val found = mutableMapOf<String, Uri>()
        val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        for (path in paths) {
            contentResolver.query(
                collection,
                arrayOf(MediaStore.MediaColumns._ID),
                "${MediaStore.MediaColumns.DATA} = ?",
                arrayOf(path),
                null,
            )?.use { c ->
                if (c.moveToFirst()) found[path] = ContentUris.withAppendedId(collection, c.getLong(0))
            }
        }
        return found to paths.filter { it !in found }
    }
}
