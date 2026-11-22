use super::super::test_callback;
use super::*;
use gashuu_worker::protocol::{self, ImageMeta, OpenedMeta, ReqId, WorkerRequest, WorkerResponse};

fn receive(client: &mut WebClient, response: WorkerResponse, images: Vec<Vec<u8>>) -> Effects {
    client.on_message(&protocol::encode(&response), images)
}

fn request(out: Vec<Outgoing>) -> WorkerRequest {
    let [Outgoing(text, _)] = out.try_into().ok().expect("one outgoing request");
    protocol::decode(&text).unwrap()
}

fn deliver(client: &WebClient, deliveries: Vec<Delivery>, deferred: bool) {
    let [delivery] = deliveries.try_into().ok().expect("one delivery");
    let ready = match delivery {
        Delivery::Now(ready) if !deferred => ready,
        Delivery::Deferred(ready) if deferred => ready,
        _ => panic!("unexpected delivery timing"),
    };
    if let Some(callback) = ready.into_callback(client) {
        callback();
    }
}

fn ready_client() -> WebClient {
    let mut client = WebClient::new();
    let (out, deliveries) = receive(&mut client, WorkerResponse::Ready, vec![]);
    assert!(out.is_empty() && deliveries.is_empty());
    client
}

fn book_id() -> BookId {
    BookId::new("books/test/本.cbz")
}

fn open_meta(count: u32) -> OpenedMeta {
    OpenedMeta {
        id: book_id().to_string(),
        page_count: count,
        skipped: 2,
        listing_truncated: true,
    }
}

fn opened(client: &mut WebClient, cache: CacheConfig) -> OpenedBook {
    let (done, result) = test_callback();
    let (out, deliveries) =
        client.open(&book_id(), ArchivePolicy { allow_rar: false }, cache, done);
    assert!(deliveries.is_empty());
    let WorkerRequest::Open {
        req,
        session,
        id,
        allow_rar,
    } = request(out)
    else {
        panic!("expected Open");
    };
    assert_eq!(id, book_id().as_str());
    assert!(!allow_rar);
    let (_, deliveries) = receive(
        client,
        WorkerResponse::Opened {
            req,
            result: Ok(open_meta(0)),
        },
        vec![],
    );
    assert!(result.try_recv().is_err());
    deliver(client, deliveries, false);
    let book = result
        .try_recv()
        .unwrap()
        .expect("zero-page Opened succeeds");
    assert_eq!(book.session, SessionId::new(session));
    assert_eq!(book.id, book_id());
    assert_eq!(book.page_count, 0);
    assert_eq!(book.skipped, 2);
    assert!(book.listing_truncated);
    assert!(!book.is_dir);
    book
}

fn meta() -> ImageMeta {
    ImageMeta {
        width: 1,
        height: 1,
    }
}

fn spread_request(
    out: Vec<Outgoing>,
    session: SessionId,
    leading: u32,
    trailing: Option<u32>,
) -> ReqId {
    let WorkerRequest::DecodeSpread {
        req,
        session: raw,
        leading: l,
        trailing: t,
    } = request(out)
    else {
        panic!("expected DecodeSpread");
    };
    assert_eq!(SessionId::new(raw), session);
    assert_eq!((l, t), (leading, trailing));
    req
}

fn spread_reply(req: ReqId, index: u32) -> WorkerResponse {
    WorkerResponse::Spread {
        req,
        leading: (index, Ok(meta())),
        trailing: None,
    }
}

fn fill(client: &mut WebClient, session: SessionId, index: u32, image: ImageMeta) {
    let (out, deliveries) = client.decode_spread(session, index as usize, None, Box::new(|_| {}));
    assert!(deliveries.is_empty());
    let req = spread_request(out, session, index, None);
    let bytes = vec![0; image.width as usize * image.height as usize * 4];
    let (_, deliveries) = receive(
        client,
        WorkerResponse::Spread {
            req,
            leading: (index, Ok(image)),
            trailing: None,
        },
        vec![bytes],
    );
    deliver(client, deliveries, false);
}

