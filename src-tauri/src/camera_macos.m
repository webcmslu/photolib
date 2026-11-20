// iPhones, iPads and cameras connected by USB, through Apple's
// ImageCaptureCore framework (what Image Capture and Photos use): an iPhone
// does not show up as a drive on a Mac. Used by camera_macos.rs.
//
// ImageCaptureCore talks through delegates on the main thread. Everything
// here keeps its state on the main queue; the C functions are called from
// background threads and wait (with a time limit) for the answer. They must
// never be called from the main thread.
//
// Strings returned to Rust are JSON, allocated with malloc; Rust frees them
// with `pl_camera_free`.

#import <Foundation/Foundation.h>
#import <ImageCaptureCore/ImageCaptureCore.h>
#include <stdatomic.h>

@interface PLCameras : NSObject <ICDeviceBrowserDelegate, ICCameraDeviceDelegate>
@property(strong) ICDeviceBrowser *browser;
@property(strong) NSMutableArray<ICCameraDevice *> *devices;
/// UUIDs of the devices whose list of files is complete.
@property(strong) NSMutableSet<NSString *> *ready;
/// The files of each device, by UUID, as numbered for Rust.
@property(strong) NSMutableDictionary<NSString *, NSArray<ICCameraFile *> *> *catalogs;
/// UUIDs of the devices a session is being opened on.
@property(strong) NSMutableSet<NSString *> *opening;
@end

@implementation PLCameras

- (instancetype)init {
    if ((self = [super init])) {
        _devices = [NSMutableArray array];
        _ready = [NSMutableSet set];
        _catalogs = [NSMutableDictionary dictionary];
        _opening = [NSMutableSet set];
        _browser = [[ICDeviceBrowser alloc] init];
        _browser.delegate = self;
        _browser.browsedDeviceTypeMask = (ICDeviceTypeMask)(ICDeviceTypeMaskCamera | ICDeviceLocationTypeMaskLocal);
        [_browser start];
    }
    return self;
}

// --- ICDeviceBrowserDelegate ---

- (void)deviceBrowser:(ICDeviceBrowser *)browser didAddDevice:(ICDevice *)device moreComing:(BOOL)moreComing {
    // Memory cards and cameras that mount as a drive are imported as folders.
    if (![device isKindOfClass:[ICCameraDevice class]] ||
        [device.transportType isEqualToString:ICTransportTypeMassStorage]) {
        return;
    }
    device.delegate = self;
    [self.devices addObject:(ICCameraDevice *)device];
    // A session at once: the phone's "locked" state is only up to date with
    // one open, and the list of its photos starts loading.
    [self openSessionOn:(ICCameraDevice *)device];
}

/// Opens a session on the device unless one is open or being opened.
- (void)openSessionOn:(ICCameraDevice *)device {
    NSString *uuid = device.UUIDString;
    if (!uuid || device.hasOpenSession || [self.opening containsObject:uuid]) return;
    [self.opening addObject:uuid];
    [device requestOpenSessionWithOptions:nil
                               completion:^(NSError *error) {
                                   (void)error;
                                   dispatch_async(dispatch_get_main_queue(), ^{
                                       [self.opening removeObject:uuid];
                                   });
                               }];
}

/// True while the phone is locked or doesn't trust this computer: it says
/// so, and has not started giving its list of photos. (The flag alone can
/// lag behind: a phone that lists its photos is not restricted.)
- (BOOL)isRestricted:(ICCameraDevice *)device {
    return device.accessRestrictedAppleDevice && ![self.ready containsObject:device.UUIDString ?: @""] &&
           device.contentCatalogPercentCompleted == 0 && device.mediaFiles.count == 0;
}

- (void)deviceBrowser:(ICDeviceBrowser *)browser didRemoveDevice:(ICDevice *)device moreGoing:(BOOL)moreGoing {
    [self forget:device];
}

