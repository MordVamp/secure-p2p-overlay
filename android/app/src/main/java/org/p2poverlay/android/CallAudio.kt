package org.p2poverlay.android

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AutomaticGainControl
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Process
import androidx.core.content.ContextCompat
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException

/**
 * Звук звонка на телефоне: микрофон → узел, смесь собеседников от узла → динамик.
 *
 * Кодек, джиттер-буфер, восстановление потерь и шифрование — в узле (Rust, app/voice.rs). Здесь только
 * 16-битный PCM 16 кГц кадрами по 20 мс через локальный UDP-канал узла (127.0.0.1, токен в каждом кадре:
 * другие приложения телефона не могут ни подмешать звук, ни получить чужой).
 *
 * Источник VOICE_COMMUNICATION включает системное эхоподавление и шумоподавление телефона. Кадр отправляется
 * на каждый прочитанный с микрофона — темп задают часы звуковой карты; ответ узла сразу идёт в AudioTrack.
 */
class CallAudio(private val ctx: Context, private val port: Int, private val token: ByteArray) {
    @Volatile var muted = false
    @Volatile var speaker = false
        set(v) { field = v; route() }
    /** Громкость своего микрофона 0..1 (для индикатора). */
    @Volatile var micLevel = 0f; private set
    @Volatile private var running = false

    private val am = ctx.getSystemService(AudioManager::class.java)
    private var rec: AudioRecord? = null
    private var track: AudioTrack? = null
    private var sock: DatagramSocket? = null
    private val threads = mutableListOf<Thread>()
    private var oldMode = AudioManager.MODE_NORMAL

    val hasMic: Boolean
        get() = ContextCompat.checkSelfPermission(ctx, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    @SuppressLint("MissingPermission")
    fun start() {
        if (running) return
        running = true
        oldMode = am.mode
        am.mode = AudioManager.MODE_IN_COMMUNICATION
        route()
        val nodePort = port   // внутри apply `port` — это свойство самого сокета (-1 до connect)
        val s = DatagramSocket().apply { soTimeout = 300; connect(InetAddress.getByName("127.0.0.1"), nodePort) }
        sock = s

        val outMin = AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        track = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(RATE).setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setBufferSizeInBytes(maxOf(outMin, FRAME * 2 * 6))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build().also { it.play() }

        if (hasMic) runCatching {
            val inMin = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, RATE, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, maxOf(inMin, FRAME * 2 * 4)).also { r ->
                if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(r.audioSessionId)?.enabled = true
                if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(r.audioSessionId)?.enabled = true
                if (AutomaticGainControl.isAvailable()) AutomaticGainControl.create(r.audioSessionId)?.enabled = true
                r.startRecording()
                rec = r
            }
        }

        threads += Thread({ sendLoop(s) }, "call-mic").apply { start() }
        threads += Thread({ recvLoop(s) }, "call-speaker").apply { start() }
    }

    fun stop() {
        running = false
        threads.forEach { runCatching { it.join(800) } }
        threads.clear()
        rec?.let { runCatching { it.stop() }; it.release() }
        track?.let { runCatching { it.stop() }; it.release() }
        rec = null; track = null
        sock?.close(); sock = null
        if (Build.VERSION.SDK_INT >= 31) am.clearCommunicationDevice() else @Suppress("DEPRECATION") { am.isSpeakerphoneOn = false }
        am.mode = oldMode
    }

    private fun route() {
        if (Build.VERSION.SDK_INT >= 31) {
            val want = if (speaker) AudioDeviceInfo.TYPE_BUILTIN_SPEAKER else AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
            am.availableCommunicationDevices.firstOrNull { it.type == want }?.let { am.setCommunicationDevice(it) }
                ?: am.clearCommunicationDevice()
        } else @Suppress("DEPRECATION") { am.isSpeakerphoneOn = speaker }
    }

    private fun sendLoop(s: DatagramSocket) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val pcm = ShortArray(FRAME)
        val buf = ByteArray(4 + 16 + FRAME * 2)
        MAGIC.copyInto(buf, 0)
        token.copyInto(buf, 4)
        var next = System.nanoTime()
        while (running) {
            val r = rec
            if (r != null) {
                var got = 0
                while (got < FRAME && running) {
                    val n = r.read(pcm, got, FRAME - got)
                    if (n <= 0) break
                    got += n
                }
            } else {
                // микрофона нет (не дано разрешение) — тишина в том же темпе, чтобы слышать собеседников
                next += 20_000_000L
                val wait = (next - System.nanoTime()) / 1_000_000L
                if (wait > 0) Thread.sleep(wait)
                pcm.fill(0)
            }
            if (muted) pcm.fill(0)
            var sum = 0.0
            for (i in 0 until FRAME) {
                val v = pcm[i].toInt()
                sum += (v * v).toDouble()
                buf[20 + 2 * i] = (v and 0xff).toByte()
                buf[21 + 2 * i] = ((v shr 8) and 0xff).toByte()
            }
            micLevel = (kotlin.math.sqrt(sum / FRAME) / 32768.0).toFloat()
            runCatching { s.send(DatagramPacket(buf, buf.size)) }
        }
    }

    private fun recvLoop(s: DatagramSocket) {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val buf = ByteArray(2048)
        val pcm = ShortArray(FRAME)
        val p = DatagramPacket(buf, buf.size)
        while (running) {
            try { s.receive(p) } catch (_: SocketTimeoutException) { continue } catch (_: Exception) { if (!running) break; continue }
            if (p.length != 4 + FRAME * 2 || buf[0] != MAGIC[0] || buf[3] != MAGIC[3]) continue
            for (i in 0 until FRAME) pcm[i] = ((buf[4 + 2 * i].toInt() and 0xff) or (buf[5 + 2 * i].toInt() shl 8)).toShort()
            // без блокировки: если буфер динамика полон (часы разошлись) — кадр выбрасывается, задержка не копится
            track?.write(pcm, 0, FRAME, AudioTrack.WRITE_NON_BLOCKING)
        }
    }

    companion object {
        const val RATE = 16_000
        const val FRAME = 320
        private val MAGIC = "P2A1".toByteArray()
    }
}