#[test]
fn requests_before_ready_are_buffered_and_flushed_in_order() {
    let mut client = WebClient::new();
    assert!(!client.is_ready());
    let policy = ArchivePolicy::default();
    let (open, _) = client.open(&book_id(), policy, CacheConfig::new(6, 0), Box::new(|_| {}));
    let (probe, _) = client.probe_page_count(&book_id(), policy, Box::new(|_| {}));
    let cancel = client.cancel_covers();
    assert!(open.is_empty() && probe.is_empty() && cancel.is_empty());
    let (out, deliveries) = receive(&mut client, WorkerResponse::Ready, vec![]);
    assert!(client.is_ready());
    assert!(client.fail_if_starting("deadline elapsed").is_none());
    assert!(deliveries.is_empty());
    let requests: Vec<_> = out
        .into_iter()
        .map(|Outgoing(text, _)| protocol::decode(&text).unwrap())
        .collect();
    assert!(matches!(
        requests.as_slice(),
        [
            WorkerRequest::Open { .. },
            WorkerRequest::ProbePageCount { .. },
            WorkerRequest::CancelCovers { below: 1 }
        ]
    ));
}

#[test]
fn startup_failure_fails_buffered_requests() {
    let mut client = WebClient::new();
    let policy = ArchivePolicy::default();
    let (open_done, open) = test_callback();
    let (probe_done, probe) = test_callback();
    let (out, deliveries) = client.open(&book_id(), policy, CacheConfig::new(6, 0), open_done);
    assert!(out.is_empty() && deliveries.is_empty());
    let (out, deliveries) = client.probe_page_count(&book_id(), policy, probe_done);
    assert!(out.is_empty() && deliveries.is_empty());

    let deliveries = client
        .fail_if_starting("web worker did not become ready")
        .expect("worker was still starting");
    assert_eq!(deliveries.len(), 2);
    assert!(open.try_recv().is_err() && probe.try_recv().is_err());
    for delivery in deliveries {
        deliver(&client, vec![delivery], true);
    }
    let unavailable = BackendError::Other {
        detail: "web worker unavailable".into(),
    };
    assert_eq!(open.try_recv().unwrap().err(), Some(unavailable.clone()));
    assert_eq!(probe.try_recv().unwrap().unwrap_err(), unavailable);

    let (out, deliveries) = receive(&mut client, WorkerResponse::Ready, vec![]);
    assert!(out.is_empty() && deliveries.is_empty());
    assert!(!client.is_ready());
}

#[test]
fn open_allocates_session_and_maps_opened() {
    let mut client = ready_client();
    let cache = CacheConfig::new(6, 0);
    let first = opened(&mut client, cache).session;
    assert_ne!(first, opened(&mut client, cache).session);
    let (done, result) = test_callback();
    let (out, _) = client.open(&book_id(), ArchivePolicy::default(), cache, done);
    let WorkerRequest::Open { req, .. } = request(out) else {
        panic!("expected Open")
    };
    let missing = WireError::Missing {
        detail: "missing book".into(),
    };
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Opened {
            req,
            result: Err(missing.clone()),
        },
        vec![],
    );
    deliver(&client, deliveries, false);
    assert_eq!(
        result.try_recv().unwrap().err(),
        Some(backend_error(missing))
    );
}

#[test]
fn decode_spread_miss_posts_request_and_fills_lru() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let (done, result) = test_callback();
    let (out, deliveries) = client.decode_spread(session, 0, Some(1), done);
    assert!(deliveries.is_empty());
    let req = spread_request(out, session, 0, Some(1));
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Spread {
            req,
            leading: (0, Ok(meta())),
            trailing: Some((1, Ok(meta()))),
        },
        vec![vec![1; 4], vec![2; 4]],
    );
    deliver(&client, deliveries, false);
    let spread = result.try_recv().unwrap();
    let trailing = spread.trailing.unwrap();
    for ((page, result), byte) in [(spread.leading, 1), (trailing, 2)] {
        let image = result.unwrap();
        assert_eq!(image.rgba(), &[byte; 4]);
        assert!(Arc::ptr_eq(
            &image,
            &client.cached_page(session, page).unwrap()
        ));
    }
}

#[test]
fn decode_spread_all_hit_is_deferred_and_posts_nothing() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 0, meta());
    fill(&mut client, session, 1, meta());
    let (done, result) = test_callback();
    let (out, deliveries) = client.decode_spread(session, 0, Some(1), done);
    assert!(out.is_empty() && result.try_recv().is_err());
    deliver(&client, deliveries, true);
    let spread = result.try_recv().unwrap();
    assert!(spread.leading.1.is_ok() && spread.trailing.unwrap().1.is_ok());
    let (done, result) = test_callback();
    let (_, deliveries) = client.decode_spread(session, 0, None, done);
    client.close(session);
    deliver(&client, deliveries, true);
    assert!(result.try_recv().is_err());
}

