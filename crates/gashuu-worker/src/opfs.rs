//! Read immutable books through one synchronous OPFS handle per open file.

use gashuu_core::{ArchivePolicy, ReadAt};
use gashuu_worker::{
    book_path,
    import::{
        fits_in_quota, probe_upload, sanitise_upload_name, upload_title, STORAGE_FULL,
        UNREADABLE_UPLOAD,
    },
    protocol::ImportedMeta,
    wire_error::WireError,
};
use js_sys::{Promise, Reflect, Uint8Array};
use std::{io, sync::Arc};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DedicatedWorkerGlobalScope, DomException, File, FileReaderSync, FileSystemDirectoryHandle,
    FileSystemFileHandle, FileSystemGetDirectoryOptions, FileSystemGetFileOptions,
    FileSystemReadWriteOptions, FileSystemRemoveOptions, FileSystemSyncAccessHandle,
    StorageManager,
};

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

fn map_error(id: &str, error: JsValue) -> WireError {
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

    pub async fn finish(self, allow_rar: bool) -> Result<ImportedMeta, WireError> {
        let Self {
            id,
            uuid,
            books,
            name,
            reader,
            ..
        } = self;
        let result = (|| {
            reader
                .handle
                .flush()
                .map_err(|error| map_error(&id, error))?;
            let count = probe_upload(&name, Arc::new(reader), ArchivePolicy { allow_rar })?;
            Ok(ImportedMeta {
                id,
                title: upload_title(&name),
                page_count: count.get() as u32,
            })
        })();
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

pub async fn delete_book(scope: &DedicatedWorkerGlobalScope, id: &str) {
    if let Ok(path) = book_path::parse(id) {
        if let Ok(books) = books_directory(&scope.navigator().storage(), false).await {
            remove_directory(&books, &path.uuid).await;
        }
    }
}

async fn remove_directory(books: &FileSystemDirectoryHandle, uuid: &str) {
    let options = FileSystemRemoveOptions::new();
    options.set_recursive(true);
    if let Err(error) = JsFuture::from(books.remove_entry_with_options(uuid, &options)).await {
        if !error
            .dyn_ref::<DomException>()
            .is_some_and(|e| e.name() == "NotFoundError")
        {
            web_sys::console::warn_2(&"could not clean up browser book storage".into(), &error);
        }
    }
}

fn storage_full() -> WireError {
    WireError::Other {
        detail: STORAGE_FULL.into(),
    }
}

async fn covers_directory() -> Result<FileSystemDirectoryHandle, JsValue> {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().dyn_into()?;
    let root: FileSystemDirectoryHandle =
        resolve(scope.navigator().storage().get_directory()).await?;
    create_dir(&root, "covers").await
}

pub async fn read_cover(name: &str) -> Option<Vec<u8>> {
    let dir = covers_directory().await.ok()?;
    let file: FileSystemFileHandle = resolve(dir.get_file_handle(name)).await.ok()?;
    let handle: FileSystemSyncAccessHandle =
        resolve(file.create_sync_access_handle()).await.ok()?;
    let result = (|| {
        let size = handle.get_size().ok()? as usize;
        let mut bytes = vec![0; size];
        let count = handle.read_with_u8_array(&mut bytes).ok()? as usize;
        (count == size).then_some(bytes)
    })();
    handle.close();
    result
}

pub async fn write_cover(name: &str, bytes: &[u8]) -> Result<(), JsValue> {
    let dir = covers_directory().await?;
    let handle = create_file(&dir, name).await?;
    let result = (|| {
        handle.truncate_with_u32(0)?;
        let count = handle.write_with_u8_array(bytes)? as usize;
        if count != bytes.len() {
            return Err("short cover write".into());
        }
        handle.flush()
    })();
    handle.close();
    result
}

pub async fn remove_cover(name: &str) -> Result<(), JsValue> {
    let dir = covers_directory().await?;
    JsFuture::from(dir.remove_entry(name)).await?;
    Ok(())
}

pub async fn list_covers() -> Result<Vec<(String, u64, f64)>, JsValue> {
    let dir = covers_directory().await?;
    let values: js_sys::Function = Reflect::get(&dir, &"values".into())?.dyn_into()?;
    let iterator = values.call0(&dir)?;
    let next: js_sys::Function = Reflect::get(&iterator, &"next".into())?.dyn_into()?;
    let mut files = Vec::new();
    loop {
        let promise: Promise = next.call0(&iterator)?.dyn_into()?;
        let entry = JsFuture::from(promise).await?;
        if Reflect::get(&entry, &"done".into())?
            .as_bool()
            .unwrap_or(false)
        {
            break;
        }
        let value = Reflect::get(&entry, &"value".into())?;
        let Ok(handle) = value.dyn_into::<FileSystemFileHandle>() else {
            continue;
        };
        let file: File = resolve(handle.get_file()).await?;
        files.push((file.name(), file.size() as u64, file.last_modified()));
    }
    Ok(files)
}

pub async fn clear_covers() -> Result<(), JsValue> {
    for (name, _, _) in list_covers().await? {
        remove_cover(&name).await?;
    }
    Ok(())
}
