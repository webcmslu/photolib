// The phone's photo library, for backing it up to a Phototek server and
// freeing up space (see ../src/lib.rs). Originals only: the photo or video as
// taken, a Live Photo's video, and a RAW's companion; never edited versions.

import Photos
import SwiftRs
import Tauri
import UIKit
import WebKit

class ExportArgs: Decodable {
  let id: String
  let dir: String
}

class DeleteArgs: Decodable {
  let ids: [String]
}

class PhotosPlugin: Plugin {
  /// Asks for access to the photo library (the system asks the user once).
  @objc public func requestAccess(_ invoke: Invoke) {
    PHPhotoLibrary.requestAuthorization(for: .readWrite) { status in
      invoke.resolve([
        "granted": status == .authorized || status == .limited,
        "limited": status == .limited,
      ])
    }
  }

  /// Every photo and video, oldest first: id, when it was taken (ms), kind.
  @objc public func list(_ invoke: Invoke) {
    DispatchQueue.global(qos: .userInitiated).async {
      let options = PHFetchOptions()
      options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: true)]
      let assets = PHAsset.fetchAssets(with: options)
      var items: [[String: Any]] = []
      items.reserveCapacity(assets.count)
      assets.enumerateObjects { asset, _, _ in
        guard asset.mediaType == .image || asset.mediaType == .video else { return }
        let taken = (asset.creationDate ?? asset.modificationDate)?.timeIntervalSince1970 ?? 0
        items.append([
          "id": asset.localIdentifier,
          "taken": Int64(taken * 1000),
          "video": asset.mediaType == .video,
        ])
      }
      invoke.resolve(["items": items])
    }
  }

  /// Writes the originals of one photo or video into `dir`, downloading them
  /// from iCloud if the phone keeps only smaller versions.
  @objc public func export(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ExportArgs.self)
    guard let asset = PHAsset.fetchAssets(withLocalIdentifiers: [args.id], options: nil).firstObject
    else {
      invoke.reject("This photo is no longer on the phone.")
      return
    }
    let wanted: [PHAssetResourceType] = [.photo, .video, .pairedVideo, .alternatePhoto]
    let resources = PHAssetResource.assetResources(for: asset).filter { wanted.contains($0.type) }
    let folder = URL(fileURLWithPath: args.dir, isDirectory: true)
    try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let options = PHAssetResourceRequestOptions()
    options.isNetworkAccessAllowed = true
    DispatchQueue.global(qos: .userInitiated).async {
      var files: [[String: Any]] = []
      var failure: String?
      for resource in resources {
        let target = folder.appendingPathComponent(resource.originalFilename)
        try? FileManager.default.removeItem(at: target)
        let done = DispatchSemaphore(value: 0)
        PHAssetResourceManager.default().writeData(for: resource, toFile: target, options: options) { error in
          if let error = error {
            failure = error.localizedDescription
          } else {
            files.append(["path": target.path, "name": resource.originalFilename])
          }
          done.signal()
        }
        done.wait()
        if failure != nil { break }
      }
      if let failure = failure {
        invoke.reject(failure)
      } else {
        invoke.resolve(["files": files])
      }
    }
  }

  /// Deletes photos and videos; the system asks the user to confirm. They
  /// stay 30 days in Recently Deleted.
  @objc public func delete(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(DeleteArgs.self)
    let assets = PHAsset.fetchAssets(withLocalIdentifiers: args.ids, options: nil)
    PHPhotoLibrary.shared().performChanges({
      PHAssetChangeRequest.deleteAssets(assets)
    }) { deleted, _ in
      invoke.resolve(["deleted": deleted])
    }
  }

  /// Never a TV on iOS (Apple TV has no web view).
  @objc public func isTelevision(_ invoke: Invoke) {
    invoke.resolve(["tv": false])
  }

  /// The phone's name, as its owner set it when the system allows it.
  @objc public func deviceName(_ invoke: Invoke) {
    DispatchQueue.main.async {
      invoke.resolve(["name": UIDevice.current.name])
    }
  }
}

@_cdecl("init_plugin_photos")
public func initPlugin() -> Plugin {
  return PhotosPlugin()
}