#[test]
fn decode_spread_with_only_leading_cached_posts_request() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 0, meta());

    let (out, deliveries) = client.decode_spread(session, 0, Some(1), Box::new(|_| {}));
    assert!(deliveries.is_empty());
    spread_request(out, session, 0, Some(1));
}

#[test]
fn decode_spread_with_only_trailing_cached_posts_request() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 1, meta());

    let (out, deliveries) = client.decode_spread(session, 0, Some(1), Box::new(|_| {}));
    assert!(deliveries.is_empty());
    spread_request(out, session, 0, Some(1));
}

#[test]
fn spread_trailing_error_keeps_leading() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let (done, result) = test_callback();
    let (out, _) = client.decode_spread(session, 0, Some(1), done);
    let req = spread_request(out, session, 0, Some(1));
    let error = WireError::Other {
        detail: "decode failed".into(),
    };
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Spread {
            req,
            leading: (0, Ok(meta())),
            trailing: Some((1, Err(error.clone()))),
        },
        vec![vec![1; 4]],
    );
    deliver(&client, deliveries, false);
    let spread = result.try_recv().unwrap();
    assert_eq!(spread.leading.1.unwrap().rgba(), &[1; 4]);
    assert_eq!(
        spread.trailing.unwrap().1.unwrap_err(),
        backend_error(error)
    );
}

#[test]
fn cached_page_posts_nothing_on_hit_or_miss() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 0, meta());
    assert!(client.cached_page(session, 0).is_some());
    assert!(client.cached_page(session, 1).is_none());
    let (out, _) = receive(&mut client, WorkerResponse::Ready, vec![]);
    assert!(out.is_empty());
}

#[test]
fn spread_for_a_closed_session_is_dropped_and_not_inserted() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let (done, result) = test_callback();
    let (out, _) = client.decode_spread(session, 0, None, done);
    let req = spread_request(out, session, 0, None);
    client.close(session);
    let (_, deliveries) = receive(&mut client, spread_reply(req, 0), vec![vec![0; 4]]);
    assert!(deliveries.is_empty() && result.try_recv().is_err());
    assert!(client.cached_page(session, 0).is_none());
}

#[test]
fn close_drops_pending_without_invoking_and_evicts_lru() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let other = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 0, meta());
    fill(&mut client, other, 0, meta());
    let (done, result) = test_callback();
    let (out, _) = client.decode_spread(session, 1, None, done);
    let req = spread_request(out, session, 1, None);
    assert!(
        matches!(request(client.close(session)), WorkerRequest::Close { session: raw } if SessionId::new(raw) == session)
    );
    let (_, deliveries) = receive(&mut client, spread_reply(req, 1), vec![vec![0; 4]]);
    assert!(deliveries.is_empty() && result.try_recv().is_err());
    assert!(client.cached_page(session, 0).is_none());
    assert!(client.cached_page(other, 0).is_some());
}

fn cover_request(out: Vec<Outgoing>, generation: u64) -> ReqId {
    let WorkerRequest::Cover {
        req,
        gen,
        id,
        max_side,
        want_count,
    } = request(out)
    else {
        panic!("expected Cover");
    };
    assert_eq!(gen, generation);
    assert_eq!(id, book_id().as_str());
    assert_eq!(max_side, 128);
    assert!(want_count);
    req
}

#[test]
fn cover_miss_then_count() {
    let mut client = ready_client();
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let (out, deliveries) = client.cover(&book_id(), 128, on_cover, Some(on_count));
    assert!(deliveries.is_empty());
    let req = cover_request(out, 0);
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Cover {
            req,
            result: Ok(meta()),
        },
        vec![vec![0; 4]],
    );
    deliver(&client, deliveries, false);
    assert!(cover.try_recv().unwrap().is_ok() && count.try_recv().is_err());
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::CoverCount { req, count: 3 },
        vec![],
    );
    deliver(&client, deliveries, false);
    assert_eq!(count.try_recv().unwrap().unwrap().get(), 3);
    let (_, duplicate) = receive(
        &mut client,
        WorkerResponse::CoverCount { req, count: 3 },
        vec![],
    );
    assert!(duplicate.is_empty());
}

