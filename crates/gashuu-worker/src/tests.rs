use crate::{
    book_path::{parse, BookPath},
    dispatcher::{Dispatcher, Lane, Reply},
    protocol::{
        decode, encode, ImageMeta, OpenedMeta, ReqId, WireResult, WorkerRequest as Request,
        WorkerResponse as Response,
    },
    wire_error::WireError,
};
use gashuu_core::{
    CoreError, DecodeFuture, DecodedImage, ImageCratePageDecoder, PageDecoder, ReadAt,
};
use image::{ImageFormat, Rgba, RgbaImage};
use std::{
    cell::{Cell, RefCell},
    future::{ready, Future},
    io::{self, Cursor, Write},
    pin::pin,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Waker},
};
use zip::{write::SimpleFileOptions, ZipWriter};

const ID: &str = "books/uuid/漫画.cbz";
const REQ: ReqId = ReqId(3);
const SESSION: u64 = u64::MAX;

fn open_request(session: u64) -> Request {
    Request::Open {
        req: REQ,
        session,
        id: ID.into(),
        allow_rar: false,
    }
}

fn spread(trailing: Option<u32>) -> Request {
    Request::DecodeSpread {
        req: REQ,
        session: SESSION,
        leading: 0,
        trailing,
    }
}

fn thumbnail() -> Request {
    Request::Thumbnail {
        req: REQ,
        session: SESSION,
        index: 0,
        max_side: 2,
    }
}

fn cover(gen: u64, want_count: bool) -> Request {
    Request::Cover {
        req: REQ,
        gen,
        id: ID.into(),
        max_side: 2,
        want_count,
    }
}

fn probe() -> Request {
    Request::ProbePageCount {
        req: REQ,
        id: ID.into(),
        allow_rar: false,
    }
}

fn close(session: u64) -> Request {
    Request::Close { session }
}

fn meta(width: u32, height: u32) -> ImageMeta {
    ImageMeta { width, height }
}

fn other(detail: &str) -> WireError {
    WireError::Other {
        detail: detail.into(),
    }
}

#[test]
fn all_requests_round_trip() {
    for request in [
        open_request(SESSION),
        close(SESSION),
        spread(None),
        spread(Some(1)),
        thumbnail(),
        cover(SESSION, true),
        Request::CancelCovers { below: SESSION },
        probe(),
    ] {
        assert_eq!(decode::<Request>(&encode(&request)).unwrap(), request);
    }
    assert!(encode(&open_request(SESSION)).contains("\"session\":18446744073709551615"));
}

#[test]
fn all_responses_round_trip() {
    for response in [
        Response::Ready,
        Response::Opened {
            req: REQ,
            result: Ok(OpenedMeta {
                id: ID.into(),
                page_count: 2,
                skipped: 1,
                listing_truncated: false,
            }),
        },
        Response::Opened {
            req: REQ,
            result: Err(WireError::FormatDisabled {
                format: "rar/cbr".into(),
            }),
        },
        Response::Spread {
            req: REQ,
            leading: (0, Ok(meta(2, 3))),
            trailing: Some((1, Err(other("decode failed")))),
        },
        Response::Spread {
            req: REQ,
            leading: (0, Err(other("decode failed"))),
            trailing: None,
        },
        Response::Thumbnail {
            req: REQ,
            result: Ok(meta(2, 2)),
        },
        Response::Thumbnail {
            req: REQ,
            result: Err(other("decode failed")),
        },
        Response::Cover {
            req: REQ,
            result: Ok(meta(1, 2)),
        },
        Response::Cover {
            req: REQ,
            result: Err(WireError::EmptyBook),
        },
        Response::CoverCount { req: REQ, count: 2 },
        Response::PageCount {
            req: REQ,
            result: Ok(2),
        },
        Response::PageCount {
            req: REQ,
            result: Err(WireError::EmptyBook),
        },
        Response::Rejected {
            req: None,
            error: other("protocol: invalid JSON"),
        },
        Response::Rejected {
            req: Some(REQ),
            error: WireError::Missing { detail: ID.into() },
        },
    ] {
        assert_eq!(decode::<Response>(&encode(&response)).unwrap(), response);
    }
}

#[test]
fn malformed_json_is_a_protocol_error() {
    assert!(
        matches!(decode::<Request>("garbage"), Err(WireError::Other { detail }) if detail.starts_with("protocol:"))
    );
}