- (void)forget:(ICDevice *)device {
    [self.devices removeObject:(ICCameraDevice *)device];
    if (device.UUIDString) {
        [self.ready removeObject:device.UUIDString];
        [self.catalogs removeObjectForKey:device.UUIDString];
    }
}

// --- ICDeviceDelegate / ICCameraDeviceDelegate ---

- (void)didRemoveDevice:(ICDevice *)device {
    [self forget:device];
}
- (void)device:(ICDevice *)device didOpenSessionWithError:(NSError *)error {
}
- (void)device:(ICDevice *)device didCloseSessionWithError:(NSError *)error {
    // The numbered list stays: its files are found again once a session is reopened.
    if (device.UUIDString) [self.ready removeObject:device.UUIDString];
}
- (void)deviceDidBecomeReadyWithCompleteContentCatalog:(ICCameraDevice *)device {
    if (device.UUIDString) [self.ready addObject:device.UUIDString];
}
// The numbered list stays as it is during an import; the next listing
// picks up photos taken or deleted meanwhile.
- (void)cameraDevice:(ICCameraDevice *)camera didAddItems:(NSArray<ICCameraItem *> *)items {
}
- (void)cameraDevice:(ICCameraDevice *)camera didRemoveItems:(NSArray<ICCameraItem *> *)items {
}
- (void)cameraDevice:(ICCameraDevice *)camera
    didReceiveThumbnail:(CGImageRef)thumbnail
                forItem:(ICCameraItem *)item
                  error:(NSError *)error {
}
- (void)cameraDevice:(ICCameraDevice *)camera
    didReceiveMetadata:(NSDictionary *)metadata
               forItem:(ICCameraItem *)item
                 error:(NSError *)error {
}
- (void)cameraDevice:(ICCameraDevice *)camera didRenameItems:(NSArray<ICCameraItem *> *)items {
}
- (void)cameraDeviceDidChangeCapability:(ICCameraDevice *)camera {
}
- (void)cameraDevice:(ICCameraDevice *)camera didReceivePTPEvent:(NSData *)eventData {
}
- (void)cameraDeviceDidRemoveAccessRestriction:(ICDevice *)device {
    // Unlocked and trusted: its list of photos is (re)loaded now.
    NSString *uuid = device.UUIDString;
    if (!uuid) return;
    [self.ready removeObject:uuid];
    [self openSessionOn:(ICCameraDevice *)device];
}
- (void)cameraDeviceDidEnableAccessRestriction:(ICDevice *)device {
    if (device.UUIDString) [self.ready removeObject:device.UUIDString];
}

// --- Helpers (main queue) ---

- (ICCameraDevice *)deviceWithId:(NSString *)uuid {
    for (ICCameraDevice *device in self.devices) {
        if ([device.UUIDString isEqualToString:uuid]) return device;
    }
    return nil;
}

/// The device's files, numbered once: its media files and their companions
/// (a Live Photo's video, a RAW file), each once.
- (NSArray<ICCameraFile *> *)catalogOf:(ICCameraDevice *)device {
    NSArray<ICCameraFile *> *catalog = self.catalogs[device.UUIDString];
    if (catalog) return catalog;
    NSMutableArray<ICCameraFile *> *files = [NSMutableArray array];
    NSMutableSet *seen = [NSMutableSet set];
    void (^add)(ICCameraItem *) = ^(ICCameraItem *item) {
        if ([item isKindOfClass:[ICCameraFile class]] && ![seen containsObject:item]) {
            [seen addObject:item];
            [files addObject:(ICCameraFile *)item];
        }
    };
    for (ICCameraItem *item in device.mediaFiles) {
        add(item);
        if ([item isKindOfClass:[ICCameraFile class]]) {
            for (ICCameraItem *sidecar in ((ICCameraFile *)item).sidecarFiles) add(sidecar);
        }
    }
    self.catalogs[device.UUIDString] = files;
    return files;
}

@end

