//! Serial request dispatch, with control messages allowed during browser awaits.

use crate::{
    book_path,
    protocol::{ImageMeta, OpenedMeta, ReqId, WireResult, WorkerRequest, WorkerResponse},
    wire_error::WireError,
};
use gashuu_core::{
    image_ops::downscale_to_thumbnail, page_decode::decode_with_fallback, ArchiveLoader,
    ArchivePolicy, CoreError, DecodedImage, PageDecoder, PageSource, ReadAt,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, Weak},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Interactive,
    Background,
}

pub struct Reply {
    pub msg: WorkerResponse,
    /// Straight-alpha RGBA8 buffers in response metadata order.
    pub images: Vec<Vec<u8>>,
}

impl Reply {
    pub fn metadata(msg: WorkerResponse) -> Self {
        Self {
            msg,
            images: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Sessions {
    sources: HashMap<u64, Arc<dyn PageSource>>,
    closed: HashSet<u64>,
    min_cover_gen: u64,
}

/// The message callback can apply controls while the dispatch loop is awaiting a decode.
#[derive(Clone, Default)]
pub struct Control(Rc<RefCell<Sessions>>);

impl Control {
    pub fn apply_control(&self, req: &WorkerRequest) -> bool {
        match *req {
            WorkerRequest::Close { session } => {
                let mut state = self.0.borrow_mut();
                state.closed.insert(session);
                state.sources.remove(&session);
            }
            WorkerRequest::CancelCovers { below } => {
                let mut state = self.0.borrow_mut();
                state.min_cover_gen = state.min_cover_gen.max(below);
            }
            _ => return false,
        }
        true
    }

    fn is_closed(&self, session: u64) -> bool {
        self.0.borrow().closed.contains(&session)
    }

    fn cover_cancelled(&self, gen: u64) -> bool {
        gen < self.0.borrow().min_cover_gen
    }
}

type FallbackHook = Rc<dyn Fn(&CoreError)>;

pub struct Dispatcher {
    control: Control,
    handles: HashMap<String, Weak<dyn ReadAt>>,
    primary: Rc<dyn PageDecoder>,
    fallback: Rc<dyn PageDecoder>,
    on_fallback: Option<FallbackHook>,
}

impl Dispatcher {
    pub fn new(primary: Rc<dyn PageDecoder>, fallback: Rc<dyn PageDecoder>) -> Self {
        Self {
            control: Control::default(),
            handles: HashMap::new(),
            primary,
            fallback,
            on_fallback: None,
        }
    }

    /// Report the primary error when the fallback successfully decodes a page.
    pub fn with_fallback_hook(mut self, hook: Rc<dyn Fn(&CoreError)>) -> Self {
        self.on_fallback = Some(hook);
        self
    }

    pub fn control(&self) -> Control {
        self.control.clone()
    }

    pub fn lane(req: &WorkerRequest) -> Lane {
        match req {
            WorkerRequest::Open { .. } | WorkerRequest::DecodeSpread { .. } => Lane::Interactive,
            _ => Lane::Background,
        }
    }

    pub fn needs_reader(req: &WorkerRequest) -> Option<&str> {
        match req {
            WorkerRequest::Open { id, .. }
            | WorkerRequest::Cover { id, .. }
            | WorkerRequest::ProbePageCount { id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn reader_for(&self, id: &str) -> Option<Arc<dyn ReadAt>> {
        self.handles.get(id)?.upgrade()
    }

    pub fn apply_control(&mut self, req: &WorkerRequest) -> bool {
        self.control.apply_control(req)
    }

    pub fn is_obsolete(&self, req: &WorkerRequest) -> bool {
        match *req {
            WorkerRequest::Open { session, .. }
            | WorkerRequest::DecodeSpread { session, .. }
            | WorkerRequest::Thumbnail { session, .. } => self.control.is_closed(session),
            WorkerRequest::Cover { gen, .. } => self.control.cover_cancelled(gen),
            _ => false,
        }
    }

    pub async fn handle(
        &mut self,
        req: WorkerRequest,
        reader: Option<WireResult<Arc<dyn ReadAt>>>,
    ) -> Vec<Reply> {
        if self.is_obsolete(&req) || self.apply_control(&req) {
            return Vec::new();
        }
        match req {
            WorkerRequest::Open {
                req,
                session,
                id,
                allow_rar,
            } => {
                let result = self.open(id, session, reader, ArchivePolicy { allow_rar });
                vec![Reply::metadata(WorkerResponse::Opened { req, result })]
            }
            WorkerRequest::DecodeSpread {
                req,
                session,
                leading,
                trailing,
            } => self.spread(req, session, leading, trailing).await,
            WorkerRequest::Thumbnail {
                req,
                session,
                index,
                max_side,
            } => {
                let Some(source) = self.source(session) else {
                    return unknown_session(req);
                };
                let mut images = Vec::new();
                let result = pack_image(
                    self.thumbnail(source.as_ref(), index, max_side).await,
                    &mut images,
                );
                if self.control.is_closed(session) {
                    return Vec::new();
                }
                vec![Reply {
                    msg: WorkerResponse::Thumbnail { req, result },
                    images,
                }]
            }
            WorkerRequest::Cover {
                req,
                gen,
                id,
                max_side,
                want_count,
            } => {
                let (result, count) = self.cover(&id, reader, max_side).await;
                if self.control.cover_cancelled(gen) {
                    return Vec::new();
                }
                let mut images = Vec::new();
                let result = pack_image(result, &mut images);
                let send_count = result.is_ok() && want_count;
                let mut replies = vec![Reply {
                    msg: WorkerResponse::Cover { req, result },
                    images,
                }];
                if send_count {
                    replies.push(Reply::metadata(WorkerResponse::CoverCount { req, count }));
                }
                replies
            }
            WorkerRequest::ProbePageCount { req, id, allow_rar } => {
                let result = self.archive_input(&id, reader).and_then(|(name, reader)| {
                    ArchiveLoader::probe_page_count_read_at(
                        &name,
                        reader,
                        ArchivePolicy { allow_rar },
                    )
                    .map(|count| count.get() as u32)
                    .map_err(WireError::from)
                });
                vec![Reply::metadata(WorkerResponse::PageCount { req, result })]
            }
            // Import and deletion are executed by the browser glue, which owns OPFS.
            WorkerRequest::Import { .. }
            | WorkerRequest::Sweep { .. }
            | WorkerRequest::DeleteBook { .. }
            | WorkerRequest::Close { .. }
            | WorkerRequest::CancelCovers { .. } => Vec::new(),
        }
    }

    fn archive_input(
        &self,
        id: &str,
        reader: Option<WireResult<Arc<dyn ReadAt>>>,
    ) -> WireResult<(String, Arc<dyn ReadAt>)> {
        let path = book_path::parse(id)?;
        let reader = match self.reader_for(id) {
            Some(shared) => shared,
            None => reader.ok_or_else(|| WireError::Other {
                detail: "book reader unavailable".into(),
            })??,
        };
        Ok((path.name, reader))
    }

    fn open(
        &mut self,
        id: String,
        session: u64,
        reader: Option<WireResult<Arc<dyn ReadAt>>>,
        policy: ArchivePolicy,
    ) -> WireResult<OpenedMeta> {
        let (name, reader) = self.archive_input(&id, reader)?;
        let source =
            ArchiveLoader::open_read_at(&name, reader.clone(), policy).map_err(WireError::from)?;
        let meta = OpenedMeta {
            id,
            page_count: source.list_pages().len() as u32,
            skipped: source.skipped_count() as u32,
            listing_truncated: source.listing_truncated(),
        };
        self.control.0.borrow_mut().sources.insert(session, source);
        self.handles.retain(|_, handle| handle.strong_count() != 0);
        self.handles
            .insert(meta.id.clone(), Arc::downgrade(&reader));
        Ok(meta)
    }

    fn source(&self, session: u64) -> Option<Arc<dyn PageSource>> {
        self.control.0.borrow().sources.get(&session).cloned()
    }

    async fn spread(
        &self,
        req: ReqId,
        session: u64,
        leading: u32,
        trailing: Option<u32>,
    ) -> Vec<Reply> {
        let Some(source) = self.source(session) else {
            return unknown_session(req);
        };
        let mut images = Vec::new();
        let result = self.decode_page(source.as_ref(), leading).await;
        if self.control.is_closed(session) {
            return Vec::new();
        }
        let leading = (leading, pack_image(result, &mut images));
        let trailing = if let Some(index) = trailing {
            let result = self.decode_page(source.as_ref(), index).await;
            if self.control.is_closed(session) {
                return Vec::new();
            }
            Some((index, pack_image(result, &mut images)))
        } else {
            None
        };
        vec![Reply {
            msg: WorkerResponse::Spread {
                req,
                leading,
                trailing,
            },
            images,
        }]
    }

    async fn cover(
        &self,
        id: &str,
        reader: Option<WireResult<Arc<dyn ReadAt>>>,
        max_side: u32,
    ) -> (WireResult<DecodedImage>, u32) {
        let source = self.archive_input(id, reader).and_then(|(name, reader)| {
            ArchiveLoader::open_read_at(&name, reader, ArchivePolicy::default())
                .map_err(WireError::from)
        });
        match source {
            Ok(source) => {
                let count = source.list_pages().len() as u32;
                if count == 0 {
                    return (Err(WireError::EmptyBook), 0);
                }
                (self.thumbnail(source.as_ref(), 0, max_side).await, count)
            }
            Err(error) => (Err(error), 0),
        }
    }

    async fn decode_page(&self, source: &dyn PageSource, index: u32) -> WireResult<DecodedImage> {
        let bytes = source.read_bytes(index as usize).map_err(WireError::from)?;
        let decoded = decode_with_fallback(bytes, self.primary.as_ref(), self.fallback.as_ref())
            .await
            .map_err(WireError::from)?;
        if let (Some(error), Some(hook)) = (&decoded.primary_error, &self.on_fallback) {
            hook(error);
        }
        Ok(decoded.image)
    }

    async fn thumbnail(
        &self,
        source: &dyn PageSource,
        index: u32,
        max_side: u32,
    ) -> WireResult<DecodedImage> {
        if max_side == 0 {
            return Err(WireError::Other {
                detail: "thumbnail max_side must be positive".into(),
            });
        }
        let decoded = self.decode_page(source, index).await?;
        downscale_to_thumbnail(&decoded, max_side).map_err(WireError::from)
    }
}

fn unknown_session(req: ReqId) -> Vec<Reply> {
    vec![Reply::metadata(WorkerResponse::Rejected {
        req: Some(req),
        error: WireError::Other {
            detail: "unknown session".into(),
        },
    })]
}

fn pack_image(
    result: WireResult<DecodedImage>,
    images: &mut Vec<Vec<u8>>,
) -> WireResult<ImageMeta> {
    result.map(|image| {
        let meta = ImageMeta {
            width: image.width(),
            height: image.height(),
        };
        images.push(image.rgba().to_vec());
        meta
    })
}