#[test]
fn maps_core_errors() {
    assert_eq!(
        WireError::from(&CoreError::EmptyBook { label: ID.into() }),
        WireError::EmptyBook
    );
    assert_eq!(
        WireError::from(&CoreError::FormatDisabled { format: "rar/cbr" }),
        WireError::FormatDisabled {
            format: "rar/cbr".into()
        }
    );
    for error in [
        CoreError::Zip(zip::result::ZipError::FileNotFound),
        CoreError::Decode(io::Error::other("bad image").into()),
        CoreError::Io(io::Error::other("bad read")),
        CoreError::IndexOutOfRange { index: 2, len: 1 },
    ] {
        assert_eq!(WireError::from(&error), other(&error.to_string()));
    }
}

#[test]
fn parses_valid_book_paths() {
    for name in ["a b.cbz", "漫画.cbz"] {
        assert_eq!(
            parse(&format!("books/uuid/{name}")),
            Ok(BookPath {
                uuid: "uuid".into(),
                name: name.into()
            })
        );
    }
}

#[test]
fn rejects_invalid_book_paths() {
    for id in [
        "books/../x",
        "books/a",
        "other/a/b.cbz",
        "books/a/b\\c.cbz",
        "books/a/.content-key",
        "books//b.cbz",
        "books/./b.cbz",
        "books/a/b.cbz/extra",
        "books/a/",
    ] {
        assert_eq!(parse(id), Err(WireError::Missing { detail: id.into() }));
    }
}

fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    RgbaImage::from_pixel(width, height, Rgba(color))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn zip(entries: &[(&str, &[u8])]) -> Arc<[u8]> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        archive
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap().into_inner().into()
}

fn book() -> Arc<dyn ReadAt> {
    Arc::new(zip(&[
        ("02.png", &png(2, 3, [0, 0, 255, 255])),
        ("01.png", &png(4, 2, [255, 0, 0, 255])),
    ]))
}

fn run<T>(future: impl Future<Output = T>) -> T {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("expected a ready test future"),
    }
}

fn dispatcher() -> Dispatcher {
    Dispatcher::new(
        Rc::new(ImageCratePageDecoder),
        Rc::new(ImageCratePageDecoder),
    )
}

fn opened(dispatcher: &mut Dispatcher, session: u64, reader: Arc<dyn ReadAt>) -> OpenedMeta {
    let replies = run(dispatcher.handle(open_request(session), Some(Ok(reader))));
    assert!(replies[0].images.is_empty());
    match &replies[..] {
        [Reply {
            msg:
                Response::Opened {
                    req: REQ,
                    result: Ok(meta),
                },
            ..
        }] => meta.clone(),
        _ => panic!("expected one successful Opened reply"),
    }
}

#[test]
fn open_reports_count_skipped_and_echoes_id() {
    let reader = Arc::new(zip(&[("01.png", &[]), ("../escape.png", &[])]));
    assert_eq!(
        opened(&mut dispatcher(), SESSION, reader),
        OpenedMeta {
            id: ID.into(),
            page_count: 1,
            skipped: 1,
            listing_truncated: false
        }
    );
}

#[test]
fn open_of_zero_image_zip_is_ok_with_zero_pages() {
    assert_eq!(
        opened(&mut dispatcher(), SESSION, Arc::new(zip(&[]))).page_count,
        0
    );
}

#[test]
fn decode_spread_returns_both_images_in_one_reply() {
    let mut worker = dispatcher();
    opened(&mut worker, SESSION, book());
    let replies = run(worker.handle(spread(Some(1)), None));
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::Spread {
            req: REQ,
            leading: (0, Ok(meta(4, 2))),
            trailing: Some((1, Ok(meta(2, 3))))
        }
    );
    assert_eq!(
        replies[0].images,
        [[255, 0, 0, 255].repeat(8), [0, 0, 255, 255].repeat(6)]
    );
}

#[test]
fn decode_spread_trailing_error_keeps_leading() {
    let mut worker = dispatcher();
    opened(&mut worker, SESSION, book());
    let replies = run(worker.handle(spread(Some(2)), None));
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::Spread {
            req: REQ,
            leading: (0, Ok(meta(4, 2))),
            trailing: Some((2, Err(other("page index 2 out of range (len 2)"))))
        }
    );
    assert_eq!(replies[0].images, [[255, 0, 0, 255].repeat(8)]);
}

struct RecordingDecoder {
    seen: RefCell<Vec<Vec<u8>>>,
    result: Result<DecodedImage, &'static str>,
}

impl RecordingDecoder {
    fn new(result: Result<DecodedImage, &'static str>) -> Rc<Self> {
        Rc::new(Self {
            seen: RefCell::default(),
            result,
        })
    }
}