static PLCameras *cameras(void) {
    static PLCameras *shared;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        // The browser must live on the main thread, whose run loop delivers its messages.
        dispatch_sync(dispatch_get_main_queue(), ^{
            shared = [[PLCameras alloc] init];
        });
    });
    return shared;
}

static char *copy_string(NSString *text) {
    return strdup(text.UTF8String ?: "");
}

static char *json_string(id object) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:0 error:nil];
    if (!data) return copy_string(@"{\"error\":\"internal error\"}");
    return copy_string([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]);
}

static char *error_json(NSString *message) {
    return json_string(@{@"error" : message});
}

static NSNumber *millis(NSDate *date) {
    return date ? @((long long)(date.timeIntervalSince1970 * 1000.0)) : (id)[NSNull null];
}

/// Waits for `semaphore`, at most `seconds`; true if it was signalled.
static BOOL wait_for(dispatch_semaphore_t semaphore, double seconds) {
    return dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(seconds * NSEC_PER_SEC))) == 0;
}

void pl_camera_free(char *text) {
    free(text);
}

/// The connected devices: [{id, name, restricted}].
char *pl_camera_list(void) {
    PLCameras *shared = cameras();
    __block NSMutableArray *list = [NSMutableArray array];
    dispatch_sync(dispatch_get_main_queue(), ^{
        for (ICCameraDevice *device in shared.devices) {
            if (!device.UUIDString) continue;
            [shared openSessionOn:device];
            [list addObject:@{
                @"id" : device.UUIDString,
                @"name" : device.name ?: @"",
                // Locked, or "Trust this computer" not answered yet.
                @"restricted" : @([shared isRestricted:device]),
            }];
        }
    });
    return json_string(list);
}

/// Opens the device and lists its files once the list is complete:
/// [{name, folder, size, created, exif}] (dates in ms since 1970), or {error}.
char *pl_camera_files(const char *device_id, double timeout) {
    PLCameras *shared = cameras();
    NSString *uuid = [NSString stringWithUTF8String:device_id];

    __block ICCameraDevice *device = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        device = [shared deviceWithId:uuid];
        if (device) [shared openSessionOn:device];
    });
    if (!device) return error_json(@"not connected");

    // Wait for the complete list of files. A phone that stays locked (or
    // untrusted) for a while is reported as such; one that gives its list is
    // fine whatever it says.
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeout];
    NSDate *restrictedSince = nil;
    for (;;) {
        __block BOOL ready = NO;
        __block BOOL gone = NO;
        __block BOOL restricted = NO;
        dispatch_sync(dispatch_get_main_queue(), ^{
            gone = [shared deviceWithId:uuid] == nil;
            // Complete: the device said so, or has listed everything. (A
            // phone that said so while locked had nothing to list yet.)
            ready = device.mediaFiles.count > 0 &&
                    ([shared.ready containsObject:uuid] || device.contentCatalogPercentCompleted >= 100);
            restricted = [shared isRestricted:device];
            if (!gone && !ready) [shared openSessionOn:device];
        });
        if (gone) return error_json(@"not connected");
        if (ready) break;
        if (restricted) {
            if (!restrictedSince) restrictedSince = [NSDate date];
            if (-[restrictedSince timeIntervalSinceNow] > 15) return error_json(@"restricted");
        } else {
            restrictedSince = nil;
        }
        if ([deadline timeIntervalSinceNow] < 0) return error_json(@"the device doesn't answer");
        [NSThread sleepForTimeInterval:0.1];
    }

    __block NSMutableArray *list = [NSMutableArray array];
    dispatch_sync(dispatch_get_main_queue(), ^{
        [shared.catalogs removeObjectForKey:uuid]; // numbered afresh
        for (ICCameraFile *file in [shared catalogOf:device]) {
            [list addObject:@{
                @"name" : file.name ?: @"",
                @"folder" : file.parentFolder.name ?: @"",
                @"size" : @(file.fileSize),
                @"created" : millis(file.creationDate),
                @"exif" : millis(file.exifCreationDate),
            }];
        }
    });
    return json_string(list);
}

