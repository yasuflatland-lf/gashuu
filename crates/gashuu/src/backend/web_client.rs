//! Main-thread Worker protocol state, independent of JavaScript and Slint.

use super::cover_mem::CoverMemCache;
use super::{
    BackendError, CountResult, Done, ImageResult, ImportedBook, OpenedBook, SessionId, SpreadPages,
};
use crate::picker::UploadHandle;
use gashuu_core::{ArchivePolicy, BookId, CacheConfig, DecodedImage, DecodedLru};
use gashuu_worker::protocol::{
    self, ImageMeta, ReqId, WireResult, WorkerRequest as Request, WorkerResponse as Response,
};
use gashuu_worker::wire_error::WireError;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Arc;

pub(super) struct Outgoing<F = ()>(pub String, pub Option<F>);
pub(super) type Effects<F = ()> = (Vec<Outgoing<F>>, Vec<Delivery>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiveCheck {
    Session(SessionId),
    CoverGen(u64),
    Always,
}

pub(super) struct Ready(LiveCheck, Box<dyn FnOnce() + Send>);

impl Ready {
    fn new<T: Send + 'static>(target: LiveCheck, done: Done<T>, value: T) -> Self {
        Self(target, Box::new(move || done(value)))
    }

    /// Take the callback while checking liveness, then run it outside the client borrow.
    pub(super) fn into_callback<F>(
        self,
        client: &WebClient<F>,
    ) -> Option<Box<dyn FnOnce() + Send>> {
        client.is_live(self.0).then_some(self.1)
    }
}

pub(super) enum Delivery {
    Now(Ready),
    Deferred(Ready),
}

enum Pending {
    Open(
        SessionId,
        CacheConfig,
        Done<Result<OpenedBook, BackendError>>,
    ),
    Spread(SessionId, usize, Option<usize>, Done<SpreadPages>),
    Thumbnail(SessionId, Done<ImageResult>),
    Cover(u64, String, Done<ImageResult>, Option<Done<CountResult>>),
    Count(LiveCheck, Done<CountResult>),
    Sweep(Done<super::SweepReport>),
    Import(Done<Result<ImportedBook, BackendError>>),
}

impl Pending {
    fn target(&self) -> LiveCheck {
        match *self {
            Self::Open(s, ..) | Self::Spread(s, ..) | Self::Thumbnail(s, _) => {
                LiveCheck::Session(s)
            }
            Self::Cover(gen, ..) => LiveCheck::CoverGen(gen),
            Self::Count(target, _) => target,
            Self::Sweep(_) | Self::Import(_) => LiveCheck::Always,
        }
    }

    fn fail_unavailable(self) -> Ready {
        let error = match self {
            Self::Import(_) => BackendError::Other {
                detail: super::STORAGE_UNAVAILABLE.into(),
            },
            _ => unavailable(),
        };
        self.fail(error)
    }

    fn fail(self, error: BackendError) -> Ready {
        let target = self.target();
        match self {
            Self::Open(_, _, done) => Ready::new(LiveCheck::Always, done, Err(error)),
            Self::Spread(_, leading, trailing, done) => Ready::new(
                target,
                done,
                SpreadPages {
                    leading: (leading, Err(error.clone())),
                    trailing: trailing.map(|index| (index, Err(error))),
                },
            ),
            Self::Thumbnail(_, done) | Self::Cover(_, _, done, _) => {
                Ready::new(target, done, Err(error))
            }
            Self::Count(_, done) => Ready::new(target, done, Err(error)),
            Self::Sweep(_) => Ready(
                target,
                Box::new(move || tracing::warn!(%error, "opfs sweep failed")),
            ),
            Self::Import(done) => Ready::new(target, done, Err(error)),
        }
    }
}

#[derive(PartialEq, Eq)]
enum Connection {
    Starting,
    Ready,
    Failed,
}

