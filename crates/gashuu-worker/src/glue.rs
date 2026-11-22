//! Bridge worker messages to the portable dispatcher.

use crate::opfs;
use gashuu_core::{ArchivePolicy, ImageCratePageDecoder, ReadAt};
use gashuu_web_decode::NativePageDecoder;
use gashuu_worker::{
    dispatcher::{Dispatcher, Lane, Reply},
    protocol::{self, ImportedMeta, WireResult, WorkerRequest, WorkerResponse},
    wire_error::WireError,
};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{DedicatedWorkerGlobalScope, File, MessageEvent};

#[derive(Default)]
struct Inbox {
    interactive: VecDeque<WorkerRequest>,
    background: VecDeque<(WorkerRequest, Option<File>)>,
    running: bool,
}

impl Inbox {
    fn push(&mut self, request: WorkerRequest, file: Option<File>) -> bool {
        match Dispatcher::lane(&request) {
            Lane::Interactive => self.interactive.push_back(request),
            Lane::Background => self.background.push_back((request, file)),
        }
        // Only the callback that transitions an idle inbox to running starts a drain.
        !std::mem::replace(&mut self.running, true)
    }

    fn pop(&mut self) -> Option<(WorkerRequest, Option<File>)> {
        let request = self
            .interactive
            .pop_front()
            .map(|request| (request, None))
            .or_else(|| self.background.pop_front());
        self.running = request.is_some();
        request
    }

    fn only_background_pending(&self) -> bool {
        self.interactive.is_empty() && !self.background.is_empty()
    }
}

struct Worker {
    dispatcher: Dispatcher,
    content_index: Option<HashMap<String, String>>,
}

