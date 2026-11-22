//! Read immutable books through one synchronous OPFS handle per open file.

use gashuu_core::{ArchivePolicy, ReadAt};
use gashuu_worker::{
    book_path,
    import::{
        fits_in_quota, probe_upload, sanitise_upload_name, upload_title, STORAGE_FULL,
        UNREADABLE_UPLOAD,
    },
    lifecycle::{content_key, head_tail_ranges, is_content_key},
    protocol::ImportedMeta,
    wire_error::WireError,
};
use js_sys::{Promise, Reflect, Uint8Array};
use std::{
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    sync::Arc,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DedicatedWorkerGlobalScope, DomException, File, FileReaderSync, FileSystemDirectoryHandle,
    FileSystemFileHandle, FileSystemGetDirectoryOptions, FileSystemGetFileOptions,
    FileSystemReadWriteOptions, FileSystemRemoveOptions, FileSystemSyncAccessHandle,
    StorageManager,
};

const CONTENT_KEY_FILE: &str = ".content-key";

pub struct OpfsReadAt {
    handle: FileSystemSyncAccessHandle,
    len: u64,
}

impl ReadAt for OpfsReadAt {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let options = FileSystemReadWriteOptions::new();
        options.set_at_f64(offset as f64);
        self.handle
            .read_with_u8_array_and_options(buf, &options)
            .map(|len| len as usize)
            .map_err(|error| io::Error::other(format!("{error:?}")))
    }
}

impl Drop for OpfsReadAt {
    fn drop(&mut self) {
        self.handle.close();
    }
}

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
// SAFETY: without wasm atomics, this module runs on one thread and its JS handle
// cannot be moved to another thread.
unsafe impl Send for OpfsReadAt {}

#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
// SAFETY: without wasm atomics, this module runs on one thread and its JS handle
// cannot be accessed concurrently by another thread.
unsafe impl Sync for OpfsReadAt {}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
compile_error!("OpfsReadAt's Send/Sync impls are only sound without wasm threads");

pub async fn open_book(id: &str) -> Result<OpfsReadAt, WireError> {
    let path = book_path::parse(id)?;
    open(&path).await.map_err(|error| map_error(id, error))
}

async fn open(path: &book_path::BookPath) -> Result<OpfsReadAt, JsValue> {
    let scope = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>()?;
    let books = books_directory(&scope.navigator().storage(), false).await?;
    let book: FileSystemDirectoryHandle = resolve(books.get_directory_handle(&path.uuid)).await?;
    let file: FileSystemFileHandle = resolve(book.get_file_handle(&path.name)).await?;
    let mut reader = OpfsReadAt {
        handle: resolve(file.create_sync_access_handle()).await?,
        len: 0,
    };
    reader.len = reader.handle.get_size()? as u64;
    Ok(reader)
}

async fn resolve<T: JsCast>(promise: Promise) -> Result<T, JsValue> {
    JsFuture::from(promise).await?.dyn_into()
}

