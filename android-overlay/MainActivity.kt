package app.spotless.android

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.util.Log

class MainActivity : TauriActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        // Android 13+ needs the notification permission for the foreground-service notification.
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        initMpvJni()

        try {
            val svc = Intent(this, PlaybackService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) startForegroundService(svc) else startService(svc)
        } catch (t: Throwable) {
            Log.w("Spotless", "could not start the playback service", t)
        }
    }

    /**
     * libmpv's Android audio output needs FFmpeg's JavaVM and application context. The libmpv AAR
     * sets both inside MPVLib.create(); the instance it returns is not needed, so it is
     * destroyed again. Called by reflection so a different AAR version cannot break the build;
     * if it fails, mpv falls back to OpenSL ES.
     */
    private fun initMpvJni() {
        try {
            val cls = Class.forName("dev.jdtech.mpv.MPVLib")
            val companion = cls.getField("Companion").get(null)
            val lib = companion!!.javaClass.getMethod("create", android.content.Context::class.java)
                .invoke(companion, applicationContext)
            lib!!.javaClass.getMethod("destroy").invoke(lib)
            Log.i("Spotless", "mpv JNI initialised")
        } catch (t: Throwable) {
            Log.w("Spotless", "mpv JNI init failed, relying on OpenSL ES", t)
        }
    }
}
