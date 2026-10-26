package com.andymitch.foto

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.media.MediaScannerConnection
import android.os.Environment
import android.provider.MediaStore
import android.util.LruCache
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.FavoriteBorder
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.PhotoLibrary
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Restore
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import com.andymitch.foto.core.AssetSummary
import com.andymitch.foto.core.Filter
import com.andymitch.foto.core.Foto
import com.andymitch.foto.core.Kind
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.time.Instant
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter

enum class Tab { Photos, Favorites, Deleted, Folders, Devices, Settings }

object Thumbs {
    val cache = object : LruCache<String, ImageBitmap>(64 * 1024 * 1024) {
        override fun sizeOf(key: String, value: ImageBitmap) = value.width * value.height * 4
    }
}

fun date(ts: Long, pattern: String): String =
    DateTimeFormatter.ofPattern(pattern).format(Instant.ofEpochSecond(ts).atOffset(ZoneOffset.UTC))

fun duration(ms: ULong): String {
    val s = (ms / 1000u).toLong()
    return "%d:%02d".format(s / 60, s % 60)
}

suspend fun <T> io(block: () -> T): T = withContext(Dispatchers.IO) { block() }

@Composable
fun FotoUi() {
    val app = LocalContext.current.app
    val foto by app.foto.collectAsState()
    val error by app.error.collectAsState()
    val library by app.library.collectAsState()
    val f = foto
    when {
        error != null -> Centered("foto could not start: $error")
        f == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        library == null -> Onboarding(f)
        else -> Main(f, library!!)
    }
}

@Composable
fun Centered(text: String) {
    Box(Modifier.fillMaxSize().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
fun rememberScanner(onTicket: (String) -> Unit) =
    rememberLauncherForActivityResult(ScanContract()) { result -> result.contents?.let(onTicket) }

fun scanOptions(): ScanOptions = ScanOptions()
    .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
    .setPrompt("Scan the code shown in foto on your computer")
    .setBeepEnabled(false)
    .setOrientationLocked(false)

@Composable
fun Onboarding(foto: Foto) {
    val app = LocalContext.current.app
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf("") }
    var ticket by remember { mutableStateOf("") }
    var message by remember { mutableStateOf<String?>(null) }
    fun join(t: String) = scope.launch {
        runCatching { io { foto.join(t.trim()) } }
            .onSuccess { app.addDefaultFolders(it.id); app.refreshLibraries() }
            .onFailure { message = it.message }
    }
    val scanner = rememberScanner { join(it) }
    Column(
        Modifier.fillMaxSize().statusBarsPadding().padding(24.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Spacer(Modifier.height(24.dp))
        Text("Welcome to foto", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Your photos stay on your devices and sync directly between them.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Join the library on your computer", fontWeight = FontWeight.SemiBold)
                Text(
                    "On your computer open Devices, choose Invite one of my devices, then scan the code.",
                    style = MaterialTheme.typography.bodySmall,
                )
                Button(onClick = { scanner.launch(scanOptions()) }) {
                    Icon(Icons.Default.QrCodeScanner, null)
                    Spacer(Modifier.width(8.dp))
                    Text("Scan code")
                }
                OutlinedTextField(ticket, { ticket = it }, label = { Text("Or paste a ticket") }, modifier = Modifier.fillMaxWidth())
                OutlinedButton(onClick = { join(ticket) }, enabled = ticket.isNotBlank()) { Text("Join") }
            }
        }
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Start a new library", fontWeight = FontWeight.SemiBold)
                OutlinedTextField(name, { name = it }, label = { Text("Library name") }, modifier = Modifier.fillMaxWidth())
                OutlinedButton(onClick = {
                    scope.launch {
                        runCatching { io { foto.createLibrary(name.trim()) } }
                            .onSuccess { app.addDefaultFolders(it.id); app.refreshLibraries() }
                            .onFailure { message = it.message }
                    }
                }, enabled = name.isNotBlank()) { Text("Create") }
            }
        }
        message?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}