pub(super) struct WebClient<F = ()> {
    connection: Connection,
    next_req: u32,
    // A superseded result may post DeleteBook from its completion. Deliver it
    // before a later import can re-adopt the same bytes.
    active_import: Option<ReqId>,
    waiting_storage: VecDeque<(Request, Option<F>)>,
    outbox: Vec<Outgoing<F>>,
    pending: HashMap<u32, Pending>,
    sessions: HashSet<SessionId>,
    pages: Option<(CacheConfig, DecodedLru<(SessionId, usize)>)>,
    covers: CoverMemCache,
    cover_gen: u64,
}

impl<F> WebClient<F> {
    pub(super) fn new() -> Self {
        Self {
            connection: Connection::Starting,
            next_req: 0,
            active_import: None,
            waiting_storage: VecDeque::new(),
            outbox: Vec::new(),
            pending: HashMap::new(),
            sessions: HashSet::new(),
            pages: None,
            covers: CoverMemCache::new(),
            cover_gen: 0,
        }
    }

    pub(super) fn is_ready(&self) -> bool {
        self.connection == Connection::Ready
    }

    fn post(&mut self, request: Request) -> Vec<Outgoing<F>> {
        self.post_with_file(request, None)
    }

    fn post_with_file(&mut self, request: Request, file: Option<F>) -> Vec<Outgoing<F>> {
        if self.active_import.is_some()
            && matches!(
                request,
                Request::Import { .. } | Request::Sweep { .. } | Request::DeleteBook { .. }
            )
        {
            self.waiting_storage.push_back((request, file));
            return vec![];
        }
        if let Request::Import { req, .. } = request {
            self.active_import = Some(req);
        }
        self.send(request, file)
    }

    fn send(&mut self, request: Request, file: Option<F>) -> Vec<Outgoing<F>> {
        let outgoing = || Outgoing(protocol::encode(&request), file);
        if self.is_ready() {
            return vec![outgoing()];
        }
        if self.connection == Connection::Starting {
            self.outbox.push(outgoing());
        }
        Vec::new()
    }

    fn queue(&mut self, pending: Pending, request: impl FnOnce(ReqId) -> Request) -> Effects<F> {
        self.queue_with_file(pending, request, None)
    }

    fn queue_with_file(
        &mut self,
        pending: Pending,
        request: impl FnOnce(ReqId) -> Request,
        file: Option<F>,
    ) -> Effects<F> {
        if self.connection == Connection::Failed {
            return deferred(pending.fail_unavailable());
        }
        loop {
            let id = self.next_req;
            self.next_req = id.wrapping_add(1);
            if let Entry::Vacant(entry) = self.pending.entry(id) {
                entry.insert(pending);
                return (self.post_with_file(request(ReqId(id)), file), Vec::new());
            }
        }
    }

    pub(super) fn open(
        &mut self,
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
        done: Done<Result<OpenedBook, BackendError>>,
    ) -> Effects<F> {
        let session = SessionId::allocate();
        self.queue(Pending::Open(session, cache, done), |req| Request::Open {
            req,
            session: session.0,
            id: id.to_string(),
            allow_rar: policy.allow_rar,
        })
    }

    pub(super) fn close(&mut self, session: SessionId) -> Vec<Outgoing<F>> {
        let opened = self.sessions.remove(&session);
        let pending = self.pending.len();
        self.pending
            .retain(|_, p| p.target() != LiveCheck::Session(session));
        if !opened && pending == self.pending.len() {
            return Vec::new();
        }
        if let Some((_, lru)) = &mut self.pages {
            lru.retain(|(s, _)| *s != session);
        }
        self.post(Request::Close { session: session.0 })
    }

    pub(super) fn cached_page(
        &mut self,
        session: SessionId,
        index: usize,
    ) -> Option<Arc<DecodedImage>> {
        self.pages.as_mut()?.1.get(&(session, index)).cloned()
    }