impl PageDecoder for RecordingDecoder {
    fn decode(&self, bytes: Vec<u8>) -> DecodeFuture {
        self.seen.borrow_mut().push(bytes);
        Box::pin(ready(
            self.result
                .clone()
                .map_err(|err| CoreError::Io(io::Error::other(err))),
        ))
    }
}

fn fake_image() -> DecodedImage {
    DecodedImage::new([0, 255, 0, 255].repeat(8), 2, 4).unwrap()
}

#[test]
fn primary_failure_retries_the_same_bytes_with_the_fallback() {
    let primary = RecordingDecoder::new(Err("primary failed"));
    let fallback = RecordingDecoder::new(Ok(fake_image()));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let mut worker =
        Dispatcher::new(primary.clone(), fallback.clone()).with_fallback_hook(Rc::new({
            let errors = errors.clone();
            move |error| {
                let CoreError::Io(error) = error else {
                    panic!("expected the primary I/O error, got {error}");
                };
                errors.borrow_mut().push(error.to_string());
            }
        }));
    opened(&mut worker, SESSION, book());
    let replies = run(worker.handle(spread(None), None));
    assert_eq!(*primary.seen.borrow(), [png(4, 2, [255, 0, 0, 255])]);
    assert_eq!(*fallback.seen.borrow(), *primary.seen.borrow());
    assert_eq!(replies[0].images, [fake_image().rgba().to_vec()]);
    assert_eq!(*errors.borrow(), ["primary failed"]);
}

#[test]
fn primary_success_does_not_call_the_fallback_hook() {
    let mut worker = dispatcher().with_fallback_hook(Rc::new(|error| {
        panic!("unexpected fallback hook call: {error}");
    }));
    opened(&mut worker, SESSION, book());
    let replies = run(worker.handle(spread(None), None));
    assert_eq!(replies[0].images, [[255, 0, 0, 255].repeat(8)]);
}

#[test]
fn both_decoders_failing_is_a_per_slot_error() {
    let primary = RecordingDecoder::new(Err("primary failed"));
    let fallback = RecordingDecoder::new(Err("fallback failed"));
    let mut worker = Dispatcher::new(primary.clone(), fallback.clone());
    opened(&mut worker, SESSION, book());
    let replies = run(worker.handle(spread(Some(1)), None));
    let error = other("I/O error: fallback failed");
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::Spread {
            req: REQ,
            leading: (0, Err(error.clone())),
            trailing: Some((1, Err(error)))
        }
    );
    assert!(replies[0].images.is_empty());
    assert_eq!(primary.seen.borrow().len(), 2);
    assert_eq!(fallback.seen.borrow().len(), 2);
}

#[test]
fn worker_keeps_no_decoded_pages() {
    let primary = RecordingDecoder::new(Ok(fake_image()));
    let mut worker = Dispatcher::new(primary.clone(), Rc::new(ImageCratePageDecoder));
    opened(&mut worker, SESSION, book());
    for _ in 0..2 {
        assert_eq!(run(worker.handle(spread(None), None))[0].images.len(), 1);
    }
    assert_eq!(primary.seen.borrow().len(), 2);
}

#[test]
fn thumbnail_and_cover_downscale_the_primary_decode_to_max_side() {
    let primary = RecordingDecoder::new(Ok(fake_image()));
    let fallback = RecordingDecoder::new(Err("unused fallback"));
    let mut worker = Dispatcher::new(primary.clone(), fallback.clone());
    let reader = book();
    opened(&mut worker, SESSION, reader.clone());
    let thumb = run(worker.handle(thumbnail(), None));
    let cover = run(worker.handle(cover(0, false), Some(Ok(reader))));
    assert_eq!(
        thumb[0].msg,
        Response::Thumbnail {
            req: REQ,
            result: Ok(meta(1, 2))
        }
    );
    assert_eq!(
        cover[0].msg,
        Response::Cover {
            req: REQ,
            result: Ok(meta(1, 2))
        }
    );
    assert_eq!(thumb[0].images, [[0, 255, 0, 255].repeat(2)]);
    assert_eq!(thumb[0].images, cover[0].images);
    assert_eq!(primary.seen.borrow().len(), 2);
    assert!(fallback.seen.borrow().is_empty());
}

