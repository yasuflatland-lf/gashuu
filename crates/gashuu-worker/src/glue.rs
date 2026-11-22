//! Bridge worker messages to the portable dispatcher.

use crate::opfs;
use gashuu_core::{BookId, ImageCratePageDecoder, ReadAt, ThumbStore};
use gashuu_web_decode::NativePageDecoder;
use gashuu_worker::{
    cover_store::{plan_cover_prune, StagedCoverStore},
    dispatcher::{Dispatcher, Lane, Reply},
    protocol::{self, ImportedMeta, WireResult, WorkerRequest, WorkerResponse},
    wire_error::WireError,
};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};
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

pub fn start() {
    console_error_panic_hook::set_once();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let store = Arc::new(StagedCoverStore::default());
    let dispatcher = Dispatcher::new(Rc::new(NativePageDecoder), Rc::new(ImageCratePageDecoder))
        .with_cover_store(store.clone())
        .with_fallback_hook(Rc::new(|error| {
            web_sys::console::warn_1(
                &format!("native page decode failed, used wasm fallback: {error}").into(),
            );
        }));
    let control = dispatcher.control();
    let dispatcher = Rc::new(RefCell::new(Some(dispatcher)));
    let inbox = Rc::new(RefCell::new(Inbox::default()));
    let callback = Closure::<dyn Fn(MessageEvent)>::new({
        let scope = scope.clone();
        let store = store.clone();
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
                    store.clone(),
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
    dispatcher: Rc<RefCell<Option<Dispatcher>>>,
    mut worker: Dispatcher,
    inbox: Rc<RefCell<Inbox>>,
    store: Arc<StagedCoverStore>,
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
                let result = import(
                    &scope,
                    &mut worker,
                    &inbox,
                    &store,
                    &name,
                    size,
                    allow_rar,
                    file,
                )
                .await;
                post(
                    &scope,
                    Reply::metadata(WorkerResponse::Imported { req, result }),
                );
            }
            WorkerRequest::DeleteBook { id } => {
                if worker.reader_for(&id).is_none() {
                    opfs::delete_book(&scope, &id).await;
                }
                // The carousel currently requests covers at 512 pixels.
                store.purge_cover(store.fingerprint(&BookId::new(&id)), &[512]);
                flush_covers(&store).await;
            }
            WorkerRequest::ClearCovers => {
                store.clear();
                flush_covers(&store).await;
            }
            WorkerRequest::PruneCovers { max_bytes } => match opfs::list_covers().await {
                Ok(files) => {
                    for name in plan_cover_prune(&files, max_bytes) {
                        warn(opfs::remove_cover(&name).await);
                    }
                }
                Err(error) => warn(Err(error)),
            },
            request => serve(&scope, &mut worker, &store, request).await,
        }
        if lane == Lane::Background && inbox.borrow().only_background_pending() {
            next_turn(&scope).await;
        }
    }
}

async fn serve(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Dispatcher,
    store: &StagedCoverStore,
    request: WorkerRequest,
) {
    if worker.is_obsolete(&request) {
        return;
    }
    let cover_name = worker.cover_file_for(&request);
    if let Some(name) = &cover_name {
        if let Some(bytes) = opfs::read_cover(name).await {
            store.preload(name.clone(), bytes);
        }
    }
    let reader = match Dispatcher::needs_reader(&request) {
        Some(id) => Some(match worker.reader_for(id) {
            Some(reader) => Ok(reader),
            None => opfs::open_book(id)
                .await
                .map(|reader| Arc::new(reader) as Arc<dyn ReadAt>),
        }),
        None => None,
    };
    // handle checks obsolescence again after the asynchronous open.
    for reply in worker.handle(request, reader).await {
        post(scope, reply);
    }
    if let Some(name) = cover_name {
        store.forget(&name);
    }
    flush_covers(store).await;
}

async fn import(
    scope: &DedicatedWorkerGlobalScope,
    worker: &mut Dispatcher,
    inbox: &RefCell<Inbox>,
    store: &StagedCoverStore,
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
    let mut upload = opfs::begin_import(scope, name, size, file).await?;
    loop {
        next_turn(scope).await;
        // Keep the current import on the stack and serve only the interactive lane
        // between its units. The next background job cannot start until it finishes.
        loop {
            let request = inbox.borrow_mut().interactive.pop_front();
            let Some(request) = request else { break };
            serve(scope, worker, store, request).await;
        }
        match upload.write_slice() {
            Ok(true) => {}
            Ok(false) => return upload.finish(allow_rar).await,
            Err(error) => {
                upload.abort().await;
                return Err(error);
            }
        }
    }
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

fn warn(result: Result<(), JsValue>) {
    if let Err(error) = result {
        web_sys::console::warn_2(&"cover cache operation failed".into(), &error);
    }
}

async fn flush_covers(store: &StagedCoverStore) {
    if store.take_clear() {
        warn(opfs::clear_covers().await);
    }
    for name in store.take_removals() {
        warn(opfs::remove_cover(&name).await);
    }
    for (name, bytes) in store.take_writes() {
        warn(opfs::write_cover(&name, &bytes).await);
    }
}
