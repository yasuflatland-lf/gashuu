//! Bridge worker messages to the portable dispatcher.

use crate::opfs;
use gashuu_core::{ImageCratePageDecoder, ReadAt};
use gashuu_web_decode::NativePageDecoder;
use gashuu_worker::{
    dispatcher::{Dispatcher, Lane, Reply},
    protocol::{self, WorkerRequest, WorkerResponse},
    wire_error::WireError,
};
use js_sys::{Array, Object, Promise, Reflect, Uint8Array};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

#[derive(Default)]
struct Inbox {
    interactive: VecDeque<WorkerRequest>,
    background: VecDeque<WorkerRequest>,
    running: bool,
}

impl Inbox {
    fn push(&mut self, request: WorkerRequest) -> bool {
        match Dispatcher::lane(&request) {
            Lane::Interactive => self.interactive.push_back(request),
            Lane::Background => self.background.push_back(request),
        }
        // Only the callback that transitions an idle inbox to running starts a drain.
        !std::mem::replace(&mut self.running, true)
    }

    fn pop(&mut self) -> Option<WorkerRequest> {
        let request = self
            .interactive
            .pop_front()
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
    let dispatcher = Dispatcher::new(Rc::new(NativePageDecoder), Rc::new(ImageCratePageDecoder))
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
        move |event: MessageEvent| {
            let request = match read_request(&event.data()) {
                Ok(request) => request,
                Err(error) => {
                    post(
                        &scope,
                        Reply::metadata(WorkerResponse::Rejected { req: None, error }),
                    );
                    return;
                }
            };
            if !control.apply_control(&request) && inbox.borrow_mut().push(request) {
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
    dispatcher: Rc<RefCell<Option<Dispatcher>>>,
    mut worker: Dispatcher,
    inbox: Rc<RefCell<Inbox>>,
) {
    loop {
        let request = inbox.borrow_mut().pop();
        let Some(request) = request else {
            *dispatcher.borrow_mut() = Some(worker);
            return;
        };
        if worker.is_obsolete(&request) {
            continue;
        }
        let lane = Dispatcher::lane(&request);
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
            post(&scope, reply);
        }
        if lane == Lane::Background && inbox.borrow().only_background_pending() {
            // A probe with a shared reader can finish without awaiting a browser
            // task. Let new messages reach onmessage before starting another job.
            let turn = Promise::new(&mut |resolve, _| {
                scope
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
                    .expect("schedule worker inbox turn");
            });
            JsFuture::from(turn)
                .await
                .expect("worker inbox turn resolves");
        }
    }
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