@Composable
fun Main(foto: Foto, library: String) {
    var tab by remember { mutableStateOf(Tab.Photos) }
    var viewing by remember { mutableStateOf<Pair<List<AssetSummary>, Int>?>(null) }
    val snackbar = remember { SnackbarHostState() }
    val v = viewing
    if (v != null) {
        Viewer(foto, library, v.first, v.second, tab == Tab.Deleted, snackbar) { viewing = null }
        return
    }
    Scaffold(
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            NavigationBar {
                listOf(
                    Triple(Tab.Photos, Icons.Default.PhotoLibrary, "Photos"),
                    Triple(Tab.Folders, Icons.Default.Folder, "Folders"),
                    Triple(Tab.Devices, Icons.Default.Devices, "Devices"),
                    Triple(Tab.Settings, Icons.Default.Settings, "Settings"),
                ).forEach { (t, icon, label) ->
                    val selected = tab == t || (t == Tab.Photos && tab in listOf(Tab.Favorites, Tab.Deleted))
                    NavigationBarItem(selected, { tab = t }, { Icon(icon, null) }, label = { Text(label) })
                }
            }
        },
    ) { pad ->
        Box(Modifier.padding(pad)) {
            when (tab) {
                Tab.Photos, Tab.Favorites, Tab.Deleted -> Grid(foto, library, tab, { tab = it }) { items, i -> viewing = items to i }
                Tab.Folders -> Folders(foto, library)
                Tab.Devices -> Devices(foto, library, snackbar)
                Tab.Settings -> Settings(foto, library)
            }
        }
    }
}

