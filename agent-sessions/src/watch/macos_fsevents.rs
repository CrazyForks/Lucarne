use std::ffi::CStr;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc as std_mpsc;
use std::thread;

use fsevent_sys as fs;
use fsevent_sys::core_foundation as cf;
use tokio::sync::mpsc;

use super::{RawWatchEvent, WatchError};

pub(super) struct MacRecursiveWatcher {
    paths: Vec<PathBuf>,
    raw_tx: mpsc::UnboundedSender<RawWatchEvent>,
    running: Option<RunningStream>,
}

impl MacRecursiveWatcher {
    pub(super) fn new(raw_tx: mpsc::UnboundedSender<RawWatchEvent>) -> Self {
        Self {
            paths: Vec::new(),
            raw_tx,
            running: None,
        }
    }

    pub(super) fn watch(&mut self, path: &Path) -> std::result::Result<(), WatchError> {
        let path = path.canonicalize().map_err(notify::Error::from)?;
        if self.paths.iter().any(|existing| existing == &path) {
            return Ok(());
        }
        self.stop();
        self.paths.push(path);
        self.start()
    }

    fn start(&mut self) -> std::result::Result<(), WatchError> {
        if self.paths.is_empty() {
            return Ok(());
        }
        let paths = create_path_array(&self.paths)?;
        let context = Box::into_raw(Box::new(StreamContext {
            raw_tx: self.raw_tx.clone(),
        }));
        let stream_context = fs::FSEventStreamContext {
            version: 0,
            info: context.cast(),
            retain: None,
            release: Some(release_context),
            copy_description: None,
        };
        let stream = unsafe {
            fs::FSEventStreamCreate(
                cf::kCFAllocatorDefault,
                callback,
                &stream_context,
                paths,
                fs::kFSEventStreamEventIdSinceNow,
                0.0,
                fs::kFSEventStreamCreateFlagFileEvents | fs::kFSEventStreamCreateFlagNoDefer,
            )
        };
        if stream.is_null() {
            unsafe {
                cf::CFRelease(paths);
                drop(Box::from_raw(context));
            }
            return Err(WatchError::Notify(notify::Error::generic(
                "failed to create FSEvents stream",
            )));
        }
        self.running = Some(RunningStream::start(stream, paths)?);
        Ok(())
    }

    fn stop(&mut self) {
        self.running.take();
    }
}

impl Drop for MacRecursiveWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

struct RunningStream {
    runloop: cf::CFRunLoopRef,
    paths: cf::CFMutableArrayRef,
    thread: Option<thread::JoinHandle<()>>,
}

unsafe impl Send for RunningStream {}
unsafe impl Send for MacRecursiveWatcher {}

impl RunningStream {
    fn start(
        stream: fs::FSEventStreamRef,
        paths: cf::CFMutableArrayRef,
    ) -> std::result::Result<Self, WatchError> {
        struct SendRef<T>(T);
        unsafe impl<T> Send for SendRef<T> {}
        impl<T: Copy> SendRef<T> {
            fn get(&self) -> T {
                self.0
            }
        }

        let stream = SendRef(stream);
        let (runloop_tx, runloop_rx) = std_mpsc::channel();
        let thread = thread::Builder::new()
            .name("lucarne-agent-fsevents".into())
            .spawn(move || {
                let stream = stream.get();
                unsafe {
                    let runloop = cf::CFRunLoopGetCurrent();
                    fs::FSEventStreamScheduleWithRunLoop(
                        stream,
                        runloop,
                        cf::kCFRunLoopDefaultMode,
                    );
                    fs::FSEventStreamStart(stream);
                    let _ = runloop_tx.send(SendRef(runloop));
                    cf::CFRunLoopRun();
                    fs::FSEventStreamStop(stream);
                    fs::FSEventStreamInvalidate(stream);
                    fs::FSEventStreamRelease(stream);
                }
            })
            .map_err(notify::Error::from)?;
        let runloop = runloop_rx
            .recv()
            .map_err(|error| notify::Error::generic(&error.to_string()))?
            .0;
        Ok(Self {
            runloop,
            paths,
            thread: Some(thread),
        })
    }
}

impl Drop for RunningStream {
    fn drop(&mut self) {
        unsafe {
            while CFRunLoopIsWaiting(self.runloop) == 0 {
                thread::yield_now();
            }
            cf::CFRunLoopStop(self.runloop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            cf::CFRelease(self.paths);
        }
    }
}

struct StreamContext {
    raw_tx: mpsc::UnboundedSender<RawWatchEvent>,
}

extern "C" fn release_context(info: *const c_void) {
    if info.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(info.cast_mut().cast::<StreamContext>()));
    }
}

extern "C" fn callback(
    _stream_ref: fs::FSEventStreamRef,
    info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const fs::FSEventStreamEventFlags,
    _event_ids: *const fs::FSEventStreamEventId,
) {
    if info.is_null() || event_paths.is_null() {
        return;
    }
    let context = unsafe { &*info.cast::<StreamContext>() };
    let event_paths = event_paths.cast::<*const std::os::raw::c_char>();
    let mut paths = Vec::new();
    for index in 0..num_events {
        let flags = unsafe { *event_flags.add(index) };
        if flags & fs::kFSEventStreamEventFlagHistoryDone != 0 {
            continue;
        }
        let raw_path = unsafe { *event_paths.add(index) };
        if raw_path.is_null() {
            continue;
        }
        let Ok(path) = unsafe { CStr::from_ptr(raw_path) }.to_str() else {
            continue;
        };
        paths.push(PathBuf::from(path));
    }
    if !paths.is_empty() {
        let _ = context.raw_tx.send(RawWatchEvent::Paths(paths));
    }
}

fn create_path_array(paths: &[PathBuf]) -> std::result::Result<cf::CFMutableArrayRef, WatchError> {
    let array =
        unsafe { cf::CFArrayCreateMutable(cf::kCFAllocatorDefault, 0, &cf::kCFTypeArrayCallBacks) };
    if array.is_null() {
        return Err(WatchError::Notify(notify::Error::generic(
            "failed to allocate FSEvents path array",
        )));
    }
    for path in paths {
        append_cf_path(array, path)?;
    }
    Ok(array)
}

fn append_cf_path(
    array: cf::CFMutableArrayRef,
    path: &Path,
) -> std::result::Result<(), WatchError> {
    let Some(path) = path.to_str() else {
        return Err(WatchError::Notify(notify::Error::generic(
            "FSEvents path is not valid UTF-8",
        )));
    };
    let mut error: cf::CFErrorRef = ptr::null_mut();
    let cf_path = unsafe { cf::str_path_to_cfstring_ref(path, &mut error) };
    if cf_path.is_null() {
        if !error.is_null() {
            unsafe {
                cf::CFRelease(error.cast());
            }
        }
        return Err(WatchError::Notify(notify::Error::path_not_found()));
    }
    unsafe {
        cf::CFArrayAppendValue(array, cf_path);
        cf::CFRelease(cf_path);
    }
    Ok(())
}

unsafe extern "C" {
    fn CFRunLoopIsWaiting(runloop: cf::CFRunLoopRef) -> cf::Boolean;
}