#[test]
fn lane_of_each_request_kind() {
    for request in [open_request(SESSION), spread(None)] {
        assert_eq!(Dispatcher::lane(&request), Lane::Interactive);
    }
    for request in [
        thumbnail(),
        cover(0, true),
        probe(),
        close(SESSION),
        Request::CancelCovers { below: 0 },
    ] {
        assert_eq!(Dispatcher::lane(&request), Lane::Background);
    }
}

#[test]
fn close_is_out_of_band() {
    let mut worker = dispatcher();
    opened(&mut worker, SESSION, book());
    let queued = spread(None);
    assert!(!worker.is_obsolete(&queued));
    assert!(worker.apply_control(&close(SESSION)));
    assert!(worker.is_obsolete(&queued));
    assert!(run(worker.handle(queued, None)).is_empty());
    assert!(worker.reader_for(ID).is_none());
}

#[test]
fn closed_session_discards_open_and_marks_thumbnail_obsolete() {
    let mut worker = dispatcher();
    assert!(!worker.is_obsolete(&thumbnail()));
    assert!(worker.apply_control(&close(SESSION)));
    assert!(worker.is_obsolete(&thumbnail()));
    assert!(run(worker.handle(open_request(SESSION), Some(Ok(book())))).is_empty());
}

#[test]
fn close_unknown_session_is_noop() {
    let mut worker = dispatcher();
    opened(&mut worker, SESSION, book());
    assert!(worker.apply_control(&close(2)));
    assert!(worker.apply_control(&close(2)));
    assert!(!worker.is_obsolete(&spread(None)));
    assert!(!worker.apply_control(&spread(None)));
    assert!(worker.is_obsolete(&open_request(2)));
    assert_eq!(run(worker.handle(spread(None), None))[0].images.len(), 1);
}

#[test]
fn cancel_covers_marks_older_generations_obsolete() {
    let mut worker = dispatcher();
    assert!(!worker.is_obsolete(&cover(4, false)));
    assert!(worker.apply_control(&Request::CancelCovers { below: 5 }));
    assert!(worker.apply_control(&Request::CancelCovers { below: 3 }));
    assert!(worker.is_obsolete(&cover(4, false)));
    assert!(!worker.is_obsolete(&cover(5, false)));
    assert!(run(worker.handle(cover(4, false), None)).is_empty());
}

#[test]
fn cover_without_count_sends_one_reply() {
    let replies = run(dispatcher().handle(cover(0, false), Some(Ok(book()))));
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::Cover {
            req: REQ,
            result: Ok(meta(2, 1))
        }
    );
    assert_eq!(replies[0].images.len(), 1);
}

#[test]
fn cover_with_count_sends_cover_then_count() {
    let replies = run(dispatcher().handle(cover(0, true), Some(Ok(book()))));
    assert_eq!(replies.len(), 2);
    assert_eq!(
        replies[0].msg,
        Response::Cover {
            req: REQ,
            result: Ok(meta(2, 1))
        }
    );
    assert_eq!(replies[1].msg, Response::CoverCount { req: REQ, count: 2 });
    assert!(replies[1].images.is_empty());
}

#[test]
fn cover_of_empty_book_is_empty_book_without_count() {
    let replies = run(dispatcher().handle(cover(0, true), Some(Ok(Arc::new(zip(&[]))))));
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::Cover {
            req: REQ,
            result: Err(WireError::EmptyBook)
        }
    );
    assert!(replies[0].images.is_empty());
}

#[test]
fn probe_page_count_of_empty_zip_is_empty_book() {
    let replies = run(dispatcher().handle(probe(), Some(Ok(Arc::new(zip(&[]))))));
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].msg,
        Response::PageCount {
            req: REQ,
            result: Err(WireError::EmptyBook)
        }
    );
}

fn assert_metadata_reply(replies: Vec<Reply>, expected: Response) {
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].msg, expected);
    assert!(replies[0].images.is_empty());
}

fn assert_archive_failure(id: &str, reader: Option<WireResult<Arc<dyn ReadAt>>>, error: WireError) {
    let mut worker = dispatcher();
    assert_metadata_reply(
        run(worker.handle(
            Request::Open {
                req: REQ,
                session: SESSION,
                id: id.into(),
                allow_rar: false,
            },
            reader.clone(),
        )),
        Response::Opened {
            req: REQ,
            result: Err(error.clone()),
        },
    );
    assert_unknown_session(run(worker.handle(spread(None), None)));
    assert_metadata_reply(
        run(worker.handle(
            Request::Cover {
                req: REQ,
                gen: 0,
                id: id.into(),
                max_side: 2,
                want_count: true,
            },
            reader.clone(),
        )),
        Response::Cover {
            req: REQ,
            result: Err(error.clone()),
        },
    );
    assert_metadata_reply(
        run(worker.handle(
            Request::ProbePageCount {
                req: REQ,
                id: id.into(),
                allow_rar: false,
            },
            reader,
        )),
        Response::PageCount {
            req: REQ,
            result: Err(error),
        },
    );
}

