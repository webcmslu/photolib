// The phone's photo library, for backing it up to a Phototek server and
// freeing up space (see ../../../../../../../src/lib.rs). Photos and videos
// come from MediaStore, as the camera saved them.
package lu.webcms.phototek.photos

import android.Manifest
import android.app.Activity
import android.app.UiModeManager
import android.content.Context
import android.content.res.Configuration
import android.content.ContentUris
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.provider.Settings
import androidx.activity.result.ActivityResult
import androidx.activity.result.IntentSenderRequest
import app.tauri.PermissionState
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class ExportArgs {
  lateinit var id: String
  lateinit var dir: String
}

@InvokeArg
class DeleteArgs {
  var ids: List<String> = emptyList()
}

@TauriPlugin(
  permissions = [
    Permission(strings = [Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO], alias = "media"),
    Permission(strings = [Manifest.permission.READ_EXTERNAL_STORAGE], alias = "storage"),
    // Without it, Android removes the GPS position from the photos read.
    Permission(strings = [Manifest.permission.ACCESS_MEDIA_LOCATION], alias = "location"),
  ]
)
class PhotosPlugin(private val activity: Activity) : Plugin(activity) {
  private val alias = if (Build.VERSION.SDK_INT >= 33) "media" else "storage"

  /** Asks for access to the photos and videos (the system asks the user once). */
  @Command
  fun requestAccess(invoke: Invoke) {
    if (getPermissionState(alias) == PermissionState.GRANTED) {
      answerAccess(invoke)
    } else {
      val aliases = if (Build.VERSION.SDK_INT >= 29) arrayOf(alias, "location") else arrayOf(alias)
      requestPermissionForAliases(aliases, invoke, "accessAnswered")
    }
  }

  @PermissionCallback
  private fun accessAnswered(invoke: Invoke) = answerAccess(invoke)

  private fun answerAccess(invoke: Invoke) {
    val granted = getPermissionState(alias) == PermissionState.GRANTED
    invoke.resolve(JSObject().put("granted", granted).put("limited", false))
  }

  /** Every photo and video, oldest first: id, when it was taken (ms), kind. */
  @Command
  fun list(invoke: Invoke) {
    Thread {
      val items = JSArray()
      for ((collection, video) in listOf(
        MediaStore.Images.Media.EXTERNAL_CONTENT_URI to false,
        MediaStore.Video.Media.EXTERNAL_CONTENT_URI to true,
      )) {
        val columns = arrayOf(MediaStore.MediaColumns._ID, MediaStore.MediaColumns.DATE_TAKEN, MediaStore.MediaColumns.DATE_ADDED)
        activity.contentResolver.query(collection, columns, null, null, null)?.use { cursor ->
          val id = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
          val takenAt = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_TAKEN)
          val added = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_ADDED)
          while (cursor.moveToNext()) {
            val taken = cursor.getLong(takenAt).takeIf { it > 0 } ?: (cursor.getLong(added) * 1000)
            items.put(
              JSObject()
                .put("id", (if (video) "video:" else "image:") + cursor.getLong(id))
                .put("taken", taken)
                .put("video", video)
            )
          }
        }
      }
      invoke.resolve(JSObject().put("items", items))
    }.start()
  }

  private fun uriOf(id: String): Uri {
    val (kind, number) = id.split(":", limit = 2)
    val collection = if (kind == "video") MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
    return ContentUris.withAppendedId(collection, number.toLong())
  }

  /** Copies one photo or video, as the camera saved it, into `dir`. */
  @Command
  fun export(invoke: Invoke) {
    val args = invoke.parseArgs(ExportArgs::class.java)
    Thread {
      try {
        val plain = uriOf(args.id)
        // The original, with its GPS position, when Android lets the app have it.
        val uri = if (Build.VERSION.SDK_INT >= 29 && getPermissionState("location") == PermissionState.GRANTED) {
          MediaStore.setRequireOriginal(plain)
        } else {
          plain
        }
        var name = "photo"
        activity.contentResolver.query(plain, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use {
          if (it.moveToFirst()) name = it.getString(0) ?: name
        }
        val folder = File(args.dir).apply { mkdirs() }
        val target = File(folder, name)
        activity.contentResolver.openInputStream(uri)!!.use { input ->
          target.outputStream().use { output -> input.copyTo(output) }
        }
        val file = JSObject().put("path", target.absolutePath).put("name", name)
        invoke.resolve(JSObject().put("files", JSArray().put(file)))
      } catch (e: Exception) {
        invoke.reject(e.message ?: "This photo can't be read.")
      }
    }.start()
  }

  /**
   * Moves photos and videos to the trash, where the phone keeps them 30 days;
   * from Android 11 the system asks the user to confirm.
   */
  @Command
  fun delete(invoke: Invoke) {
    val args = invoke.parseArgs(DeleteArgs::class.java)
    val uris = args.ids.map { uriOf(it) }
    if (Build.VERSION.SDK_INT >= 30) {
      val request = MediaStore.createTrashRequest(activity.contentResolver, uris, true)
      startIntentSenderForResult(invoke, IntentSenderRequest.Builder(request.intentSender).build(), "deleteAnswered")
    } else {
      for (uri in uris) activity.contentResolver.delete(uri, null, null)
      invoke.resolve(JSObject().put("deleted", true))
    }
  }

  @ActivityCallback
  private fun deleteAnswered(invoke: Invoke, result: ActivityResult) {
    invoke.resolve(JSObject().put("deleted", result.resultCode == Activity.RESULT_OK))
  }

  /** True on a TV (Android TV, Google TV). */
  @Command
  fun isTelevision(invoke: Invoke) {
    val modes = activity.getSystemService(Context.UI_MODE_SERVICE) as UiModeManager
    val tv = modes.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION
    invoke.resolve(JSObject().put("tv", tv))
  }

  /** The phone's name, as its owner set it. */
  @Command
  fun deviceName(invoke: Invoke) {
    val name = Settings.Global.getString(activity.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL
    invoke.resolve(JSObject().put("name", name))
  }
}