    pub(super) fn decode_spread(
        &mut self,
        session: SessionId,
        leading: usize,
        trailing: Option<usize>,
        done: Done<SpreadPages>,
    ) -> Effects<F> {
        let target = LiveCheck::Session(session);
        if !self.is_live(target) {
            return Effects::default();
        }
        if self.connection != Connection::Failed {
            let first = self.cached_page(session, leading);
            let second =
                trailing.and_then(|i| self.cached_page(session, i).map(|image| (i, Ok(image))));
            if let Some(image) = first.filter(|_| trailing.is_none() || second.is_some()) {
                let spread = SpreadPages {
                    leading: (leading, Ok(image)),
                    trailing: second,
                };
                return deferred(Ready::new(target, done, spread));
            }
        }
        self.queue(Pending::Spread(session, leading, trailing, done), |req| {
            Request::DecodeSpread {
                req,
                session: session.0,
                leading: leading as u32,
                trailing: trailing.map(|i| i as u32),
            }
        })
    }

    pub(super) fn thumbnail(
        &mut self,
        session: SessionId,
        index: usize,
        max_side: u32,
        done: Done<ImageResult>,
    ) -> Effects<F> {
        if !self.is_live(LiveCheck::Session(session)) {
            return Effects::default();
        }
        self.queue(Pending::Thumbnail(session, done), |req| {
            Request::Thumbnail {
                req,
                session: session.0,
                index: index as u32,
                max_side,
            }
        })
    }

    pub(super) fn cover(
        &mut self,
        id: &BookId,
        max_side: u32,
        on_cover: Done<ImageResult>,
        on_count: Option<Done<CountResult>>,
    ) -> Effects<F> {
        let gen = self.cover_gen;
        let key = format!("{id}#{max_side}");
        if self.connection != Connection::Failed {
            if let Some(image) = self.covers.get(&key) {
                let target = LiveCheck::CoverGen(gen);
                let (out, mut deliveries) = on_count.map_or_else(Effects::default, |done| {
                    self.probe(id, ArchivePolicy::default(), target, done)
                });
                deliveries.push(Delivery::Deferred(Ready::new(target, on_cover, Ok(image))));
                return (out, deliveries);
            }
        }
        let want_count = on_count.is_some();
        self.queue(Pending::Cover(gen, key, on_cover, on_count), |req| {
            Request::Cover {
                req,
                gen,
                id: id.to_string(),
                max_side,
                want_count,
            }
        })
    }

    pub(super) fn cancel_covers(&mut self) -> Vec<Outgoing<F>> {
        self.cover_gen += 1;
        self.pending
            .retain(|_, p| !matches!(p.target(), LiveCheck::CoverGen(_)));
        self.post(Request::CancelCovers {
            below: self.cover_gen,
        })
    }

    pub(super) fn probe_page_count(
        &mut self,
        id: &BookId,
        policy: ArchivePolicy,
        done: Done<CountResult>,
    ) -> Effects<F> {
        self.probe(id, policy, LiveCheck::Always, done)
    }

    fn probe(
        &mut self,
        id: &BookId,
        policy: ArchivePolicy,
        target: LiveCheck,
        done: Done<CountResult>,
    ) -> Effects<F> {
        self.queue(Pending::Count(target, done), |req| {
            Request::ProbePageCount {
                req,
                id: id.to_string(),
                allow_rar: policy.allow_rar,
            }
        })
    }

    pub(super) fn import(
        &mut self,
        handle: &UploadHandle,
        file: Option<F>,
        policy: ArchivePolicy,
        done: Done<Result<ImportedBook, BackendError>>,
    ) -> Effects<F> {
        let Some(file) = file else {
            return deferred(Pending::Import(done).fail(BackendError::Other {
                detail: "selected file is no longer available".into(),
            }));
        };
        self.queue_with_file(
            Pending::Import(done),
            |req| Request::Import {
                req,
                name: handle.name.clone(),
                size: handle.size,
                allow_rar: policy.allow_rar,
            },
            Some(file),
        )
    }

    pub(super) fn sweep(
        &mut self,
        keep: Vec<BookId>,
        now_ms: Option<f64>,
        done: Done<super::SweepReport>,
    ) -> Effects<F> {
        let keep = keep.into_iter().map(|id| id.to_string()).collect();
        self.queue(Pending::Sweep(done), |req| Request::Sweep {
            req,
            keep,
            now_ms,
        })
    }