pub fn start() {
    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let dispatcher = Dispatcher::new(Rc::new(NativePageDecoder), Rc::new(ImageCratePageDecoder))
        .with_fallback_hook(Rc::new(|error| {
            web_sys::console::warn_1(
                &format!("native page decode failed, used wasm fallback: {error}").into(),
            );
        }));
    let control = dispatcher.control();
    let dispatcher = Rc::new(RefCell::new(Some(Worker {
        dispatcher,
        content_index: None,
    })));
    let inbox = Rc::new(RefCell::new(Inbox::default()));
    let callback = Closure::<dyn Fn(MessageEvent)>::new({
        let scope = scope.clone();
        move |event: MessageEvent| {
            let envelope = event.data();
            let request = match read_request(&envelope) {
                Ok(request) => request,
                Err(error) => {
                    post(
                        &scope,
                        Reply::metadata(WorkerResponse::Rejected { req: None, error }),
                    );
                    return;
                }
            };
            let file = if matches!(request, WorkerRequest::Import { .. }) {
                Reflect::get(&envelope, &"f".into())
                    .ok()
                    .and_then(|file| file.dyn_into().ok())
            } else {
                None
            };
            if !control.apply_control(&request) && inbox.borrow_mut().push(request, file) {
                let worker = dispatcher
                    .borrow_mut()
                    .take()
                    .expect("idle dispatcher is available");
                spawn_local(drain(
                    scope.clone(),
                    dispatcher.clone(),
                    worker,
                    inbox.clone(),
                ));
            }
        }
    });
    scope.set_onmessage(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    post(&scope, Reply::metadata(WorkerResponse::Ready));
}

async fn drain(
    scope: DedicatedWorkerGlobalScope,
    dispatcher: Rc<RefCell<Option<Worker>>>,
    mut worker: Worker,
    inbox: Rc<RefCell<Inbox>>,
) {
    loop {
        let request = inbox.borrow_mut().pop();
        let Some((request, file)) = request else {
            *dispatcher.borrow_mut() = Some(worker);
            return;
        };
        let lane = Dispatcher::lane(&request);
        match request {
            WorkerRequest::Import {
                req,
                name,
                size,
                allow_rar,
            } => {
                let result =
                    import(&scope, &mut worker, &inbox, &name, size, allow_rar, file).await;
                post(
                    &scope,
                    Reply::metadata(WorkerResponse::Imported { req, result }),
                );
            }
            WorkerRequest::DeleteBook { id } => {
                if worker.dispatcher.reader_for(&id).is_none()
                    && opfs::delete_book(&scope, &id).await
                {
                    if let Some(index) = &mut worker.content_index {
                        index.retain(|_, stored| stored != &id);
                    }
                }
            }
            WorkerRequest::Sweep { req, keep, now_ms } => {
                let response = sweep(
                    &scope,
                    &mut worker,
                    &inbox,
                    req,
                    keep.into_iter().collect(),
                    now_ms,
                )
                .await
                .unwrap_or_else(|error| WorkerResponse::Rejected {
                    req: Some(req),
                    error: opfs::map_error("books", error),
                });
                post(&scope, Reply::metadata(response));
            }
            request => serve(&scope, &mut worker.dispatcher, request).await,
        }
        if lane == Lane::Background && inbox.borrow().only_background_pending() {
            next_turn(&scope).await;
        }
    }
}

async fn open_reader(worker: &Dispatcher, id: &str) -> WireResult<Arc<dyn ReadAt>> {
    match worker.reader_for(id) {
        Some(reader) => Ok(reader),
        None => opfs::open_book(id)
            .await
            .map(|reader| Arc::new(reader) as Arc<dyn ReadAt>),
    }
}

async fn serve(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Dispatcher,
    request: WorkerRequest,
) {
    if worker.is_obsolete(&request) {
        return;
    }
    let reader = match Dispatcher::needs_reader(&request) {
        Some(id) => Some(open_reader(worker, id).await),
        None => None,
    };
    // handle checks obsolescence again after the asynchronous open.
    for reply in worker.handle(request, reader).await {
        post(scope, reply);
    }
}

async fn import(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Worker,
    inbox: &RefCell<Inbox>,
    name: &str,
    size: u64,
    allow_rar: bool,
    file: Option<File>,
) -> WireResult<ImportedMeta> {
    let file = file
        .filter(|file| file.size() == size as f64)
        .ok_or_else(|| WireError::Other {
            detail: "protocol: missing File or incorrect upload size".into(),
        })?;
    let key = opfs::picked_key(&file, size)?;
    if worker.content_index.is_none() {
        let index = build_index(scope, &mut worker.dispatcher, inbox).await?;
        worker.content_index = Some(index);
    }
    let index = worker.content_index.as_mut().expect("initialized above");
    if let Some(id) = index.get(&key) {
        match open_reader(&worker.dispatcher, id).await {
            Ok(reader) => {
                let path = gashuu_worker::book_path::parse(id)?;
                let pages = gashuu_worker::import::probe_upload(
                    &path.name,
                    reader,
                    ArchivePolicy { allow_rar },
                )?;
                return Ok(ImportedMeta {
                    id: id.clone(),
                    title: gashuu_worker::import::upload_title(&path.name),
                    page_count: pages.get() as u32,
                    duplicate: true,
                });
            }
            Err(WireError::Missing { .. }) => {
                index.remove(&key);
            }
            Err(e) => return Err(e),
        }
    }
    let mut upload = opfs::begin_import(scope, name, size, file).await?;
    loop {
        yield_to_readers(scope, &mut worker.dispatcher, inbox).await;
        match upload.write_slice() {
            Ok(true) => {}
            Ok(false) => {
                let meta = upload.finish(allow_rar, &key).await?;
                index.insert(key, meta.id.clone());
                return Ok(meta);
            }
            Err(error) => {
                upload.abort().await;
                return Err(error);
            }
        }
    }
}

async fn yield_to_readers(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Dispatcher,
    inbox: &RefCell<Inbox>,
) {
    next_turn(scope).await;
    loop {
        let request = inbox.borrow_mut().interactive.pop_front();
        let Some(request) = request else { break };
        serve(scope, worker, request).await;
    }
    // Allow one queued cover per directory or upload slice. Storage mutations
    // stay in submission order, including imports queued behind a sweep.
    let cover = {
        let mut inbox = inbox.borrow_mut();
        let position = inbox
            .background
            .iter()
            .position(|(req, _)| matches!(req, WorkerRequest::Cover { .. }));
        position.and_then(|i| inbox.background.remove(i))
    };
    if let Some((request, _)) = cover {
        serve(scope, worker, request).await;
    }
}

async fn build_index(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Dispatcher,
    inbox: &RefCell<Inbox>,
) -> WireResult<HashMap<String, String>> {
    let mut index = HashMap::new();
    let map_error = |e| opfs::map_error("books", e);
    if let Some(dirs) = opfs::BookDirs::open(scope).await.map_err(map_error)? {
        while let Some(dir) = dirs.next().await.map_err(map_error)? {
            if let Some(id) = dir.id() {
                match dir.stored_key(&id, worker.reader_for(&id)).await {
                    Ok(key) => {
                        index.entry(key).or_insert(id);
                    }
                    Err(WireError::Missing { .. }) => {}
                    Err(error) => return Err(error),
                }
            }
            yield_to_readers(scope, worker, inbox).await;
        }
    }
    Ok(index)
}

async fn sweep(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Worker,
    inbox: &RefCell<Inbox>,
    req: gashuu_worker::protocol::ReqId,
    keep: HashSet<String>,
    now_ms: Option<f64>,
) -> Result<WorkerResponse, JsValue> {
    let mut inventory = gashuu_worker::lifecycle::SweepInventory::default();
    let mut removed = 0;
    if let Some(dirs) = opfs::BookDirs::open(scope).await? {
        while let Some(dir) = dirs.next().await? {
            yield_to_readers(scope, &mut worker.dispatcher, inbox).await;
            let id = dir.id();
            let modified = match &dir.file {
                Some((_, handle))
                    if now_ms.is_some() && !id.as_ref().is_some_and(|id| keep.contains(id)) =>
                {
                    match JsFuture::from(handle.get_file()).await {
                        Ok(value) => value.dyn_into::<File>()?.last_modified(),
                        Err(e) if opfs::named(&e, "NotFoundError") => continue,
                        Err(e) if opfs::named(&e, "NoModificationAllowedError") => {
                            inventory.locked(id);
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                }
                _ => 0.,
            };
            if !inventory.should_delete(id.as_deref(), modified, &keep, now_ms) {
                continue;
            }
            if id
                .as_deref()
                .is_some_and(|id| worker.dispatcher.reader_for(id).is_some())
            {
                inventory.locked(id);
                continue;
            }
            match opfs::remove_entry(&dirs.books, &dir.uuid).await {
                Ok(()) => removed += 1,
                Err(e) if opfs::named(&e, "NotFoundError") => {}
                Err(e) if opfs::named(&e, "NoModificationAllowedError") => inventory.locked(id),
                Err(e) => return Err(e),
            }
        }
    }
    if let Some(index) = &mut worker.content_index {
        let present: HashSet<_> = inventory.present.iter().collect();
        index.retain(|_, id| present.contains(id));
    }
    Ok(WorkerResponse::Swept {
        req,
        removed,
        skipped_recent: inventory.skipped_recent,
        skipped_locked: inventory.skipped_locked,
        present: inventory.present,
    })
}

async fn next_turn(scope: &DedicatedWorkerGlobalScope) {
    // A synchronous slice or probe can finish without letting onmessage run.
    let turn = Promise::new(&mut |resolve, _| {
        scope
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
            .expect("schedule worker inbox turn");
    });
    JsFuture::from(turn)
        .await
        .expect("worker inbox turn resolves");
}

fn read_request(envelope: &JsValue) -> Result<WorkerRequest, WireError> {
    let metadata = Reflect::get(envelope, &"m".into())
        .ok()
        .and_then(|value| value.as_string())
        .ok_or_else(|| WireError::Other {
            detail: "protocol: expected a string in envelope.m".into(),
        })?;
    protocol::decode(&metadata)
}

fn post(scope: &DedicatedWorkerGlobalScope, reply: Reply) {
    let envelope = Object::new();
    Reflect::set(&envelope, &"m".into(), &protocol::encode(&reply.msg).into())
        .expect("set worker envelope metadata");
    let sent = if reply.images.is_empty() {
        scope.post_message(&envelope)
    } else {
        let buffers = Array::new();
        let transfer = Array::new();
        for image in reply.images {
            let bytes = Uint8Array::from(image.as_slice());
            transfer.push(&bytes.buffer());
            buffers.push(&bytes);
        }
        Reflect::set(&envelope, &"b".into(), &buffers).expect("set worker envelope pixel buffers");
        scope.post_message_with_transfer(&envelope, &transfer)
    };
    sent.expect("post worker response");
}