#[test]
fn cover_empty_book_has_no_count() {
    let mut client = ready_client();
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let (out, _) = client.cover(&book_id(), 128, on_cover, Some(on_count));
    let req = cover_request(out, 0);
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Cover {
            req,
            result: Err(WireError::EmptyBook),
        },
        vec![],
    );
    deliver(&client, deliveries, false);
    assert_eq!(
        cover.try_recv().unwrap().unwrap_err(),
        BackendError::EmptyBook
    );
    assert_eq!(
        count.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    );
}

#[test]
fn cover_mem_hit_is_deferred_and_probes_count_when_asked() {
    let mut client = ready_client();
    let (out, _) = client.cover(&book_id(), 128, Box::new(|_| {}), None);
    let WorkerRequest::Cover {
        req,
        want_count: false,
        ..
    } = request(out)
    else {
        panic!("expected Cover")
    };
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Cover {
            req,
            result: Ok(meta()),
        },
        vec![vec![7; 4]],
    );
    deliver(&client, deliveries, false);
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let (out, deliveries) = client.cover(&book_id(), 128, on_cover, Some(on_count));
    let WorkerRequest::ProbePageCount { req, id, .. } = request(out) else {
        panic!("expected ProbePageCount")
    };
    assert_eq!(id, book_id().as_str());
    assert!(cover.try_recv().is_err());
    deliver(&client, deliveries, true);
    assert_eq!(cover.try_recv().unwrap().unwrap().rgba(), &[7; 4]);
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::PageCount { req, result: Ok(5) },
        vec![],
    );
    deliver(&client, deliveries, false);
    assert_eq!(count.try_recv().unwrap().unwrap().get(), 5);
    let (on_cover, cover) = test_callback();
    let (out, deliveries) = client.cover(&book_id(), 128, on_cover, None);
    assert!(out.is_empty());
    client.cancel_covers();
    deliver(&client, deliveries, true);
    assert!(cover.try_recv().is_err());
}

#[test]
fn cancel_covers_drops_older_pending_and_posts_cancel() {
    let mut client = ready_client();
    let (on_cover, cover) = test_callback();
    let (on_count, count) = test_callback();
    let (out, _) = client.cover(&book_id(), 128, on_cover, Some(on_count));
    let req = cover_request(out, 0);
    assert_eq!(
        request(client.cancel_covers()),
        WorkerRequest::CancelCovers { below: 1 }
    );
    let (out, _) = client.cover(&book_id(), 128, Box::new(|_| {}), Some(Box::new(|_| {})));
    assert_ne!(cover_request(out, 1), req);
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Cover {
            req,
            result: Ok(meta()),
        },
        vec![vec![0; 4]],
    );
    assert!(deliveries.is_empty() && cover.try_recv().is_err() && count.try_recv().is_err());
}

#[test]
fn backend_error_table() {
    let other = |detail: &str| BackendError::Other {
        detail: detail.into(),
    };
    for (error, expected) in [
        (WireError::EmptyBook, BackendError::EmptyBook),
        (
            WireError::FormatDisabled {
                format: "rar/cbr".into(),
            },
            BackendError::FormatDisabled { format: "rar/cbr" },
        ),
        (
            WireError::FormatDisabled {
                format: "zip".into(),
            },
            other("zip support is disabled"),
        ),
        (
            WireError::Missing {
                detail: "missing".into(),
            },
            BackendError::Missing {
                detail: "missing".into(),
            },
        ),
        (
            WireError::Other {
                detail: "bad".into(),
            },
            other("bad"),
        ),
    ] {
        assert_eq!(backend_error(error), expected);
    }
}

#[test]
fn rejected_with_req_resolves_that_request() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let (done, result) = test_callback();
    let (out, _) = client.thumbnail(session, 3, 128, done);
    let WorkerRequest::Thumbnail {
        req,
        index: 3,
        max_side: 128,
        ..
    } = request(out)
    else {
        panic!("expected Thumbnail")
    };
    let error = WireError::Other {
        detail: "unknown session".into(),
    };
    let (_, deliveries) = receive(
        &mut client,
        WorkerResponse::Rejected {
            req: Some(req),
            error: error.clone(),
        },
        vec![],
    );
    deliver(&client, deliveries, false);
    assert_eq!(
        result.try_recv().unwrap().unwrap_err(),
        backend_error(error)
    );
}