@Composable
fun StatusLine() {
    val status by LocalContext.current.app.status.collectAsState()
    val s = status ?: return
    val text = when {
        s.paused != null -> s.paused!!
        s.indexing > 0u -> "Importing ${s.indexing} files"
        s.thumbsPending > 0u -> "Fetching ${s.thumbsPending} previews"
        s.originalsPending > 0u -> "Downloading ${s.originalsPending} originals"
        else -> "Up to date"
    }
    val peers = when (s.peersOnline) {
        0uL -> "no devices online"
        1uL -> "1 device online"
        else -> "${s.peersOnline} devices online"
    }
    Text("$text · $peers", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

private sealed interface Cell {
    data class Header(val label: String) : Cell
    data class Item(val index: Int, val asset: AssetSummary) : Cell
}

@Composable
fun Grid(foto: Foto, library: String, tab: Tab, onTab: (Tab) -> Unit, open: (List<AssetSummary>, Int) -> Unit) {
    val app = LocalContext.current.app
    val version by app.version.collectAsState()
    val filter = when (tab) {
        Tab.Favorites -> Filter.FAVORITES
        Tab.Deleted -> Filter.DELETED
        else -> Filter.ALL
    }
    val items by produceState<List<AssetSummary>?>(null, library, filter, version) {
        value = io { foto.timeline(library, filter) }
    }
    val cells = remember(items) {
        buildList {
            var month = ""
            items.orEmpty().forEachIndexed { i, a ->
                val m = date(a.takenAt, "MMMM yyyy")
                if (m != month) {
                    month = m
                    add(Cell.Header(m))
                }
                add(Cell.Item(i, a))
            }
        }
    }
    Column(Modifier.fillMaxSize()) {
        Column(Modifier.statusBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(tab == Tab.Photos, { onTab(Tab.Photos) }, { Text("All") })
                FilterChip(tab == Tab.Favorites, { onTab(Tab.Favorites) }, { Text("Favourites") })
                FilterChip(tab == Tab.Deleted, { onTab(Tab.Deleted) }, { Text("Recently deleted") })
            }
            StatusLine()
        }
        when {
            items == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            items!!.isEmpty() -> Centered(
                when (tab) {
                    Tab.Favorites -> "No favourites yet."
                    Tab.Deleted -> "Nothing deleted in the last 30 days."
                    else -> "No photos yet. Pick folders to back up, or wait for your other devices to sync."
                }
            )
            else -> LazyVerticalGrid(
                columns = GridCells.Adaptive(96.dp),
                contentPadding = PaddingValues(2.dp),
                horizontalArrangement = Arrangement.spacedBy(2.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                items(cells, key = { c -> if (c is Cell.Header) "h-${c.label}" else (c as Cell.Item).asset.hash },
                    span = { c -> if (c is Cell.Header) GridItemSpan(maxLineSpan) else GridItemSpan(1) }) { c ->
                    when (c) {
                        is Cell.Header -> Text(
                            c.label,
                            fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.padding(start = 12.dp, top = 16.dp, bottom = 6.dp),
                        )
                        is Cell.Item -> Tile(foto, library, c.asset, version) { open(items!!, c.index) }
                    }
                }
            }
        }
    }
}

@Composable
fun thumbnail(foto: Foto, library: String, hash: String, large: Boolean, version: Int): ImageBitmap? {
    val key = "$hash-$large"
    val img by produceState(Thumbs.cache.get(key), key, version) {
        if (value == null) {
            value = io {
                foto.thumbnail(library, hash, large)
                    ?.let { BitmapFactory.decodeByteArray(it, 0, it.size) }
                    ?.asImageBitmap()
                    ?.also { Thumbs.cache.put(key, it) }
            }
        }
    }
    return img
}

@Composable
fun Tile(foto: Foto, library: String, a: AssetSummary, version: Int, onClick: () -> Unit) {
    val img = thumbnail(foto, library, a.hash, false, version)
    Box(
        Modifier.aspectRatio(1f).background(MaterialTheme.colorScheme.surfaceVariant).clickable(onClick = onClick)
    ) {
        img?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
        if (a.kind == Kind.VIDEO) Badge(duration(a.durationMs), Alignment.BottomEnd)
        if (a.live) Badge("LIVE", Alignment.TopStart)
        if (a.favorite) Icon(Icons.Default.Favorite, null, Modifier.align(Alignment.BottomStart).padding(4.dp).size(16.dp), tint = Color.White)
        if (!a.local) Box(Modifier.align(Alignment.TopEnd).padding(5.dp).size(6.dp).clip(CircleShape).background(Color.White.copy(alpha = 0.8f)))
    }
}

@Composable
fun androidx.compose.foundation.layout.BoxScope.Badge(text: String, align: Alignment) {
    Text(
        text,
        color = Color.White,
        fontSize = 11.sp,
        modifier = Modifier.align(align).padding(4.dp).background(Color.Black.copy(alpha = 0.5f), RoundedCornerShape(4.dp)).padding(horizontal = 4.dp),
    )
}

fun decodeOriginal(path: String): ImageBitmap? = runCatching {
    ImageDecoder.decodeBitmap(ImageDecoder.createSource(File(path))) { decoder, info, _ ->
        val longest = maxOf(info.size.width, info.size.height)
        var sample = 1
        while (longest / sample > 4096) sample *= 2
        decoder.setTargetSampleSize(sample)
        decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
    }.asImageBitmap()
}.getOrNull()

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun Viewer(
    foto: Foto,
    library: String,
    items: List<AssetSummary>,
    start: Int,
    deletedView: Boolean,
    snackbar: SnackbarHostState,
    close: () -> Unit,
) {
    BackHandler(onBack = close)
    val context = LocalContext.current
    val app = context.app
    val scope = rememberCoroutineScope()
    val version by app.version.collectAsState()
    val pager = rememberPagerState(start) { items.size }
    var zoomed by remember { mutableStateOf(false) }
    var info by remember { mutableStateOf(false) }
    val current = items.getOrNull(pager.currentPage) ?: return close()
    val detail by produceState(foto.asset(library, current.hash), current.hash, version) {
        value = io { foto.asset(library, current.hash) }
    }
    Box(Modifier.fillMaxSize().background(Color.Black)) {
        HorizontalPager(pager, userScrollEnabled = !zoomed, key = { items[it].hash }) { page ->
            val a = items[page]
            val large = thumbnail(foto, library, a.hash, true, version)
            val original by produceState<ImageBitmap?>(null, a.hash, version) {
                if (a.kind == Kind.PHOTO) value = io { foto.original(library, a.hash)?.let(::decodeOriginal) }
            }
            var scale by remember { mutableFloatStateOf(1f) }
            var offset by remember { mutableStateOf(Offset.Zero) }
            val state = rememberTransformableState { zoom, pan, _ ->
                scale = (scale * zoom).coerceIn(1f, 8f)
                offset = if (scale == 1f) Offset.Zero else offset + pan
                zoomed = scale > 1f
            }
            Box(
                Modifier.fillMaxSize()
                    .pointerInput(a.hash) {
                        detectTapGestures(onDoubleTap = {
                            scale = if (scale > 1f) 1f else 2.5f
                            offset = Offset.Zero
                            zoomed = scale > 1f
                        })
                    }
                    .transformable(state, canPan = { scale > 1f }),
                contentAlignment = Alignment.Center,
            ) {
                (original ?: large)?.let {
                    Image(
                        it, null,
                        Modifier.fillMaxSize().graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
                        contentScale = ContentScale.Fit,
                    )
                }
                if (a.kind == Kind.VIDEO) {
                    IconButton(
                        onClick = {
                            scope.launch {
                                val path = io { foto.original(library, a.hash) }
                                if (path == null) {
                                    snackbar.showSnackbar("Downloading the video, try again in a moment")
                                } else {
                                    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", File(path))
                                    context.startActivity(
                                        Intent(Intent.ACTION_VIEW).setDataAndType(uri, "video/*").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                                    )
                                }
                            }
                        },
                        modifier = Modifier.size(72.dp).background(Color.Black.copy(alpha = 0.5f), CircleShape),
                    ) { Icon(Icons.Default.PlayArrow, "Play", tint = Color.White, modifier = Modifier.size(40.dp)) }
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().background(Color.Black.copy(alpha = 0.55f)).statusBarsPadding().padding(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = close) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White) }
            Text(
                date(current.takenAt, "d MMM yyyy, HH:mm"),
                color = Color.White,
                modifier = Modifier.weight(1f),
                maxLines = 1,
            )
            val d = detail
            if (d != null && !current.local && d.path == null && !deletedView) {
                IconButton(onClick = {
                    scope.launch {
                        val saved = io { foto.saveOriginal(library, current.hash) }
                        if (saved == null) {
                            snackbar.showSnackbar("Downloading, try again in a moment")
                        } else {
                            MediaScannerConnection.scanFile(context, arrayOf(saved), null, null)
                            snackbar.showSnackbar("Saved to Pictures/foto")
                        }
                    }
                }) { Icon(Icons.Default.Download, "Save to device", tint = Color.White) }
            }
            IconButton(onClick = {
                val fav = d?.favorite ?: false
                scope.launch { runCatching { io { foto.setFavorite(library, current.hash, !fav) } } }
            }) {
                Icon(if (d?.favorite == true) Icons.Default.Favorite else Icons.Default.FavoriteBorder, "Favourite", tint = Color.White)
            }
            IconButton(onClick = { info = true }) { Icon(Icons.Default.Info, "Info", tint = Color.White) }
            IconButton(onClick = {
                scope.launch {
                    val r = runCatching {
                        io { if (deletedView) foto.restore(library, listOf(current.hash)) else foto.delete(library, listOf(current.hash)) }
                    }
                    snackbar.showSnackbar(r.exceptionOrNull()?.message ?: if (deletedView) "Restored" else "Moved to Recently deleted")
                    close()
                }
            }) { Icon(if (deletedView) Icons.Default.Restore else Icons.Default.Delete, "Delete", tint = Color.White) }
        }
        if (info && detail != null) {
            val d = detail!!
            ModalBottomSheet(onDismissRequest = { info = false }) {
                Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Field("Name", d.name)
                    Field("Taken", date(d.takenAt, "d MMMM yyyy, HH:mm"))
                    Field("Dimensions", "${d.w} × ${d.h}")
                    Field("Size", "%.1f MB".format(d.size.toDouble() / (1 shl 20)))
                    d.camera?.let { Field("Camera", it) }
                    Field("Added from", d.origin)
                    Field("Originals on", d.holders.joinToString().ifEmpty { "No device yet" })
                    Field("On this device", d.path ?: "Not downloaded")
                    Spacer(Modifier.height(24.dp))
                }
            }
        }
    }
}

@Composable
fun Field(label: String, value: String) {
    Column {
        Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

data class Album(val name: String, val path: String)

fun albums(context: android.content.Context): List<Album> {
    val root = Environment.getExternalStorageDirectory().absolutePath
    val seen = linkedMapOf<String, String>()
    context.contentResolver.query(
        MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL),
        arrayOf(MediaStore.MediaColumns.BUCKET_DISPLAY_NAME, MediaStore.MediaColumns.RELATIVE_PATH),
        "${MediaStore.Files.FileColumns.MEDIA_TYPE} IN (${MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE}, ${MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO})",
        null,
        null,
    )?.use { c ->
        while (c.moveToNext()) {
            val name = c.getString(0) ?: continue
            val rel = c.getString(1) ?: continue
            seen.putIfAbsent("$root/${rel.trimEnd('/')}", name)
        }
    }
    return seen.map { (path, name) -> Album(name, path) }.filterNot { it.path.contains("/Pictures/foto") }.sortedBy { it.name.lowercase() }
}

@Composable
fun Folders(foto: Foto, library: String) {
    val context = LocalContext.current
    val version by context.app.version.collectAsState()
    val scope = rememberCoroutineScope()
    var tick by remember { mutableStateOf(0) }
    val albums by produceState(emptyList<Album>(), tick) { value = io { albums(context) } }
    val roots by produceState(emptyList<String>(), library, version, tick) {
        value = io { foto.roots().filter { it.library == library }.map { it.path } }
    }
    LazyColumn(Modifier.fillMaxSize().statusBarsPadding()) {
        item {
            Column(Modifier.padding(16.dp)) {
                Text("Folders", style = MaterialTheme.typography.headlineSmall)
                Text(
                    "Albums on this phone that foto backs up to your other devices.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        items(albums, key = { it.path }) { album ->
            val on = album.path in roots
            ListItem(
                headlineContent = { Text(album.name) },
                supportingContent = { Text(album.path.substringAfter("/0/"), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                trailingContent = {
                    Switch(on, {
                        scope.launch {
                            runCatching { io { if (on) foto.removeRoot(library, album.path) else foto.addRoot(library, album.path) } }
                            tick++
                        }
                    })
                },
            )
        }
    }
}

fun qr(text: String): ImageBitmap {
    val m = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 640, 640)
    val bmp = Bitmap.createBitmap(m.width, m.height, Bitmap.Config.ARGB_8888)
    for (x in 0 until m.width) for (y in 0 until m.height) bmp.setPixel(x, y, if (m[x, y]) 0xFF000000.toInt() else 0xFFFFFFFF.toInt())
    return bmp.asImageBitmap()
}

@Composable
fun Devices(foto: Foto, library: String, snackbar: SnackbarHostState) {
    val app = LocalContext.current.app
    val version by app.version.collectAsState()
    val libraries by app.libraries.collectAsState()
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val devices by produceState(foto.devices(library), library, version) { value = io { foto.devices(library) } }
    var share by remember { mutableStateOf<Pair<String, ImageBitmap>?>(null) }
    var ticket by remember { mutableStateOf("") }
    fun join(t: String) = scope.launch {
        runCatching { io { foto.join(t.trim()) } }
            .onSuccess { app.addDefaultFolders(it.id); app.refreshLibraries(); app.library.value = it.id; snackbar.showSnackbar("Joined ${it.name}") }
            .onFailure { snackbar.showSnackbar(it.message ?: "Could not join") }
    }
    val scanner = rememberScanner { join(it) }
    LazyColumn(Modifier.fillMaxSize().statusBarsPadding(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item { Text("Devices", style = MaterialTheme.typography.headlineSmall) }
        if (libraries.size > 1) {
            item {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    libraries.forEach { l -> FilterChip(l.id == library, { app.library.value = l.id }, { Text(l.name) }) }
                }
            }
        }
        items(devices, key = { it.id }) { d ->
            ListItem(
                leadingContent = {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(if (d.online) Color(0xFF34C759) else Color.Gray))
                },
                headlineContent = { Text(if (d.thisDevice) "${d.name} (this phone)" else d.name) },
                supportingContent = {
                    Text(
                        listOfNotNull(
                            d.platform,
                            if (d.originalsAll) "${d.held} of ${d.total} originals" else "originals on demand",
                            if (d.archive) "archive" else null,
                        ).joinToString(" · ")
                    )
                },
            )
        }
        item { HorizontalDivider() }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = {
                    scope.launch { runCatching { io { foto.share(library, true) } }.onSuccess { share = it to qr(it) } }
                }) { Text("Invite a device") }
                OutlinedButton(onClick = {
                    scope.launch { runCatching { io { foto.share(library, false) } }.onSuccess { share = it to qr(it) } }
                }) { Text("Share read-only") }
            }
        }
        share?.let { (t, img) ->
            item {
                Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
                    Image(img, "Ticket QR", Modifier.size(260.dp))
                    TextButton(onClick = { clipboard.setText(AnnotatedString(t)) }) { Text("Copy ticket") }
                }
            }
        }
        item { HorizontalDivider() }
        item { Text("Join another library", fontWeight = FontWeight.SemiBold) }
        item {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                Button(onClick = { scanner.launch(scanOptions()) }) {
                    Icon(Icons.Default.QrCodeScanner, null)
                    Spacer(Modifier.width(8.dp))
                    Text("Scan")
                }
            }
        }
        item {
            OutlinedTextField(ticket, { ticket = it }, label = { Text("Paste a ticket") }, modifier = Modifier.fillMaxWidth(), singleLine = true)
        }
        item { OutlinedButton(onClick = { join(ticket) }, enabled = ticket.isNotBlank()) { Text("Join") } }
    }
}

