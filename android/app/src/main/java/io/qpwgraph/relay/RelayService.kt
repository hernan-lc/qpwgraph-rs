package io.qpwgraph.relay

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.roundToInt

class RelayService : Service() {
    companion object {
        const val EXTRA_HANDLE = "handle"
        const val EXTRA_ROLE = "role"
        const val EXTRA_SAMPLE_RATE = "sample_rate"
        const val EXTRA_CHANNELS = "channels"
        const val EXTRA_FRAME_MS = "frame_ms"
        private const val CHANNEL = "relay-audio"
        private const val NOTIFICATION_ID = 48123
    }

    private val running = AtomicBoolean(false)
    private var captureThread: Thread? = null
    private var playbackThread: Thread? = null
    private var handle = 0L
    private var role = "emit"
    private var sampleRate = 48_000
    private var channels = 1
    private var frameMs = 20

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        handle = intent?.getLongExtra(EXTRA_HANDLE, 0L) ?: 0L
        role = intent?.getStringExtra(EXTRA_ROLE) ?: "emit"
        sampleRate = intent?.getIntExtra(EXTRA_SAMPLE_RATE, 48_000) ?: 48_000
        channels = intent?.getIntExtra(EXTRA_CHANNELS, 1) ?: 1
        frameMs = intent?.getIntExtra(EXTRA_FRAME_MS, 20) ?: 20
        startForeground(NOTIFICATION_ID, notification())
        startAudio()
        return START_NOT_STICKY
    }

    private fun startAudio() {
        if (handle == 0L || !running.compareAndSet(false, true)) return
        val frameSamples = sampleRate * frameMs / 1000 * channels
        if (role == "emit" || role == "both") {
            captureThread = Thread {
                val minimum = AudioRecord.getMinBufferSize(
                    sampleRate,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                )
                if (minimum <= 0) return@Thread
                val recorder = AudioRecord(
                    MediaRecorder.AudioSource.MIC,
                    sampleRate,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                    maxOf(minimum, frameSamples * 2),
                )
                val pcm = ShortArray(frameSamples)
                val floats = FloatArray(frameSamples)
                try {
                    recorder.startRecording()
                    while (running.get()) {
                        val count = recorder.read(pcm, 0, pcm.size)
                        if (count <= 0) continue
                        for (index in 0 until count) floats[index] = pcm[index] / 32768f
                        NativeBridge.pushCapture(handle, if (count == floats.size) floats else floats.copyOf(count))
                    }
                } finally {
                    recorder.stop()
                    recorder.release()
                }
            }.also { it.start() }
        }
        if (role == "receive" || role == "both") {
            playbackThread = Thread {
                val minimum = AudioTrack.getMinBufferSize(
                    sampleRate,
                    AudioFormat.CHANNEL_OUT_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                )
                if (minimum <= 0) return@Thread
                val track = AudioTrack.Builder()
                    .setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_MEDIA)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                            .build(),
                    )
                    .setAudioFormat(
                        AudioFormat.Builder()
                            .setSampleRate(sampleRate)
                            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                            .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                            .build(),
                    )
                    .setBufferSizeInBytes(maxOf(minimum, frameSamples * 2))
                    .build()
                val floats = FloatArray(frameSamples)
                val pcm = ShortArray(frameSamples)
                try {
                    track.play()
                    while (running.get()) {
                        val count = NativeBridge.pullPlayback(handle, floats)
                        if (count <= 0) {
                            Thread.sleep(2)
                            continue
                        }
                        for (index in 0 until count) {
                            pcm[index] = (floats[index].coerceIn(-1f, 1f) * Short.MAX_VALUE)
                                .roundToInt().toShort()
                        }
                        track.write(pcm, 0, count)
                    }
                } finally {
                    track.stop()
                    track.release()
                }
            }.also { it.start() }
        }
    }

    override fun onDestroy() {
        running.set(false)
        captureThread?.join(500)
        playbackThread?.join(500)
        captureThread = null
        playbackThread = null
        if (handle != 0L) {
            NativeBridge.disconnect(handle)
            NativeBridge.release(handle)
            handle = 0L
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL,
                "Relay audio",
                NotificationManager.IMPORTANCE_LOW,
            )
            getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
    }

    private fun notification(): Notification = NotificationCompat.Builder(this, CHANNEL)
        .setContentTitle("qpwgraph Relay")
        .setContentText("Audio relay is active")
        .setSmallIcon(android.R.drawable.ic_btn_speak_now)
        .setOngoing(true)
        .build()
}