#[test]
fn image_length_mismatch_becomes_other_error() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    for images in [vec![vec![0; 3]], vec![]] {
        let (done, result) = test_callback();
        let (out, _) = client.decode_spread(session, 0, None, done);
        let req = spread_request(out, session, 0, None);
        let (_, deliveries) = receive(&mut client, spread_reply(req, 0), images);
        deliver(&client, deliveries, false);
        assert!(matches!(
            result.try_recv().unwrap().leading.1,
            Err(BackendError::Other { .. })
        ));
        assert!(client.cached_page(session, 0).is_none());
    }
}

#[test]
fn worker_failure_fails_pending_and_future_requests() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    fill(&mut client, session, 0, meta());
    let (done, pending) = test_callback();
    client.thumbnail(session, 0, 128, done);
    let deliveries = client.on_worker_failed("startup failed");
    assert!(pending.try_recv().is_err());
    deliver(&client, deliveries, true);
    let unavailable = BackendError::Other {
        detail: "web worker unavailable".into(),
    };
    assert_eq!(pending.try_recv().unwrap().unwrap_err(), unavailable);
    assert!(client.on_worker_failed("another error").is_empty());
    let (done, future) = test_callback();
    let (out, deliveries) = client.open(
        &book_id(),
        ArchivePolicy::default(),
        CacheConfig::new(2, 0),
        done,
    );
    assert!(out.is_empty() && future.try_recv().is_err());
    deliver(&client, deliveries, true);
    assert_eq!(future.try_recv().unwrap().err(), Some(unavailable));
    assert!(client.cached_page(session, 0).is_some());
}

#[test]
fn malformed_message_fails_pending_and_future_requests() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let (done, pending) = test_callback();
    let (out, deliveries) = client.thumbnail(session, 0, 128, done);
    assert!(matches!(request(out), WorkerRequest::Thumbnail { .. }));
    assert!(deliveries.is_empty());

    let (out, deliveries) = client.on_message("not json", vec![]);
    assert!(out.is_empty() && pending.try_recv().is_err());
    deliver(&client, deliveries, true);
    let unavailable = BackendError::Other {
        detail: "web worker unavailable".into(),
    };
    assert_eq!(pending.try_recv().unwrap().unwrap_err(), unavailable);

    let (done, future) = test_callback();
    let (out, deliveries) = client.thumbnail(session, 1, 128, done);
    assert!(out.is_empty() && future.try_recv().is_err());
    deliver(&client, deliveries, true);
    assert_eq!(future.try_recv().unwrap().unwrap_err(), unavailable);
}

#[test]
fn lru_budget_follows_latest_open_config() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    let limited = opened(&mut client, CacheConfig::new(6, 0).with_max_bytes(1)).session;
    // The minimum validated budget is 64 MiB. Two pages fit and the third forces eviction.
    let image = ImageMeta {
        width: 4096,
        height: 1536,
    };
    fill(&mut client, session, 0, image.clone());
    fill(&mut client, limited, 1, image.clone());
    assert!(client.cached_page(session, 0).is_some());
    fill(&mut client, limited, 2, image);
    assert!(client.cached_page(limited, 1).is_none());
    assert!(client.cached_page(session, 0).is_some());
    assert!(client.cached_page(limited, 2).is_some());
}

#[test]
fn lru_capacity_equals_the_passed_web_clamped_config() {
    let mut client = ready_client();
    let session = opened(&mut client, CacheConfig::new(6, 0)).session;
    for index in 0..7 {
        fill(&mut client, session, index, meta());
    }
    assert!(client.cached_page(session, 0).is_none());
    for index in 1..7 {
        assert!(client.cached_page(session, index).is_some());
    }
}

#[test]
fn imports_route_success_failure_and_discard() {
    let mut client = ready_client();
    let handle = crate::picker::UploadHandle {
        key: 7,
        name: "本.cbz".into(),
        size: 20,
    };
    for result in [
        Ok(gashuu_worker::protocol::ImportedMeta {
            duplicate: true,
            id: book_id().to_string(),
            title: "本".into(),
            page_count: 2,
        }),
        Err(WireError::EmptyBook),
    ] {
        let (done, received) = test_callback();
        let (out, deliveries) =
            client.import(&handle, Some(()), ArchivePolicy { allow_rar: false }, done);
        assert!(deliveries.is_empty());
        assert_eq!(out[0].1, Some(()));
        let WorkerRequest::Import {
            req,
            name,
            size,
            allow_rar,
        } = request(out)
        else {
            panic!("expected Import");
        };
        assert_eq!((name, size, allow_rar), (handle.name.clone(), 20, false));
        let succeeded = result.is_ok();
        let (_, deliveries) = receive(
            &mut client,
            WorkerResponse::Imported { req, result },
            vec![],
        );
        deliver(&client, deliveries, false);
        let response = received.try_recv().unwrap();
        assert!(client.after_delivery().is_empty());
        if succeeded {
            let imported = response.unwrap();
            assert_eq!(imported.id, book_id());
            assert!(imported.duplicate);
            assert_eq!(imported.title, "本");
            assert_eq!(imported.page_count.get(), 2);
        } else {
            assert_eq!(response.unwrap_err(), BackendError::EmptyBook);
        }
    }
    assert_eq!(
        request(client.discard(&book_id())),
        WorkerRequest::DeleteBook {
            id: book_id().to_string()
        }
    );
}