static ICCameraFile *file_at(PLCameras *shared, NSString *uuid, long index) {
    __block ICCameraFile *file = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        ICCameraDevice *device = [shared deviceWithId:uuid];
        if (!device) return;
        [shared openSessionOn:device];
        NSArray<ICCameraFile *> *catalog = [shared catalogOf:device];
        if (index >= 0 && index < (long)catalog.count) file = catalog[index];
    });
    return file;
}

/// Reads `length` bytes at `offset` of file `index` into `out`. Returns the
/// number of bytes read, or -1 (with `*error` set) on failure.
long long pl_camera_read(const char *device_id, long index, long long offset, long long length, uint8_t *out,
                         char **error) {
    PLCameras *shared = cameras();
    ICCameraFile *file = file_at(shared, [NSString stringWithUTF8String:device_id], index);
    if (!file) {
        *error = copy_string(@"the file is no longer on the device");
        return -1;
    }
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block long long got = -1;
    __block NSString *failure = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        [file requestReadDataAtOffset:offset
                               length:length
                           completion:^(NSData *data, NSError *readError) {
                               if (data && !readError) {
                                   got = MIN((long long)data.length, length);
                                   memcpy(out, data.bytes, (size_t)got);
                               } else {
                                   failure = readError.localizedDescription ?: @"can't read";
                               }
                               dispatch_semaphore_signal(done);
                           }];
    });
    if (!wait_for(done, 60)) {
        *error = copy_string(@"the device doesn't answer");
        return -1;
    }
    if (failure) *error = copy_string(failure);
    return got;
}

/// Downloads file `index` into `dir` as `name`. Returns NULL on success, else
/// an error message. Stops early when `*cancel` becomes non-zero.
char *pl_camera_download(const char *device_id, long index, const char *dir, const char *name,
                         const _Atomic int *cancel) {
    PLCameras *shared = cameras();
    ICCameraFile *file = file_at(shared, [NSString stringWithUTF8String:device_id], index);
    if (!file) return copy_string(@"the file is no longer on the device");
    NSDictionary *options = @{
        ICDownloadsDirectoryURL : [NSURL fileURLWithPath:[NSString stringWithUTF8String:dir] isDirectory:YES],
        ICSaveAsFilename : [NSString stringWithUTF8String:name],
        ICOverwrite : @YES,
    };
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block NSString *failure = nil;
    __block NSProgress *progress = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        progress = [file requestDownloadWithOptions:options
                                         completion:^(NSString *filename, NSError *error) {
                                             (void)filename;
                                             if (error) failure = error.localizedDescription ?: @"can't download";
                                             dispatch_semaphore_signal(done);
                                         }];
    });
    while (!wait_for(done, 0.2)) {
        if (atomic_load(cancel)) {
            [progress cancel];
            wait_for(done, 10);
            return copy_string(@"cancelled");
        }
        __block BOOL gone = NO;
        dispatch_sync(dispatch_get_main_queue(), ^{
            gone = [shared deviceWithId:[NSString stringWithUTF8String:device_id]] == nil;
        });
        if (gone) return copy_string(@"the device was disconnected");
    }
    return failure ? copy_string(failure) : NULL;
}

/// A JPEG thumbnail of file `index`, at most `max_side` pixels: returns its
/// length and puts it in `*out` (malloc, freed with `pl_camera_free`), or -1
/// with `*error` set.
long long pl_camera_thumbnail(const char *device_id, long index, long max_side, uint8_t **out, char **error) {
    PLCameras *shared = cameras();
    ICCameraFile *file = file_at(shared, [NSString stringWithUTF8String:device_id], index);
    if (!file) {
        *error = copy_string(@"the file is no longer on the device");
        return -1;
    }
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block NSData *jpeg = nil;
    __block NSString *failure = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        [file requestThumbnailDataWithOptions:@{ICImageSourceThumbnailMaxPixelSize : @(max_side)}
                                   completion:^(NSData *data, NSError *thumbError) {
                                       if (data.length > 0 && !thumbError) {
                                           jpeg = data;
                                       } else {
                                           failure = thumbError.localizedDescription ?: @"no thumbnail";
                                       }
                                       dispatch_semaphore_signal(done);
                                   }];
    });
    if (!wait_for(done, 30)) {
        *error = copy_string(@"the device doesn't answer");
        return -1;
    }
    if (!jpeg) {
        *error = copy_string(failure);
        return -1;
    }
    *out = malloc(jpeg.length);
    memcpy(*out, jpeg.bytes, jpeg.length);
    return (long long)jpeg.length;
}

