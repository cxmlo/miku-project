package dev.miku.link.ui

import android.Manifest
import android.content.pm.PackageManager
import android.util.Size
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.LuminanceSource
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Coucou-inspired fullscreen CameraX QR scanner (`1280x720`, auto-focus, normal + inverted decode).
 */
@Composable
fun QrScannerScreen(
    onScanned: (String) -> Unit,
    onClose: () -> Unit,
    onPasteLink: () -> Unit,
) {
    val context = LocalContext.current
    var hasCameraPerm by remember {
        mutableStateOf(
            ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
                PackageManager.PERMISSION_GRANTED
        )
    }
    var cameraError by remember { mutableStateOf<String?>(null) }

    val launcher = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        hasCameraPerm = granted
    }

    LaunchedEffect(Unit) {
        if (!hasCameraPerm) {
            launcher.launch(Manifest.permission.CAMERA)
        }
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black),
    ) {
        if (hasCameraPerm) {
            CameraXQrPreview(
                onScanned = onScanned,
                onError = { cameraError = it },
            )
            CoucouViewfinderOverlay()
        } else {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(horizontal = 32.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(
                    text = "Permiso de cámara requerido",
                    color = CoucouColors.Text,
                    fontSize = 18.sp,
                    fontWeight = FontWeight.SemiBold,
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(10.dp))
                Text(
                    text = "Activa la cámara para escanear el QR de Miku PC.",
                    color = CoucouColors.TextDim,
                    fontSize = 14.sp,
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(20.dp))
                CoucouPillButton(
                    label = "Permitir cámara",
                    onClick = { launcher.launch(Manifest.permission.CAMERA) },
                    primary = true,
                )
            }
        }

        Row(
            modifier = Modifier
                .fillMaxWidth()
                .windowInsetsPadding(WindowInsets.statusBars)
                .padding(horizontal = 20.dp, vertical = 14.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                modifier = Modifier
                    .heightIn(min = 44.dp)
                    .clip(CircleShape)
                    .background(Color(0xB3141518))
                    .border(1.dp, CoucouColors.Line, CircleShape)
                    .clickable(role = Role.Button, onClick = onClose)
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Atrás", color = CoucouColors.Text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }

            Box(
                modifier = Modifier
                    .heightIn(min = 44.dp)
                    .clip(CircleShape)
                    .background(Color(0xB3141518))
                    .border(1.dp, CoucouColors.Line, CircleShape)
                    .clickable(role = Role.Button, onClick = onPasteLink)
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Pegar enlace", color = CoucouColors.Text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            }
        }

        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .padding(horizontal = 28.dp, vertical = 44.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            if (cameraError != null) {
                Text(
                    text = cameraError ?: "",
                    color = CoucouColors.Danger,
                    fontSize = 13.sp,
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(8.dp))
            }
            Text(
                text = "Apunta al código QR en Miku PC → Ajustes → Miku-Link",
                color = CoucouColors.Text,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun CameraXQrPreview(
    onScanned: (String) -> Unit,
    onError: (String) -> Unit,
) {
    val context = LocalContext.current
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val delivered = remember { AtomicBoolean(false) }

    DisposableEffect(Unit) {
        onDispose { executor.shutdown() }
    }

    AndroidView(
        modifier = Modifier.fillMaxSize(),
        factory = { ctx ->
            val previewView = PreviewView(ctx).apply {
                implementationMode = PreviewView.ImplementationMode.COMPATIBLE
                scaleType = PreviewView.ScaleType.FILL_CENTER
            }
            val future = ProcessCameraProvider.getInstance(ctx)
            future.addListener({
                try {
                    val provider = future.get()
                    val preview = Preview.Builder().build().also {
                        it.setSurfaceProvider(previewView.surfaceProvider)
                    }
                    val analysis = ImageAnalysis.Builder()
                        .setTargetResolution(Size(1280, 720))
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                    val reader = QRCodeReader()
                    val hints = mapOf(DecodeHintType.TRY_HARDER to true)

                    analysis.setAnalyzer(executor) { image ->
                        if (delivered.get()) {
                            image.close()
                            return@setAnalyzer
                        }
                        try {
                            val decoded = decodeYuvImage(image, reader, hints)
                            if (decoded != null && delivered.compareAndSet(false, true)) {
                                previewView.post { onScanned(decoded) }
                            }
                        } finally {
                            image.close()
                        }
                    }

                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner,
                        CameraSelector.DEFAULT_BACK_CAMERA,
                        preview,
                        analysis,
                    )
                } catch (t: Throwable) {
                    onError(t.message ?: "No se pudo iniciar CameraX")
                }
            }, ContextCompat.getMainExecutor(ctx))
            previewView
        },
    )
}

@Composable
private fun CoucouViewfinderOverlay() {
    Canvas(modifier = Modifier.fillMaxSize()) {
        val boxSize = minOf(size.width, size.height) * 0.66f
        val left = (size.width - boxSize) / 2f
        val top = (size.height - boxSize) / 2f - 20f
        val right = left + boxSize
        val bottom = top + boxSize
        val r = 24.dp.toPx()

        val scrim = Path().apply {
            fillType = PathFillType.EvenOdd
            addRect(Rect(0f, 0f, size.width, size.height))
            addRoundRect(
                androidx.compose.ui.geometry.RoundRect(
                    left = left,
                    top = top,
                    right = right,
                    bottom = bottom,
                    cornerRadius = CornerRadius(r, r),
                )
            )
        }
        drawPath(scrim, color = Color(0x99000000))

        val stroke = Stroke(width = 3.dp.toPx(), cap = StrokeCap.Round)
        val arm = 26.dp.toPx()
        val c = Color(0xFFF5F6F8)
        // Top-left
        drawLine(c, Offset(left, top + arm), Offset(left, top + r), stroke.width, StrokeCap.Round)
        drawLine(c, Offset(left + r, top), Offset(left + arm, top), stroke.width, StrokeCap.Round)
        // Top-right
        drawLine(c, Offset(right - arm, top), Offset(right - r, top), stroke.width, StrokeCap.Round)
        drawLine(c, Offset(right, top + r), Offset(right, top + arm), stroke.width, StrokeCap.Round)
        // Bottom-left
        drawLine(c, Offset(left, bottom - arm), Offset(left, bottom - r), stroke.width, StrokeCap.Round)
        drawLine(c, Offset(left + r, bottom), Offset(left + arm, bottom), stroke.width, StrokeCap.Round)
        // Bottom-right
        drawLine(c, Offset(right - arm, bottom), Offset(right - r, bottom), stroke.width, StrokeCap.Round)
        drawLine(c, Offset(right, bottom - arm), Offset(right, bottom - r), stroke.width, StrokeCap.Round)
    }
}

private fun decodeYuvImage(
    image: ImageProxy,
    reader: QRCodeReader,
    hints: Map<DecodeHintType, Any>,
): String? {
    val plane = image.planes.firstOrNull() ?: return null
    val w = image.width
    val h = image.height
    val rowStride = plane.rowStride
    val pixelStride = plane.pixelStride
    if (w <= 0 || h <= 0 || rowStride < w) return null

    val buf = plane.buffer
    val raw = ByteArray(buf.remaining())
    buf.get(raw)

    // Pack Y plane tightly into w * h bytes so PlanarYUVLuminanceSource never fails on trailing row padding.
    val packed = if (rowStride == w && pixelStride == 1 && raw.size >= w * h) {
        raw
    } else {
        ByteArray(w * h).also { dst ->
            for (row in 0 until h) {
                val srcRowStart = row * rowStride
                if (srcRowStart >= raw.size) break
                if (pixelStride == 1) {
                    val avail = minOf(w, raw.size - srcRowStart)
                    System.arraycopy(raw, srcRowStart, dst, row * w, avail)
                } else {
                    for (col in 0 until w) {
                        val idx = srcRowStart + col * pixelStride
                        if (idx < raw.size) dst[row * w + col] = raw[idx]
                    }
                }
            }
        }
    }

    val source = PlanarYUVLuminanceSource(packed, w, h, 0, 0, w, h, false)
    return tryDecode(reader, source, hints) ?: tryDecode(reader, source.invert(), hints)
}

private fun tryDecode(
    reader: QRCodeReader,
    source: LuminanceSource,
    hints: Map<DecodeHintType, Any>,
): String? {
    return try {
        reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
    } catch (_: Throwable) {
        null
    } finally {
        reader.reset()
    }
}