#[test]
fn missing_upload_is_reported_as_a_deferred_error() {
    let mut client = ready_client();
    let handle = crate::picker::UploadHandle {
        key: 7,
        name: "book.cbz".into(),
        size: 20,
    };
    let (done, result) = test_callback();
    let (out, deliveries) = client.import(&handle, None, ArchivePolicy::default(), done);
    assert!(out.is_empty());
    assert!(result.try_recv().is_err());
    deliver(&client, deliveries, true);
    assert!(matches!(
        result.try_recv().unwrap(),
        Err(BackendError::Other { .. })
    ));
}

#[test]
fn import_payload_survives_startup_and_transport_failure_resolves_the_callback() {
    let handle = crate::picker::UploadHandle {
        key: 1,
        name: "book.cbz".into(),
        size: 20,
    };
    let mut client = WebClient::new();
    let (done, result) = test_callback();
    let (out, deliveries) = client.import(&handle, Some(()), ArchivePolicy::default(), done);
    assert!(out.is_empty() && deliveries.is_empty());
    let (out, deliveries) = receive(&mut client, WorkerResponse::Ready, vec![]);
    assert!(deliveries.is_empty());
    assert_eq!(out[0].1, Some(()));
    assert!(matches!(request(out), WorkerRequest::Import { .. }));
    let deliveries = client.on_worker_failed("startup transport failed");
    deliver(&client, deliveries, true);
    assert_eq!(
        result.try_recv().unwrap().unwrap_err(),
        BackendError::Other {
            detail: crate::backend::STORAGE_UNAVAILABLE.into()
        }
    );
}

#[test]
fn sweep_delivers_storage_inventory() {
    let mut client = ready_client();
    for now in [Some(22.), None] {
        let (done, received) = test_callback();
        let (out, _) = client.sweep(vec![book_id()], now, done);
        let WorkerRequest::Sweep { req, keep, now_ms } = request(out) else {
            panic!("expected sweep")
        };
        assert_eq!(keep, [book_id().to_string()]);
        assert_eq!(now_ms, now);
        let (_, deliveries) = receive(
            &mut client,
            WorkerResponse::Swept {
                req,
                removed: 1,
                skipped_recent: 2,
                skipped_locked: 3,
                present: keep,
            },
            vec![],
        );
        deliver(&client, deliveries, false);
        let report = received.try_recv().unwrap();
        assert_eq!(
            (report.removed, report.skipped_recent, report.skipped_locked),
            (1, 2, 3)
        );
        assert_eq!(report.present, [book_id()]);
    }
}

#[test]
fn superseded_import_cleanup_runs_before_a_queued_duplicate_can_reuse_its_id() {
    let mut client = ready_client();
    let upload = crate::picker::UploadHandle {
        key: 1,
        name: "book.cbz".into(),
        size: 20,
    };
    let start = |client: &mut WebClient| {
        client
            .import(
                &upload,
                Some(()),
                ArchivePolicy::default(),
                Box::new(|_| {}),
            )
            .0
    };
    let WorkerRequest::Import { req, .. } = request(start(&mut client)) else {
        panic!("expected first import")
    };
    assert!(start(&mut client).is_empty());
    let (_, done) = receive(
        &mut client,
        WorkerResponse::Imported {
            req,
            result: Ok(gashuu_worker::protocol::ImportedMeta {
                id: book_id().to_string(),
                title: "Book".into(),
                page_count: 1,
                duplicate: false,
            }),
        },
        vec![],
    );
    deliver(&client, done, false);
    assert!(matches!(
        request(client.discard_stale(&book_id())),
        WorkerRequest::DeleteBook { .. }
    ));
    assert!(matches!(
        request(client.after_delivery()),
        WorkerRequest::Import { .. }
    ));
}