/// Deletes files from the device. `wanted` is a JSON array of
/// [index, name, size]: a file is only deleted if the one at `index` in the
/// catalog still has that name and size (the phone may have changed since
/// it was looked at). Returns {deleted: [index…], failed: [[index, why]…]}
/// or {error}.
char *pl_camera_delete(const char *device_id, const char *wanted) {
    PLCameras *shared = cameras();
    NSString *uuid = [NSString stringWithUTF8String:device_id];
    NSData *data = [NSData dataWithBytes:wanted length:strlen(wanted)];
    NSArray *list = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    if (![list isKindOfClass:[NSArray class]]) return error_json(@"bad request");
    __block ICCameraDevice *device = nil;
    NSMutableArray<ICCameraFile *> *files = [NSMutableArray array];
    NSMapTable<ICCameraFile *, NSNumber *> *numbers = [NSMapTable strongToStrongObjectsMapTable];
    NSMutableArray *failed = [NSMutableArray array];
    dispatch_sync(dispatch_get_main_queue(), ^{
        device = [shared deviceWithId:uuid];
        if (!device) return;
        NSArray<ICCameraFile *> *catalog = [shared catalogOf:device];
        for (NSArray *one in list) {
            if (![one isKindOfClass:[NSArray class]] || one.count != 3) continue;
            NSNumber *number = one[0];
            NSString *name = one[1];
            NSNumber *size = one[2];
            long index = number.longValue;
            ICCameraFile *file = index >= 0 && index < (long)catalog.count ? catalog[index] : nil;
            if (!file || ![file.name isEqualToString:name] || file.fileSize != size.longLongValue) {
                [failed addObject:@[ number, @"changed on the device since it was looked at" ]];
                continue;
            }
            [files addObject:file];
            [numbers setObject:number forKey:file];
        }
    });
    if (!device) return error_json(@"not connected");
    if (files.count == 0) return json_string(@{@"deleted" : @[], @"failed" : failed});

    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    __block NSDictionary *result = nil;
    __block NSString *failure = nil;
    dispatch_sync(dispatch_get_main_queue(), ^{
        [device requestDeleteFiles:files
            deleteFailed:^(NSDictionary<ICDeleteError, ICCameraItem *> *failed) {
                (void)failed;
            }
            completion:^(NSDictionary<ICDeleteResult, NSArray<ICCameraItem *> *> *outcome, NSError *error) {
                result = outcome;
                if (error && !outcome) failure = error.localizedDescription ?: @"can't delete";
                dispatch_semaphore_signal(done);
            }];
    });
    // Deleting thousands of files takes a while.
    if (!wait_for(done, 1800)) return error_json(@"the device doesn't answer");
    if (failure) return error_json(failure);

    NSMutableArray *deleted = [NSMutableArray array];
    for (ICCameraItem *item in result[ICDeleteSuccessful]) {
        NSNumber *number = [numbers objectForKey:(ICCameraFile *)item];
        if (number) [deleted addObject:number];
    }
    for (ICCameraItem *item in result[ICDeleteFailed]) {
        NSNumber *number = [numbers objectForKey:(ICCameraFile *)item];
        if (number) [failed addObject:@[ number, @"the device refused" ]];
    }
    // The catalog is numbered afresh at the next listing.
    return json_string(@{@"deleted" : deleted, @"failed" : failed});
}