pub fn map_error(id: &str, error: JsValue) -> WireError {
    if let Some(exception) = error.dyn_ref::<DomException>() {
        return match exception.name().as_str() {
            "QuotaExceededError" => storage_full(),
            "SecurityError" | "NotSupportedError" => WireError::Other {
                detail: "this browser cannot store books".into(),
            },
            "NotFoundError" => WireError::Missing {
                detail: format!("{id} not found in browser storage"),
            },
            "NoModificationAllowedError" => WireError::Other {
                detail: "book is open in another tab".into(),
            },
            name => WireError::Other {
                detail: format!("{name}: {}", exception.message()),
            },
        };
    }
    WireError::Other {
        detail: error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

const IMPORT_SLICE: u64 = 8 * 1024 * 1024;

pub struct Upload {
    id: String,
    uuid: String,
    books: FileSystemDirectoryHandle,
    name: String,
    file: File,
    reader: OpfsReadAt,
    offset: u64,
}

impl Upload {
    pub fn write_slice(&mut self) -> Result<bool, WireError> {
        if self.offset == self.reader.len {
            return Ok(false);
        }
        let end = self
            .offset
            .saturating_add(IMPORT_SLICE)
            .min(self.reader.len);
        let read = || -> Result<js_sys::ArrayBuffer, JsValue> {
            let blob = self
                .file
                .slice_with_f64_and_f64(self.offset as f64, end as f64)?;
            FileReaderSync::new()?.read_as_array_buffer(&blob)
        };
        let buffer = read().map_err(|error| {
            web_sys::console::warn_2(&UNREADABLE_UPLOAD.into(), &error);
            WireError::Other {
                detail: UNREADABLE_UPLOAD.into(),
            }
        })?;
        let options = FileSystemReadWriteOptions::new();
        options.set_at_f64(self.offset as f64);
        let written = self
            .reader
            .handle
            .write_with_js_u8_array_and_options(&Uint8Array::new(&buffer), &options)
            .map_err(|error| map_error(&self.id, error))?;
        if written != (end - self.offset) as f64 {
            return Err(WireError::Other {
                detail: "short write to browser storage".into(),
            });
        }
        self.offset = end;
        Ok(true)
    }

    pub async fn finish(self, allow_rar: bool, key: &str) -> Result<ImportedMeta, WireError> {
        let Self {
            id,
            uuid,
            books,
            name,
            reader,
            ..
        } = self;
        let result = async {
            reader
                .handle
                .flush()
                .map_err(|error| map_error(&id, error))?;
            let count = probe_upload(&name, Arc::new(reader), ArchivePolicy { allow_rar })?;
            let directory = resolve(books.get_directory_handle(&uuid))
                .await
                .map_err(|e| map_error(&id, e))?;
            write_key(&directory, key)
                .await
                .map_err(|e| map_error(&id, e))?;
            Ok(ImportedMeta {
                duplicate: false,
                id,
                title: upload_title(&name),
                page_count: count.get() as u32,
            })
        }
        .await;
        if result.is_err() {
            remove_directory(&books, &uuid).await;
        }
        result
    }

    pub async fn abort(self) {
        let Self {
            books,
            uuid,
            reader,
            ..
        } = self;
        drop(reader);
        remove_directory(&books, &uuid).await;
    }
}

pub async fn begin_import(
    scope: &DedicatedWorkerGlobalScope,
    name: &str,
    size: u64,
    file: File,
) -> Result<Upload, WireError> {
    let storage = scope.navigator().storage();
    if let Ok(promise) = storage.estimate() {
        if let Ok(estimate) = JsFuture::from(promise).await {
            let bytes = |key: &str| {
                let value = Reflect::get(&estimate, &key.into()).ok()?.as_f64()?;
                (value.is_finite() && value >= 0.0).then_some(value as u64)
            };
            if !fits_in_quota(size, bytes("usage"), bytes("quota")) {
                return Err(storage_full());
            }
        }
    }
    let name = sanitise_upload_name(name);
    let uuid = scope
        .crypto()
        .map_err(|error| map_error(&name, error))?
        .random_uuid();
    let id = format!("books/{uuid}/{name}");
    let books = books_directory(&storage, true)
        .await
        .map_err(|error| map_error(&id, error))?;
    let dir = create_dir(&books, &uuid)
        .await
        .map_err(|error| map_error(&id, error))?;
    match create_file(&dir, &name).await {
        Ok(handle) => Ok(Upload {
            id,
            uuid,
            books,
            name,
            file,
            reader: OpfsReadAt { handle, len: size },
            offset: 0,
        }),
        Err(error) => {
            remove_directory(&books, &uuid).await;
            Err(map_error(&id, error))
        }
    }
}

async fn create_file(
    dir: &FileSystemDirectoryHandle,
    name: &str,
) -> Result<FileSystemSyncAccessHandle, JsValue> {
    let options = FileSystemGetFileOptions::new();
    options.set_create(true);
    let file: FileSystemFileHandle =
        resolve(dir.get_file_handle_with_options(name, &options)).await?;
    if !Reflect::get(&file, &"createSyncAccessHandle".into())?.is_function() {
        return Err("this browser cannot store books".into());
    }
    resolve(file.create_sync_access_handle()).await
}

async fn books_directory(
    storage: &StorageManager,
    create: bool,
) -> Result<FileSystemDirectoryHandle, JsValue> {
    let root: FileSystemDirectoryHandle = resolve(storage.get_directory()).await?;
    if create {
        create_dir(&root, "books").await
    } else {
        resolve(root.get_directory_handle("books")).await
    }
}

async fn create_dir(
    dir: &FileSystemDirectoryHandle,
    name: &str,
) -> Result<FileSystemDirectoryHandle, JsValue> {
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    resolve(dir.get_directory_handle_with_options(name, &options)).await
}

pub async fn delete_book(scope: &DedicatedWorkerGlobalScope, id: &str) -> bool {
    let Ok(path) = book_path::parse(id) else {
        return false;
    };
    let result = async {
        let books = books_directory(&scope.navigator().storage(), false).await?;
        remove_entry(&books, &path.uuid).await
    }
    .await;
    match result {
        Ok(()) => true,
        Err(e) if named(&e, "NotFoundError") => true,
        Err(e) => {
            web_sys::console::warn_2(&"could not delete stored book".into(), &e);
            false
        }
    }
}

pub fn named(error: &JsValue, name: &str) -> bool {
    error
        .dyn_ref::<DomException>()
        .is_some_and(|e| e.name() == name)
}

pub async fn remove_entry(books: &FileSystemDirectoryHandle, uuid: &str) -> Result<(), JsValue> {
    let options = FileSystemRemoveOptions::new();
    options.set_recursive(true);
    JsFuture::from(books.remove_entry_with_options(uuid, &options)).await?;
    Ok(())
}

async fn remove_directory(books: &FileSystemDirectoryHandle, uuid: &str) {
    if let Err(e) = remove_entry(books, uuid).await {
        if !named(&e, "NotFoundError") {
            web_sys::console::warn_2(&"could not clean up browser book storage".into(), &e);
        }
    }
}

pub fn picked_key(file: &File, size: u64) -> Result<String, WireError> {
    let read = |range: Range<u64>| -> Result<Vec<u8>, JsValue> {
        let blob = file.slice_with_f64_and_f64(range.start as f64, range.end as f64)?;
        let buffer = FileReaderSync::new()?.read_as_array_buffer(&blob)?;
        Ok(Uint8Array::new(&buffer).to_vec())
    };
    let (head, tail) = head_tail_ranges(size);
    let unreadable = |_: JsValue| WireError::Other {
        detail: UNREADABLE_UPLOAD.into(),
    };
    Ok(content_key(
        size,
        &read(head).map_err(unreadable)?,
        &read(tail).map_err(unreadable)?,
    ))
}

async fn next_name(iter: &js_sys::AsyncIterator) -> Result<Option<String>, JsValue> {
    let item: js_sys::IteratorNext = JsFuture::from(iter.next()?).await?.unchecked_into();
    if item.done() {
        Ok(None)
    } else {
        item.value()
            .as_string()
            .map(Some)
            .ok_or_else(|| "invalid directory entry".into())
    }
}

pub struct BookDirs {
    pub books: FileSystemDirectoryHandle,
    keys: js_sys::AsyncIterator,
}

impl BookDirs {
    pub async fn open(scope: &DedicatedWorkerGlobalScope) -> Result<Option<Self>, JsValue> {
        match books_directory(&scope.navigator().storage(), false).await {
            Ok(books) => Ok(Some(Self {
                keys: books.keys(),
                books,
            })),
            Err(e) if named(&e, "NotFoundError") => Ok(None),
            Err(e) => Err(e),
        }
    }
    /// Returns root iterator errors separately from errors reading one child directory.
    pub async fn next(&self) -> Result<Option<Result<BookDir, JsValue>>, JsValue> {
        while let Some(uuid) = next_name(&self.keys).await? {
            let dir: FileSystemDirectoryHandle =
                match resolve(self.books.get_directory_handle(&uuid)).await {
                    Ok(dir) => dir,
                    Err(e) if named(&e, "NotFoundError") || named(&e, "TypeMismatchError") => {
                        continue
                    }
                    Err(e) => return Ok(Some(Err(e))),
                };
            return Ok(Some(BookDir::read(uuid, dir).await));
        }
        Ok(None)
    }
}

pub struct BookDir {
    pub uuid: String,
    dir: FileSystemDirectoryHandle,
    file: Option<(String, FileSystemFileHandle)>,
}

impl BookDir {
    async fn read(uuid: String, dir: FileSystemDirectoryHandle) -> Result<Self, JsValue> {
        let keys = dir.keys();
        let mut file = None;
        while let Some(name) = next_name(&keys).await? {
            if name.starts_with('.') {
                continue;
            }
            let handle = match resolve(dir.get_file_handle(&name)).await {
                Ok(handle) => handle,
                Err(e) if named(&e, "NotFoundError") => continue,
                Err(e) => return Err(e),
            };
            file = Some((name, handle));
            break;
        }
        Ok(Self { uuid, dir, file })
    }

    pub async fn last_modified(&self) -> Result<Option<f64>, JsValue> {
        let handle = match &self.file {
            Some((_, handle)) => handle.clone(),
            None => match resolve(self.dir.get_file_handle(CONTENT_KEY_FILE)).await {
                Ok(handle) => handle,
                Err(e) if named(&e, "NotFoundError") || named(&e, "TypeMismatchError") => {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            },
        };
        let file: File = resolve(handle.get_file()).await?;
        Ok(Some(file.last_modified()))
    }

    pub fn id(&self) -> Option<String> {
        self.file
            .as_ref()
            .map(|(name, _)| format!("books/{}/{name}", self.uuid))
    }

    pub async fn stored_key(
        &self,
        id: &str,
        shared: Option<Arc<dyn ReadAt>>,
    ) -> Result<String, WireError> {
        let sidecar: Result<FileSystemFileHandle, _> =
            resolve(self.dir.get_file_handle(CONTENT_KEY_FILE)).await;
        match sidecar {
            Ok(handle) => {
                let file: File = resolve(handle.get_file())
                    .await
                    .map_err(|e| map_error(id, e))?;
                if file.size() == 64. {
                    let key = JsFuture::from(file.text())
                        .await
                        .map_err(|e| map_error(id, e))?
                        .as_string()
                        .unwrap_or_default();
                    if is_content_key(&key) {
                        return Ok(key);
                    }
                }
            }
            Err(e) if named(&e, "NotFoundError") => {}
            Err(e) => return Err(map_error(id, e)),
        }
        let reader = match shared {
            Some(reader) => reader,
            None => Arc::new(open_book(id).await?),
        };
        let size = reader.len();
        let mut cursor = gashuu_core::ReadAtCursor::new(reader);
        let mut read = |range: Range<u64>| -> Result<Vec<u8>, WireError> {
            let mut bytes = vec![0; (range.end - range.start) as usize];
            cursor
                .seek(SeekFrom::Start(range.start))
                .and_then(|_| cursor.read_exact(&mut bytes))
                .map_err(|e| WireError::Other {
                    detail: e.to_string(),
                })?;
            Ok(bytes)
        };
        let (head, tail) = head_tail_ranges(size);
        let key = content_key(size, &read(head)?, &read(tail)?);
        write_key(&self.dir, &key)
            .await
            .map_err(|e| map_error(id, e))?;
        Ok(key)
    }
}

async fn write_key(dir: &FileSystemDirectoryHandle, key: &str) -> Result<(), JsValue> {
    let handle = create_file(dir, CONTENT_KEY_FILE).await?;
    let result = (|| {
        handle.truncate_with_u32(0)?;
        if handle.write_with_u8_array(key.as_bytes())? != key.len() as f64 {
            return Err("short content key write".into());
        }
        handle.flush()
    })();
    handle.close();
    result
}

fn storage_full() -> WireError {
    WireError::Other {
        detail: STORAGE_FULL.into(),
    }
}
