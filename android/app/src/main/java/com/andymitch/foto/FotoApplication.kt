package com.andymitch.foto

import android.app.Application
import android.content.Context
import android.database.ContentObserver
import android.graphics.Bitmap
import android.media.ThumbnailUtils
import android.net.ConnectivityManager
import android.net.Network
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Environment
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import android.util.Size
import com.andymitch.foto.core.Config
import com.andymitch.foto.core.Event
import com.andymitch.foto.core.EventSink
import com.andymitch.foto.core.Foto
import com.andymitch.foto.core.LibraryInfo
import com.andymitch.foto.core.Platform
import com.andymitch.foto.core.Status
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.io.ByteArrayOutputStream
import java.io.File

object Native {
    init {
        System.loadLibrary("foto_ffi")
    }

    @JvmStatic
    external fun initAndroidContext(context: Context)
}

class AndroidPlatform(private val context: Context) : Platform {
    override fun thumbnail(path: String, video: Boolean, maxPx: UInt): ByteArray? = runCatching {
        val size = Size(maxPx.toInt(), maxPx.toInt())
        val bmp = if (video) {
            ThumbnailUtils.createVideoThumbnail(File(path), size, null)
        } else {
            ThumbnailUtils.createImageThumbnail(File(path), size, null)
        }
        ByteArrayOutputStream().use { out ->
            bmp.compress(Bitmap.CompressFormat.JPEG, 80, out)
            bmp.recycle()
            out.toByteArray()
        }
    }.getOrNull()

    override fun trash(paths: List<String>): Boolean = true

    override fun isMetered(): Boolean =
        context.getSystemService(ConnectivityManager::class.java).isActiveNetworkMetered
}

class FotoApplication : Application(), EventSink {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    val foto = MutableStateFlow<Foto?>(null)
    val error = MutableStateFlow<String?>(null)
    val libraries = MutableStateFlow<List<LibraryInfo>>(emptyList())
    val library = MutableStateFlow<String?>(null)
    val version = MutableStateFlow(0)
    val status = MutableStateFlow<Status?>(null)
    private var multicast: WifiManager.MulticastLock? = null
    private val main = Handler(Looper.getMainLooper())
    private val rescan = Runnable { foto.value?.scan() }

    override fun onCreate() {
        super.onCreate()
        scope.launch {
            runCatching {
                Native.initAndroidContext(this@FotoApplication)
                val pictures = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_PICTURES)
                val config = Config(
                    dataDir = File(filesDir, "foto").absolutePath,
                    libraryDir = File(pictures, "foto").absolutePath,
                    deviceName = Build.MODEL,
                    platform = "android",
                    originalsAll = false,
                )
                Foto.open(config, AndroidPlatform(this@FotoApplication))
            }.onSuccess { f ->
                f.subscribe(this@FotoApplication)
                foto.value = f
                refreshLibraries()
                status.value = f.status()
                watchSystem()
            }.onFailure { error.value = it.message ?: it.toString() }
        }
        SyncWorker.schedule(this)
    }

    fun refreshLibraries() {
        val f = foto.value ?: return
        val libs = f.libraries()
        libraries.value = libs
        if (library.value == null || libs.none { it.id == library.value }) {
            library.value = libs.firstOrNull()?.id
        }
    }

    override fun onEvent(event: Event) {
        when (event) {
            is Event.LibraryChanged -> {
                refreshLibraries()
                version.update { it + 1 }
            }
            is Event.StatusChanged -> status.value = event.status
        }
    }

    private fun watchSystem() {
        val cm = getSystemService(ConnectivityManager::class.java)
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                foto.value?.networkChanged()
            }

            override fun onLost(network: Network) {
                foto.value?.networkChanged()
            }
        })
        val observer = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) {
                main.removeCallbacks(rescan)
                main.postDelayed(rescan, 3000)
            }
        }
        contentResolver.registerContentObserver(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true, observer)
        contentResolver.registerContentObserver(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, true, observer)
    }

    fun foreground(active: Boolean) {
        val wifi = applicationContext.getSystemService(WifiManager::class.java)
        if (active && multicast == null) {
            multicast = wifi.createMulticastLock("foto-mdns").apply {
                setReferenceCounted(false)
                acquire()
            }
        } else if (!active) {
            multicast?.release()
            multicast = null
        }
        if (active) {
            foto.value?.networkChanged()
            foto.value?.scan()
        }
    }

    fun addDefaultFolders(library: String) {
        val f = foto.value ?: return
        val camera = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DCIM), "Camera")
        if (camera.isDirectory) runCatching { f.addRoot(library, camera.absolutePath) }
    }

    fun busy(): Boolean {
        val s = status.value ?: return false
        return s.indexing > 0u || s.thumbsPending > 0u || s.originalsPending > 0u
    }
}

val Context.app: FotoApplication get() = applicationContext as FotoApplication
