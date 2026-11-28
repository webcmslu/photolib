// Backs up the phone's new photos to the Phototek server while the app is
// closed: WorkManager runs it every few hours when the phone is online. It
// does what the app's own backup does (src-tauri/src/mobile.rs), with the
// same record of what is saved, so neither sends a photo twice: each photo
// not in the record is sent to POST /api/upload, which keeps it only if the
// library doesn't have it, and answers with its fingerprint.
package lu.webcms.phototek.photos

import android.content.ContentUris
import android.content.Context
import android.os.Build
import android.provider.MediaStore
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder
import java.util.concurrent.TimeUnit

class BackupWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
  companion object {
    private const val WORK = "phototek-backup"
    private const val PREFS = "phototek-backup"

    /** Backs up every `hours` from now on, with these settings (the app calls it). */
    fun schedule(context: Context, url: String, token: String?, device: String, record: String, hours: Long) {
      context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
        .putString("url", url.trimEnd('/'))
        .putString("token", token)
        .putString("device", device)
        .putString("record", record)
        .apply()
      val request = PeriodicWorkRequestBuilder<BackupWorker>(hours, TimeUnit.HOURS)
        .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
        .build()
      WorkManager.getInstance(context).enqueueUniquePeriodicWork(WORK, ExistingPeriodicWorkPolicy.UPDATE, request)
    }

    fun cancel(context: Context) {
      WorkManager.getInstance(context).cancelUniqueWork(WORK)
    }
  }

  private data class Item(val id: String, val uri: android.net.Uri, val name: String, val taken: Long)

  override fun doWork(): Result {
    val prefs = applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val url = prefs.getString("url", null) ?: return Result.success()
    val token = prefs.getString("token", null)
    val device = prefs.getString("device", null) ?: "Phone"
    val recordFile = File(prefs.getString("record", null) ?: return Result.success())

    val record = try {
      JSONObject(recordFile.readText())
    } catch (e: Exception) {
      JSONObject()
    }
    val saved = record.optJSONObject("saved") ?: JSONObject().also { record.put("saved", it) }

    var sent = 0
    for (item in photos()) {
      if (isStopped) break
      if (saved.has(item.id)) continue
      val fingerprint = try {
        send(url, token, device, item)
      } catch (e: ServerUnreachable) {
        // Offline, or the server is down: try again next time.
        write(recordFile, record)
        return Result.retry()
      } catch (e: Exception) {
        null // this photo can't be read: the others go on
      } ?: continue
      saved.put(item.id, JSONArray().put(fingerprint))
      sent += 1
      if (sent % 10 == 0) write(recordFile, record)
    }
    write(recordFile, record)
    return Result.success()
  }

  private fun write(file: File, record: JSONObject) {
    val part = File(file.path + ".part")
    part.writeText(record.toString())
    part.renameTo(file)
  }

  /** Every photo and video, as the app lists them (same ids). */
  private fun photos(): List<Item> {
    val items = mutableListOf<Item>()
    for ((collection, video) in listOf(
      MediaStore.Images.Media.EXTERNAL_CONTENT_URI to false,
      MediaStore.Video.Media.EXTERNAL_CONTENT_URI to true,
    )) {
      val columns = arrayOf(
        MediaStore.MediaColumns._ID, MediaStore.MediaColumns.DISPLAY_NAME,
        MediaStore.MediaColumns.DATE_TAKEN, MediaStore.MediaColumns.DATE_ADDED,
      )
      applicationContext.contentResolver.query(collection, columns, null, null, null)?.use { cursor ->
        while (cursor.moveToNext()) {
          val number = cursor.getLong(0)
          val taken = cursor.getLong(2).takeIf { it > 0 } ?: (cursor.getLong(3) * 1000)
          items.add(
            Item(
              (if (video) "video:" else "image:") + number,
              ContentUris.withAppendedId(collection, number),
              cursor.getString(1) ?: "photo",
              taken,
            )
          )
        }
      }
    }
    return items.sortedBy { it.taken }
  }

  private class ServerUnreachable(cause: Exception) : Exception(cause)

  /** Sends one photo; returns its fingerprint, as the server answers. */
  private fun send(server: String, token: String?, device: String, item: Item): String? {
    val query = "device=" + URLEncoder.encode(device, "UTF-8") +
      "&name=" + URLEncoder.encode(item.name, "UTF-8") + "&taken=" + item.taken
    val connection = try {
      (URL("$server/api/upload?$query").openConnection() as HttpURLConnection).apply {
        requestMethod = "POST"
        doOutput = true
        connectTimeout = 15_000
        readTimeout = 120_000
        setChunkedStreamingMode(256 * 1024)
        setRequestProperty("Content-Type", "application/octet-stream")
        if (token != null) setRequestProperty("Authorization", "Bearer $token")
      }
    } catch (e: Exception) {
      throw ServerUnreachable(e)
    }
    try {
      val uri = if (Build.VERSION.SDK_INT >= 29) {
        try {
          MediaStore.setRequireOriginal(item.uri)
        } catch (e: Exception) {
          item.uri
        }
      } else {
        item.uri
      }
      val input = try {
        applicationContext.contentResolver.openInputStream(uri)
      } catch (e: SecurityException) {
        applicationContext.contentResolver.openInputStream(item.uri)
      } ?: return null
      try {
        input.use { photo -> connection.outputStream.use { photo.copyTo(it) } }
      } catch (e: java.io.IOException) {
        throw ServerUnreachable(e)
      }
      if (connection.responseCode != 200) {
        // Refused (a read-only library…): the next run tries again.
        throw ServerUnreachable(Exception("HTTP ${connection.responseCode}"))
      }
      val answer = JSONObject(connection.inputStream.bufferedReader().readText())
      return answer.optString("fingerprint").takeIf { it.isNotEmpty() }
    } finally {
      connection.disconnect()
    }
  }
}