#[test]
fn archive_requests_reject_invalid_ids() {
    for id in ["../x", "books/a"] {
        assert_archive_failure(
            id,
            Some(Ok(book())),
            WireError::Missing { detail: id.into() },
        );
    }
}

#[test]
fn archive_requests_require_a_reader() {
    assert_archive_failure(ID, None, other("book reader unavailable"));
}

#[test]
fn archive_requests_preserve_a_missing_reader_error() {
    let missing = WireError::Missing { detail: ID.into() };
    assert_archive_failure(ID, Some(Err(missing.clone())), missing);
}

#[test]
fn archive_requests_report_a_corrupt_zip() {
    let bytes: Arc<[u8]> = Arc::from(&b"This byte sequence is not a ZIP archive"[..]);
    assert_archive_failure(
        ID,
        Some(Ok(Arc::new(bytes))),
        other("archive error: invalid Zip archive: Could not find EOCD"),
    );
}

struct CountingReader {
    bytes: Arc<[u8]>,
    drops: Arc<AtomicUsize>,
}

impl ReadAt for CountingReader {
    fn len(&self) -> u64 {
        ReadAt::len(&self.bytes)
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.bytes.read_at(offset, buf)
    }
}

impl Drop for CountingReader {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn reader_is_shared_and_released_when_last_session_closes() {
    let drops = Arc::new(AtomicUsize::new(0));
    let reader: Arc<dyn ReadAt> = Arc::new(CountingReader {
        bytes: zip(&[("01.png", &[])]),
        drops: drops.clone(),
    });
    let mut worker = dispatcher();
    opened(&mut worker, SESSION, reader.clone());
    let shared = worker.reader_for(ID).unwrap();
    assert!(Arc::ptr_eq(&reader, &shared));
    opened(&mut worker, 2, shared);
    drop(reader);
    assert!(worker.apply_control(&close(SESSION)));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert!(worker.reader_for(ID).is_some());
    assert!(worker.apply_control(&close(2)));
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    assert!(worker.reader_for(ID).is_none());
}

#[test]
fn unknown_session_is_rejected() {
    assert_unknown_session(run(dispatcher().handle(spread(None), None)));
}

#[test]
fn thumbnail_for_unknown_session_is_rejected() {
    assert_unknown_session(run(dispatcher().handle(thumbnail(), None)));
}

fn assert_unknown_session(replies: Vec<Reply>) {
    assert_metadata_reply(
        replies,
        Response::Rejected {
            req: Some(REQ),
            error: other("unknown session"),
        },
    );
}

struct YieldingDecoder {
    calls: Cell<usize>,
    yield_on_call: usize,
}

impl YieldingDecoder {
    fn new(yield_on_call: usize) -> Self {
        Self {
            calls: Cell::new(0),
            yield_on_call,
        }
    }
}

impl PageDecoder for YieldingDecoder {
    fn decode(&self, _bytes: Vec<u8>) -> DecodeFuture {
        let call = self.calls.get() + 1;
        self.calls.set(call);
        let mut should_yield = call == self.yield_on_call;
        Box::pin(std::future::poll_fn(move |context| {
            if std::mem::take(&mut should_yield) {
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(Ok(fake_image()))
        }))
    }
}

#[test]
fn closing_during_decode_discards_the_pending_reply() {
    let mut worker = Dispatcher::new(
        Rc::new(YieldingDecoder::new(1)),
        Rc::new(ImageCratePageDecoder),
    );
    opened(&mut worker, SESSION, book());
    let control = worker.control();
    let mut pending = pin!(worker.handle(spread(Some(1)), None));
    let mut context = Context::from_waker(Waker::noop());
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert!(control.apply_control(&close(SESSION)));
    assert!(matches!(pending.poll(&mut context), Poll::Ready(replies) if replies.is_empty()));
}

#[test]
fn closing_during_thumbnail_decode_discards_the_pending_reply() {
    let mut worker = Dispatcher::new(
        Rc::new(YieldingDecoder::new(1)),
        Rc::new(ImageCratePageDecoder),
    );
    opened(&mut worker, SESSION, book());
    let control = worker.control();
    let mut pending = pin!(worker.handle(thumbnail(), None));
    let mut context = Context::from_waker(Waker::noop());
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert!(control.apply_control(&close(SESSION)));
    assert!(matches!(pending.poll(&mut context), Poll::Ready(replies) if replies.is_empty()));
}

#[test]
fn closing_during_trailing_decode_discards_the_pending_spread() {
    let decoder = Rc::new(YieldingDecoder::new(2));
    let mut worker = Dispatcher::new(decoder.clone(), Rc::new(ImageCratePageDecoder));
    opened(&mut worker, SESSION, book());
    let control = worker.control();
    let mut pending = pin!(worker.handle(spread(Some(1)), None));
    let mut context = Context::from_waker(Waker::noop());
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert_eq!(decoder.calls.get(), 2);
    assert!(control.apply_control(&close(SESSION)));
    assert!(matches!(pending.poll(&mut context), Poll::Ready(replies) if replies.is_empty()));
}

#[test]
fn cancelling_during_cover_decode_discards_pixels_and_count() {
    let mut worker = Dispatcher::new(
        Rc::new(YieldingDecoder::new(1)),
        Rc::new(ImageCratePageDecoder),
    );
    let control = worker.control();
    let mut pending = pin!(worker.handle(cover(1, true), Some(Ok(book()))));
    let mut context = Context::from_waker(Waker::noop());
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert!(control.apply_control(&Request::CancelCovers { below: 2 }));
    assert!(matches!(pending.poll(&mut context), Poll::Ready(replies) if replies.is_empty()));
}

#[test]
fn import_names_are_safe_and_keep_the_archive_extension() {
    use crate::import::sanitise_upload_name;
    for (raw, expected) in [
        ("a/b/../c.cbz", "c.cbz"),
        (r"C:\x\y.zip", "y.zip"),
        (" . .\u{7}book.cbz ", "book.cbz"),
        ("", "book.cbz"),
        (" ... ", "book.cbz"),
    ] {
        assert_eq!(sanitise_upload_name(raw), expected);
    }
    let long = format!("{}.cbz", "本".repeat(100));
    assert_eq!(
        sanitise_upload_name(&long),
        format!("{}.cbz", "本".repeat(38))
    );
}

#[test]
fn upload_titles_follow_the_desktop_file_rule() {
    use crate::import::upload_title;
    assert_eq!(upload_title("One Piece 01.cbz"), "One Piece 01");
    for name in ["", ".", ".cbz", "book", "book.tar.zip", "本.cbz"] {
        assert_eq!(
            upload_title(name),
            gashuu_core::display_title(std::path::Path::new(name), false)
        );
    }
}

#[test]
fn quota_preflight_accounts_for_usage_and_unknown_estimates() {
    use crate::import::fits_in_quota;
    assert!(fits_in_quota(7, Some(1), Some(10)));
    assert!(fits_in_quota(9, Some(1), Some(10)));
    assert!(!fits_in_quota(10, Some(1), Some(10)));
    assert!(!fits_in_quota(1, Some(11), Some(10)));
    assert!(fits_in_quota(10, None, Some(1)));
    assert!(fits_in_quota(10, Some(1), None));
}

#[test]
fn import_protocol_round_trips_metadata_without_file_bytes() {
    use crate::protocol::ImportedMeta;
    let id = "books/uuid/本.cbz".to_string();
    for request in [
        Request::Import {
            req: ReqId(4),
            name: "本.cbz".into(),
            size: 348_000_000,
            allow_rar: false,
        },
        Request::DeleteBook { id: id.clone() },
    ] {
        assert_eq!(Dispatcher::lane(&request), Lane::Background);
        assert_eq!(decode::<Request>(&encode(&request)).unwrap(), request);
    }
    for result in [
        Ok(ImportedMeta {
            id,
            title: "本".into(),
            page_count: 42,
        }),
        Err(WireError::EmptyBook),
    ] {
        let response = Response::Imported {
            req: ReqId(4),
            result,
        };
        assert_eq!(decode::<Response>(&encode(&response)).unwrap(), response);
    }
}

#[test]
fn import_rejects_a_rar_renamed_as_cbz() {
    let rar: Arc<[u8]> = Arc::from(&b"Rar!\x1a\x07\x00"[..]);
    assert_eq!(
        crate::import::probe_upload(
            "renamed.cbz",
            Arc::new(rar),
            gashuu_core::ArchivePolicy::default()
        ),
        Err(WireError::FormatDisabled {
            format: "rar/cbr".into()
        })
    );
}
