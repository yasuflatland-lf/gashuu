//! Read immutable books through one synchronous OPFS handle per open file.

use gashuu_core::ReadAt;
use gashuu_worker::{book_path, wire_error::WireError};
use js_sys::Promise;
use std::io;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    DedicatedWorkerGlobalScope, DomException, FileSystemDirectoryHandle, FileSystemFileHandle,
    FileSystemReadWriteOptions, FileSystemSyncAccessHandle,
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
    let root: FileSystemDirectoryHandle =
        resolve(scope.navigator().storage().get_directory()).await?;
    let books: FileSystemDirectoryHandle = resolve(root.get_directory_handle("books")).await?;
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