    pub(super) fn discard(&mut self, id: &BookId) -> Vec<Outgoing<F>> {
        self.post(Request::DeleteBook { id: id.to_string() })
    }

    pub(super) fn discard_stale(&mut self, id: &BookId) -> Vec<Outgoing<F>> {
        self.send(Request::DeleteBook { id: id.to_string() }, None)
    }

    pub(super) fn after_delivery(&mut self) -> Vec<Outgoing<F>> {
        let finished =
            matches!(self.active_import, Some(req) if !self.pending.contains_key(&req.0));
        if !finished {
            return vec![];
        }
        self.active_import = None;
        let mut outgoing = vec![];
        while self.active_import.is_none() {
            let Some((request, file)) = self.waiting_storage.pop_front() else {
                break;
            };
            outgoing.extend(self.post_with_file(request, file));
        }
        outgoing
    }

    pub(super) fn on_message(&mut self, text: &str, images: Vec<Vec<u8>>) -> Effects<F> {
        if self.connection == Connection::Failed {
            return Effects::default();
        }
        let response = match protocol::decode(text) {
            Ok(response) => response,
            Err(error) => {
                return (
                    Vec::new(),
                    self.on_worker_failed(&backend_error(error).to_string()),
                )
            }
        };
        let req = match response {
            Response::Ready => {
                self.connection = Connection::Ready;
                return (std::mem::take(&mut self.outbox), Vec::new());
            }
            Response::Rejected { req: None, error } => {
                tracing::warn!(?error, "worker rejected an uncorrelated request");
                return Effects::default();
            }
            Response::Opened { req, .. }
            | Response::Spread { req, .. }
            | Response::Thumbnail { req, .. }
            | Response::Cover { req, .. }
            | Response::CoverCount { req, .. }
            | Response::PageCount { req, .. }
            | Response::Swept { req, .. }
            | Response::Imported { req, .. }
            | Response::Rejected { req: Some(req), .. } => req,
        };
        let Some(pending) = self.pending.remove(&req.0) else {
            return Effects::default();
        };
        let target = pending.target();
        let mut images = images.into_iter();
        let mut image = |result| unpack(result, &mut images);
        let ready = match (pending, response) {
            (
                Pending::Open(session, config, done),
                Response::Opened {
                    result: Ok(meta), ..
                },
            ) => {
                self.sessions.insert(session);
                if self
                    .pages
                    .as_ref()
                    .is_none_or(|(current, _)| *current != config)
                {
                    let capacity =
                        NonZeroUsize::new(config.capacity()).expect("validated capacity");
                    self.pages = Some((config, DecodedLru::new(capacity, config.max_bytes())));
                }
                let opened = OpenedBook {
                    session,
                    id: BookId::new(meta.id),
                    page_count: meta.page_count as usize,
                    skipped: meta.skipped as usize,
                    listing_truncated: meta.listing_truncated,
                    is_dir: false,
                };
                Ready::new(target, done, Ok(opened))
            }
            (
                Pending::Spread(session, .., done),
                Response::Spread {
                    leading, trailing, ..
                },
            ) => {
                let mut slot = |(index, result)| {
                    let index = index as usize;
                    let decoded = image(result);
                    if let (Ok(page), Some((_, lru))) = (&decoded, &mut self.pages) {
                        lru.put((session, index), Arc::clone(page));
                    }
                    (index, decoded)
                };
                let spread = SpreadPages {
                    leading: slot(leading),
                    trailing: trailing.map(slot),
                };
                Ready::new(target, done, spread)
            }
            (Pending::Thumbnail(_, done), Response::Thumbnail { result, .. }) => {
                Ready::new(target, done, image(result))
            }
            (Pending::Cover(_, key, done, count), Response::Cover { result, .. }) => {
                let cover = image(result);
                if let Ok(page) = &cover {
                    self.covers.insert(key, Arc::clone(page));
                    if let Some(count) = count {
                        self.pending.insert(req.0, Pending::Count(target, count));
                    }
                }
                Ready::new(target, done, cover)
            }
            (
                Pending::Count(_, done),
                Response::CoverCount { count, .. }
                | Response::PageCount {
                    result: Ok(count), ..
                },
            ) => Ready::new(
                target,
                done,
                NonZeroUsize::new(count as usize).ok_or(BackendError::EmptyBook),
            ),
            (
                Pending::Sweep(done),
                Response::Swept {
                    removed,
                    skipped_recent,
                    skipped_locked,
                    present,
                    ..
                },
            ) => {
                let report = super::SweepReport {
                    removed,
                    skipped_recent,
                    skipped_locked,
                    present: present.into_iter().map(BookId::new).collect(),
                };
                tracing::info!(
                    removed = report.removed,
                    skipped_recent = report.skipped_recent,
                    skipped_locked = report.skipped_locked,
                    "opfs sweep"
                );
                Ready::new(target, done, report)
            }
            (Pending::Import(done), Response::Imported { result, .. }) => {
                let result = result.map_err(backend_error).and_then(|meta| {
                    let page_count = NonZeroUsize::new(meta.page_count as usize)
                        .ok_or(BackendError::EmptyBook)?;
                    Ok(ImportedBook {
                        duplicate: meta.duplicate,
                        id: BookId::new(meta.id),
                        title: meta.title,
                        page_count,
                    })
                });
                Ready::new(target, done, result)
            }
            (
                pending,
                Response::Rejected { error, .. }
                | Response::Opened {
                    result: Err(error), ..
                }
                | Response::PageCount {
                    result: Err(error), ..
                },
            ) => pending.fail(backend_error(error)),
            (pending, _) => pending.fail(BackendError::Other {
                detail: "unexpected web worker response".into(),
            }),
        };
        (Vec::new(), vec![Delivery::Now(ready)])
    }