@Composable
fun Settings(foto: Foto, library: String) {
    val scope = rememberCoroutineScope()
    var policy by remember { mutableStateOf(foto.policy()) }
    var name by remember { mutableStateOf(foto.deviceName()) }
    fun update(p: com.andymitch.foto.core.Policy) {
        policy = p
        scope.launch { runCatching { io { foto.setPolicy(p) } } }
    }
    Column(
        Modifier.fillMaxSize().statusBarsPadding().padding(16.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.headlineSmall)
        OutlinedTextField(name, { name = it }, label = { Text("Device name") }, modifier = Modifier.fillMaxWidth(), singleLine = true)
        OutlinedButton(onClick = { scope.launch { runCatching { io { foto.setDeviceName(name.trim()) } } } }) { Text("Rename") }
        Toggle("Keep every original on this phone", "Off: originals download when you open them.", policy.originalsAll) {
            update(policy.copy(originalsAll = it))
        }
        Toggle("Use mobile data for originals", "Previews always sync on any network.", policy.meteredOk) {
            update(policy.copy(meteredOk = it))
        }
        Toggle("Archive", "Keep files that are deleted on other devices.", policy.archive) {
            update(policy.copy(archive = it))
        }
        Field("Library folder", foto.libraryDir(library))
        Field("Device ID", foto.deviceId())
        StatusLine()
    }
}

@Composable
fun Toggle(title: String, subtitle: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title)
            Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(value, onChange)
    }
}
