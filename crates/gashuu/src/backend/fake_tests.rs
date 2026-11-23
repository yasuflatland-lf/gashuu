use super::*;

#[test]
fn completion_after_close_is_dropped() {
    let fake = FakeBackend::new();
    let session = fake.new_session();
    let (done, result) = super::super::test_callback();
    fake.decode_spread(session, 0, None, done);
    fake.close(session);
    fake.complete_spread(
        0,
        SpreadPages {
            leading: (0, Err(BackendError::EmptyBook)),
            trailing: None,
        },
    );
    assert!(result.try_recv().is_err());
}

#[test]
fn calls_are_recorded_in_order() {
    let fake = FakeBackend::new();
    let id = BookId::new("book.cbz");
    let session = fake.new_session();
    let image = Arc::new(DecodedImage::new(vec![0; 4], 1, 1).unwrap());
    fake.set_cached(session, 3, Arc::clone(&image));
    assert!(Arc::ptr_eq(&fake.cached_page(session, 3).unwrap(), &image));
    fake.cover(
        &id,
        ArchivePolicy::default(),
        512,
        Box::new(|_| panic!("cancelled cover delivered")),
        None,
    );
    fake.cancel_covers();
    fake.complete_cover(0, Ok(image), None);
    fake.close(session);
    assert_eq!(
        fake.calls(),
        [
            Call::CachedPage(session, 3),
            Call::Cover {
                id,
                policy: ArchivePolicy::default(),
                max_side: 512,
                wants_count: false
            },
            Call::CancelCovers,
            Call::Close(session),
        ]
    );
}

#[test]
fn trait_calls_defer_completion_and_allow_out_of_order_delivery() {
    let fake = FakeBackend::new();
    let id = BookId::new("book.cbz");
    let session = fake.new_session();
    let expect = BackendError::EmptyBook;
    let (open_done, open) = super::super::test_callback();
    let (thumb_done, thumb) = super::super::test_callback();
    let (cover_done, cover) = super::super::test_callback();
    let (count_done, count) = super::super::test_callback();
    let (first_done, first) = super::super::test_callback();
    let (second_done, second) = super::super::test_callback();

    fake.open(
        &id,
        ArchivePolicy::default(),
        CacheConfig::default(),
        open_done,
    );
    fake.thumbnail(session, 0, 128, thumb_done);
    fake.cover(
        &id,
        ArchivePolicy::default(),
        512,
        cover_done,
        Some(count_done),
    );
    fake.probe_page_count(&id, ArchivePolicy::default(), first_done);
    fake.probe_page_count(&id, ArchivePolicy::default(), second_done);
    assert!(open.try_recv().is_err());
    assert!(thumb.try_recv().is_err());
    assert!(cover.try_recv().is_err());
    assert!(count.try_recv().is_err());
    assert!(first.try_recv().is_err());
    assert!(second.try_recv().is_err());

    fake.complete_open(0, Err(expect.clone()));
    fake.complete_thumbnail(0, Err(expect.clone()));
    fake.complete_cover(0, Err(expect.clone()), Some(Ok(NonZeroUsize::MIN)));
    fake.complete_probe(1, Ok(NonZeroUsize::MIN));
    fake.complete_probe(0, Err(expect.clone()));
    assert_eq!(open.try_recv().unwrap().err(), Some(expect.clone()));
    assert_eq!(thumb.try_recv().unwrap(), Err(expect.clone()));
    assert_eq!(cover.try_recv().unwrap(), Err(expect.clone()));
    assert!(count.try_recv().is_err());
    assert_eq!(first.try_recv().unwrap(), Err(expect));
    assert_eq!(second.try_recv().unwrap(), Ok(NonZeroUsize::MIN));
}
