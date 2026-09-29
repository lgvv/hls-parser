//! FFI 계약: 핸들 소유권, snapshot token, 타입이 있는 오류, 동시성

use hls_ffi::*;

fn document() -> std::sync::Arc<HlsDocument> {
    parse_playlist("#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-PROGRAM-DATE-TIME:1970-01-01T00:00:00Z\n#EXTINF:1,\na.ts\n".into()).unwrap()
}

#[test]
fn handles_own_snapshots_and_export_is_an_independent_copy() {
    let document = document();
    let mut copy = document.segment_at(0).unwrap();
    copy.uri = "changed".into();
    assert_eq!(document.segment_at(0).unwrap().uri, "a.ts");
    let timeline = document.build_relative_timeline().unwrap();
    let wall = document.build_wall_clock_timeline().unwrap();
    let token = document.snapshot_token();
    drop(document);
    let LocateResult::Found { target } = timeline.locate(1) else {
        panic!("expected Found")
    };
    assert_eq!(target.snapshot_token, token);
    assert_eq!(target.offset_nanos, 1);
    assert!(matches!(wall.locate(1), LocateResult::Found { .. }));
}

#[test]
fn target_cannot_be_used_with_another_snapshot() {
    let first = document();
    let second = document();
    let LocateResult::Found { target } = first.build_relative_timeline().unwrap().locate(1) else {
        panic!()
    };
    assert!(matches!(
        second.segment_for_target(target.clone()),
        Err(HlsError::SnapshotMismatch)
    ));
    assert_eq!(first.segment_for_target(target).unwrap().uri, "a.ts");
    assert!(matches!(
        first.segment_at(u64::MAX),
        Err(HlsError::OutOfBounds)
    ));
    assert!(matches!(
        first.segments_in_range(u64::MAX, 1),
        Err(HlsError::OutOfBounds)
    ));
    assert!(matches!(
        first.segments_in_range(0, 1025),
        Err(HlsError::OutOfBounds)
    ));
}

#[test]
fn typed_errors_and_wrong_playlist_cross_the_boundary() {
    assert!(matches!(
        parse_playlist("bad".into()),
        Err(HlsError::Parse {
            code: ParseErrorCode::InvalidHeader,
            span: Some(_),
            ..
        })
    ));
    let master = parse_playlist("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\na.m3u8\n".into()).unwrap();
    assert!(matches!(
        master.build_relative_timeline(),
        Err(HlsError::WrongPlaylist)
    ));
    assert!(matches!(
        master.export_playlist(),
        PlaylistDto::Multivariant { .. }
    ));
}

#[test]
fn immutable_handles_can_be_queried_concurrently() {
    let doc = document();
    let timeline = doc.build_relative_timeline().unwrap();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let timeline = timeline.clone();
            scope.spawn(move || {
                for n in 0..1000 {
                    assert!(matches!(timeline.locate(n), LocateResult::Found { .. }));
                }
            });
        }
    });
}

#[test]
fn tracker_merges_refreshes_and_validates_targets() {
    let window = |offset: u64, len: u64, pdt: &str| {
        let mut text = format!(
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:{offset}\n#EXT-X-PROGRAM-DATE-TIME:{pdt}\n"
        );
        for i in offset..offset + len {
            text.push_str(&format!("#EXTINF:2,\nseg_{i}.mp4\n"));
        }
        parse_playlist(text).unwrap()
    };
    let tracker = HlsPlaylistTracker::new(default_tracker_options()).unwrap();
    let first = tracker.apply(window(0, 4, "2026-09-27T10:00:00Z")).unwrap();
    assert_eq!((first.added, first.stale), (4, false));
    assert!(first.timing_error.is_none());
    let second = tracker.apply(window(3, 4, "2026-09-27T10:00:06Z")).unwrap();
    assert_eq!(second.added, 3);
    assert_eq!(tracker.segment_count(), 7);
    let base = parse_utc_timestamp("2026-09-27T10:00:00Z".into()).unwrap();
    let LocateResult::Found { target } = tracker.locate(base + 5_000_000_000).unwrap() else {
        panic!("expected found");
    };
    assert_eq!(
        (target.media_sequence, target.offset_nanos),
        (2, 1_000_000_000)
    );
    assert_eq!(
        tracker.segment_for_target(target.clone()).unwrap().uri,
        "seg_2.mp4"
    );
    let next = tracker.segment_after(target.media_sequence).unwrap();
    assert_eq!((next.segment.media_sequence, next.contiguous), (3, true));
    assert_eq!(tracker.live_edge().unwrap().media_sequence, 6);
    let window_dto = tracker.window().unwrap();
    assert_eq!(
        window_dto.end_unix_nanos - window_dto.start_unix_nanos,
        14_000_000_000
    );
    let conflict = tracker
        .apply(window(2, 2, "2026-09-27T10:00:05Z"))
        .unwrap_err();
    assert!(matches!(
        conflict,
        HlsError::Tracker {
            code: TrackerErrorCode::ConflictingSegment,
            media_sequence: Some(2),
            ..
        }
    ));
    let multivariant =
        parse_playlist("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n".into()).unwrap();
    assert!(matches!(
        tracker.apply(multivariant),
        Err(HlsError::WrongPlaylist)
    ));
    tracker.reset().unwrap();
    assert!(matches!(
        tracker.segment_for_target(target),
        Err(HlsError::SnapshotMismatch)
    ));
    assert_eq!(tracker.segment_count(), 0);
}