    pub(super) fn fail_if_starting(&mut self, detail: &str) -> Option<Vec<Delivery>> {
        (self.connection == Connection::Starting).then(|| self.on_worker_failed(detail))
    }

    pub(super) fn on_worker_failed(&mut self, detail: &str) -> Vec<Delivery> {
        if self.connection == Connection::Failed {
            return Vec::new();
        }
        tracing::error!(%detail, "web worker unavailable");
        self.connection = Connection::Failed;
        self.outbox.clear();
        self.waiting_storage.clear();
        self.active_import = None;
        self.pending
            .drain()
            .map(|(_, p)| Delivery::Deferred(p.fail_unavailable()))
            .collect()
    }

    fn is_live(&self, target: LiveCheck) -> bool {
        match target {
            LiveCheck::Session(session) => self.sessions.contains(&session),
            LiveCheck::CoverGen(gen) => self.cover_gen == gen,
            LiveCheck::Always => true,
        }
    }
}

fn deferred<F>(ready: Ready) -> Effects<F> {
    (Vec::new(), vec![Delivery::Deferred(ready)])
}

fn unavailable() -> BackendError {
    BackendError::Other {
        detail: "web worker unavailable".into(),
    }
}

fn unpack(
    result: WireResult<ImageMeta>,
    images: &mut impl Iterator<Item = Vec<u8>>,
) -> ImageResult {
    let ImageMeta { width, height } = result.map_err(backend_error)?;
    let bytes = images.next().ok_or_else(|| BackendError::Other {
        detail: "missing worker image buffer".into(),
    })?;
    DecodedImage::new(bytes, width, height)
        .map(Arc::new)
        .map_err(|e| BackendError::from_core(&e))
}

fn backend_error(error: WireError) -> BackendError {
    match error {
        WireError::EmptyBook => BackendError::EmptyBook,
        WireError::FormatDisabled { format } if format == "rar/cbr" => {
            BackendError::FormatDisabled { format: "rar/cbr" }
        }
        WireError::FormatDisabled { format } => BackendError::Other {
            detail: format!("{format} support is disabled"),
        },
        WireError::Missing { detail } => BackendError::Missing { detail },
        WireError::Other { detail } => BackendError::Other { detail },
    }
}

#[cfg(test)]
#[path = "web_client_tests.rs"]
mod tests;
