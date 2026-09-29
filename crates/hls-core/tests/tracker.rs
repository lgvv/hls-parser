//! tracker 계약: 임의 순서의 sliding window 병합, 충돌, 퇴출, 최근접 검색
use hls_core::*;
use proptest::prelude::*;
use std::{fmt::Write, sync::Arc};

/// 합성 라이브 스트림의 segment 하나
#[derive(Debug, Clone)]
struct Seg {
    millis: u64,
    gap: bool,
    /// 이 segment 앞의 discontinuity, 이후 run은 `jump_millis`만큼 밀림
    discontinuity: bool,
    jump_millis: u64,
}

const BASE: i64 = 1_790_503_200_000_000_000; // 2026-09-27T10:00:00Z

/// 모든 segment의 UTC 시작, discontinuity마다 wall-clock 점프
fn starts(stream: &[Seg]) -> Vec<i64> {
    let mut out = Vec::with_capacity(stream.len());
    let mut clock = BASE;
    for seg in stream {
        if seg.discontinuity {
            clock += i64::try_from(seg.jump_millis * 1_000_000).unwrap();
        }
        out.push(clock);
        clock += i64::try_from(seg.millis * 1_000_000).unwrap();
    }
    out
}

fn rfc3339(unix_nanos: i64) -> String {
    let secs = unix_nanos.div_euclid(1_000_000_000);
    let nanos = unix_nanos.rem_euclid(1_000_000_000);
    let date = chrono::DateTime::from_timestamp(secs, u32::try_from(nanos).unwrap()).unwrap();
    date.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

/// window `[offset, offset + len)`를 드문드문한 PDT의 DVR playlist로 출력
/// PDT는 첫 segment와 모든 discontinuity 뒤에만, sequence 태그는 맞춰 조정
fn window(stream: &[Seg], offset: usize, len: usize, pdt_on_first: bool) -> Arc<MediaSnapshot> {
    let starts = starts(stream);
    let mut text = String::from("#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:10\n");
    let disc_seq = stream[..=offset].iter().filter(|s| s.discontinuity).count();
    writeln!(text, "#EXT-X-MEDIA-SEQUENCE:{offset}").unwrap();
    writeln!(text, "#EXT-X-DISCONTINUITY-SEQUENCE:{disc_seq}").unwrap();
    for i in offset..offset + len {
        let seg = &stream[i];
        let first = i == offset;
        if seg.discontinuity && !first {
            text.push_str("#EXT-X-DISCONTINUITY\n");
        }
        if (first && pdt_on_first) || (seg.discontinuity && !first) {
            writeln!(text, "#EXT-X-PROGRAM-DATE-TIME:{}", rfc3339(starts[i])).unwrap();
        }
        if seg.gap {
            text.push_str("#EXT-X-GAP\n");
        }
        writeln!(
            text,
            "#EXTINF:{}.{:03},\nseg_{i}.mp4",
            seg.millis / 1000,
            seg.millis % 1000
        )
        .unwrap();
    }
    parse(&text, &ParseOptions::default())
        .unwrap()
        .media()
        .unwrap()
        .clone()
}

fn stream(spec: &[(u64, bool, bool)]) -> Vec<Seg> {
    spec.iter()
        .map(|&(millis, gap, discontinuity)| Seg {
            millis,
            gap,
            discontinuity,
            jump_millis: 5_000,
        })
        .collect()
}

fn target(result: LocateResult) -> (u64, u64) {
    match result {
        LocateResult::Found(t) => (t.media_sequence(), t.offset_in_segment().as_nanos()),
        other => panic!("expected Found, got {other:?}"),
    }
}

#[test]
fn overlapping_refreshes_merge_into_one_history() {
    let s = stream(&[(2000, false, false); 10]);
    let mut tracker = PlaylistTracker::default();
    let first = tracker.apply(&window(&s, 0, 6, true)).unwrap();
    assert_eq!((first.added, first.evicted, first.stale), (6, 0, false));
    assert!(first.timing_error.is_none());
    let second = tracker.apply(&window(&s, 4, 6, true)).unwrap();
    assert_eq!((second.added, second.stale), (4, false));
    let again = tracker.apply(&window(&s, 4, 6, true)).unwrap();
    assert!(again.stale);
    assert_eq!(tracker.segment_count(), 10);
    assert_eq!(tracker.generation(), 2);
    let sequences: Vec<u64> = tracker
        .segments()
        .iter()
        .map(Segment::media_sequence)
        .collect();
    assert_eq!(sequences, (0..10).collect::<Vec<_>>());
    let (start, end) = tracker.window().unwrap();
    assert_eq!(start.as_unix_nanos(), BASE);
    assert_eq!(end.as_unix_nanos(), BASE + 20_000_000_000);
    // 첫 창에만 있던 seg0의 시각, 창이 밀려 seg0이 사라진 뒤에도 찾아야 함
    assert_eq!(
        target(
            tracker
                .locate(UtcTimestamp::from_unix_nanos(BASE + 1))
                .unwrap()
        ),
        (0, 1)
    );
    assert_eq!(
        target(
            tracker
                .locate(UtcTimestamp::from_unix_nanos(BASE + 19_999_999_999))
                .unwrap()
        ),
        (9, 1_999_999_999)
    );
    assert_eq!(tracker.live_edge().unwrap().media_sequence(), 9);
}

#[test]
fn a_window_starting_at_a_discontinuity_keeps_the_boundary() {
    let s = stream(&[
        (2000, false, false),
        (2000, false, false),
        (2000, false, true),
        (2000, false, false),
    ]);
    let mut tracker = PlaylistTracker::default();
    tracker.apply(&window(&s, 0, 4, true)).unwrap();
    assert!(tracker.segments()[2].discontinuity());
    // 창이 밀려 seg2가 맨 앞에 오면 DISCONTINUITY 태그를 실을 자리가 없음
    // 그래도 앞서 본 플래그가 병합 후 사라지면 안 됨
    tracker.apply(&window(&s, 2, 2, true)).unwrap();
    assert!(tracker.segments()[2].discontinuity());
    // 반대 순서, 밀린 창을 먼저 받아 플래그를 모르다가 전체 창이 오면서 알게 되는 경우
    let mut reverse = PlaylistTracker::default();
    reverse.apply(&window(&s, 2, 2, true)).unwrap();
    assert!(!reverse.segments()[0].discontinuity());
    reverse.apply(&window(&s, 0, 4, true)).unwrap();
    assert!(reverse.segments()[2].discontinuity());
    let starts = starts(&s);
    for t in [&tracker, &reverse] {
        assert_eq!(
            target(t.locate(UtcTimestamp::from_unix_nanos(starts[2])).unwrap()),
            (2, 0)
        );
        assert_eq!(
            t.locate(UtcTimestamp::from_unix_nanos(starts[2] - 1))
                .unwrap(),
            LocateResult::Gap {
                segment_index: None
            }
        );
        assert!(!t.segment_after(1).unwrap().contiguous);
        assert!(t.segment_after(2).unwrap().contiguous);
    }
}

#[test]
fn a_sequence_jump_is_a_run_boundary_and_a_gap() {
    let s = stream(&[(2000, false, false); 12]);
    let mut tracker = PlaylistTracker::default();
    tracker.apply(&window(&s, 0, 3, true)).unwrap();
    tracker.apply(&window(&s, 8, 3, true)).unwrap(); // 네트워크가 끊겼다 돌아온 상황, 3..8은 못 봄
    assert_eq!(tracker.segment_count(), 6);
    let starts = starts(&s);
    assert_eq!(
        tracker
            .locate(UtcTimestamp::from_unix_nanos(starts[5]))
            .unwrap(),
        LocateResult::Gap {
            segment_index: None
        }
    );
    let next = tracker.segment_after(2).unwrap();
    assert_eq!((next.media_sequence, next.contiguous), (8, false));
    assert_eq!(tracker.segment_after(10), None);
    let nearest = tracker
        .locate_nearest(UtcTimestamp::from_unix_nanos(starts[5]))
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            nearest.media_sequence(),
            nearest.offset_in_segment().as_nanos()
        ),
        (8, 0)
    );
    // 나중에 못 본 구간을 받으면 구멍이 메워져 하나의 연속 run이 됨
    tracker.apply(&window(&s, 2, 8, true)).unwrap();
    assert_eq!(tracker.segment_count(), 11);
    assert_eq!(
        target(
            tracker
                .locate(UtcTimestamp::from_unix_nanos(starts[5] + 7))
                .unwrap()
        ),
        (5, 7)
    );
}

#[test]
fn nearest_skips_gaps_in_playback_order() {
    let s = stream(&[
        (2000, false, false),
        (2000, true, false),
        (2000, true, false),
        (2000, false, false),
    ]);
    let mut tracker = PlaylistTracker::default();
    tracker.apply(&window(&s, 0, 4, true)).unwrap();
    let starts = starts(&s);
    let nearest = |nanos: i64| {
        tracker
            .locate_nearest(UtcTimestamp::from_unix_nanos(nanos))
            .unwrap()
            .map(|t| (t.media_sequence(), t.offset_in_segment().as_nanos()))
    };
    assert_eq!(nearest(starts[0] + 5), Some((0, 5)));
    assert_eq!(nearest(starts[1] + 5), Some((3, 0)));
    assert_eq!(nearest(BASE - 1), Some((0, 0)));
    assert_eq!(nearest(starts[3] + 2_000_000_000), Some((3, 0)));
    let only_gaps = stream(&[(2000, true, false)]);
    let mut empty = PlaylistTracker::default();
    empty.apply(&window(&only_gaps, 0, 1, true)).unwrap();
    assert_eq!(
        empty
            .locate_nearest(UtcTimestamp::from_unix_nanos(BASE))
            .unwrap(),
        None
    );
}

#[test]
fn conflicting_media_is_rejected_and_leaves_the_history_untouched() {
    let s = stream(&[(2000, false, false); 4]);
    let mut tracker = PlaylistTracker::default();
    tracker.apply(&window(&s, 0, 4, true)).unwrap();
    let mut other = s.clone();
    other[2].millis = 3000;
    let err = tracker.apply(&window(&other, 1, 3, true)).unwrap_err();
    assert_eq!(err, TrackerError::ConflictingSegment { media_sequence: 2 });
    assert_eq!(tracker.generation(), 1);
    assert_eq!(tracker.segments()[2].duration().as_nanos(), 2_000_000_000);
}

#[test]
fn pdt_tolerance_accepts_small_drift_and_strict_mode_rejects_it() {
    let s = stream(&[(2000, false, false); 6]);
    let mut drifted = s.clone();
    drifted[3].millis = 2001; // 인코더 시계가 1ms 밀린 스트림, 두 번째 창의 anchor가 누적보다 1ms 늦음
    let first = window(&s, 0, 4, true);
    let second = window(&drifted, 3, 3, true);
    let mut strict = PlaylistTracker::default();
    strict.apply(&first).unwrap();
    // sequence 3의 URI와 길이는 같고 두 번째 window의 선언 anchor만 다름
    let err = strict.apply(&second).unwrap_err();
    assert_eq!(err, TrackerError::ConflictingSegment { media_sequence: 3 });
    let mut lenient = PlaylistTracker::new(TrackerOptions {
        pdt_tolerance_nanos: 2_000_000,
        ..TrackerOptions::default()
    });
    lenient.apply(&first).unwrap();
    // 두 window의 sequence 3 선언 PDT가 서로 다르므로 여전히 미디어 충돌
    assert!(lenient.apply(&second).is_err());
    // 허용 오차 안에서 어긋나는 나중 anchor는 수용
    let mut off_by_one = s.clone();
    off_by_one[0].millis = 2001;
    let anchored_later = window(&off_by_one, 4, 2, true);
    let update = lenient.apply(&anchored_later).unwrap();
    assert!(update.timing_error.is_none());
    // 엄격 모드는 갱신 전체를 거절, 이미 시간이 정해진 이력이 조용히 바뀌는 일은 없어야 함
    let mut strict_later = PlaylistTracker::default();
    strict_later.apply(&first).unwrap();
    assert_eq!(
        strict_later.apply(&anchored_later).unwrap_err(),
        TrackerError::ConflictingSegment { media_sequence: 4 }
    );
    assert_eq!(strict_later.segment_count(), 4);
    assert_eq!(strict_later.generation(), 1);
    assert!(
        strict_later
            .locate(UtcTimestamp::from_unix_nanos(BASE))
            .is_ok()
    );
    assert_eq!(strict_later.segment_after(3), None);
    // 추적 중인 segment의 anchor 학습도 시간이 정해진 이력과 대조
    let mut learned_wrong = s.clone();
    learned_wrong[1].millis = 2500;
    let err = strict_later
        .apply(&window(&learned_wrong, 2, 2, true))
        .unwrap_err();
    assert_eq!(err, TrackerError::ConflictingSegment { media_sequence: 2 });
    assert!(strict_later.segments()[2].declared_program_time().is_none());
}

#[test]
fn eviction_trim_end_list_and_reset() {
    let s = stream(&[(2000, false, false); 8]);
    let mut tracker = PlaylistTracker::new(TrackerOptions {
        max_segments: 5,
        ..TrackerOptions::default()
    });
    tracker.apply(&window(&s, 0, 4, true)).unwrap();
    let update = tracker.apply(&window(&s, 2, 6, true)).unwrap();
    assert_eq!((update.added, update.evicted), (4, 3));
    assert_eq!(tracker.segments()[0].media_sequence(), 3);
    let starts = starts(&s);
    assert_eq!(tracker.window().unwrap().0.as_unix_nanos(), starts[3]);
    assert_eq!(
        tracker
            .trim_before(UtcTimestamp::from_unix_nanos(starts[5]))
            .unwrap(),
        2
    );
    assert_eq!(tracker.segments()[0].media_sequence(), 5);
    assert_eq!(
        tracker
            .trim_before(UtcTimestamp::from_unix_nanos(BASE))
            .unwrap(),
        0
    );
    assert!(!tracker.end_list());
    let ended = parse(
        "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:14Z\n#EXTINF:2,\nseg_7.mp4\n#EXT-X-ENDLIST\n",
        &ParseOptions::default(),
    )
    .unwrap();
    tracker.apply(ended.media().unwrap()).unwrap();
    assert!(tracker.end_list());
    assert!(tracker.metadata().unwrap().end_list());
    tracker.reset();
    assert_eq!(tracker.segment_count(), 0);
    assert!(!tracker.end_list());
    assert_eq!(
        tracker.locate(UtcTimestamp::from_unix_nanos(BASE)),
        Err(TimelineError::EmptySnapshot)
    );
}

#[test]
fn windows_without_anchors_track_but_cannot_be_timed() {
    let s = stream(&[(2000, false, false); 4]);
    let mut tracker = PlaylistTracker::default();
    let update = tracker.apply(&window(&s, 0, 4, false)).unwrap();
    assert_eq!(
        update.timing_error,
        Some(TimelineError::MissingTimeMapping { segment_index: 0 })
    );
    assert_eq!(tracker.segment_count(), 4);
    assert!(tracker.locate(UtcTimestamp::from_unix_nanos(BASE)).is_err());
    assert_eq!(tracker.segment_after(1).unwrap().media_sequence, 2);
    // anchor 없이 받은 네 segment, 뒤늦게 seg2에 anchor가 오면 seg0과 seg1의 시각도 거꾸로 계산됨
    let update = tracker.apply(&window(&s, 2, 2, true)).unwrap();
    assert!(update.timing_error.is_none());
    assert_eq!(tracker.window().unwrap().0.as_unix_nanos(), BASE);
    let blocked = parse(
        "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-SKIP:SKIPPED-SEGMENTS=1\n#EXTINF:2,\nseg.mp4\n",
        &ParseOptions::default(),
    )
    .unwrap();
    assert!(matches!(
        tracker.apply(blocked.media().unwrap()),
        Err(TrackerError::UnsupportedTimingFeature { .. })
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    #[test]
    fn any_order_of_windows_matches_a_linear_oracle(
        spec in prop::collection::vec((1000u64..9000, prop::bool::weighted(0.15), prop::bool::weighted(0.2)), 3..40),
        windows in prop::collection::vec((0usize..40, 1usize..12), 1..6),
        queries in prop::collection::vec(0i64..400_000_000_000, 32),
    ) {
        let mut spec = spec;
        spec[0].2 = false;
        let s = stream(&spec);
        let starts = starts(&s);
        let mut tracker = PlaylistTracker::default();
        let mut seen = vec![false; s.len()];
        // window는 자기 첫 segment의 discontinuity 태그를 실을 수 없음
        // 그래서 플래그는 그 segment를 다른 segment 뒤에서 본 뒤에야 알 수 있음
        let mut flag_known = vec![false; s.len()];
        for (offset, len) in windows {
            let offset = offset % s.len();
            let len = len.min(s.len() - offset);
            tracker.apply(&window(&s, offset, len, true)).unwrap();
            for flag in &mut seen[offset..offset + len] { *flag = true; }
            for flag in &mut flag_known[offset + 1..offset + len] { *flag = true; }
        }
        let expected: Vec<u64> = (0..s.len()).filter(|&i| seen[i]).map(|i| i as u64).collect();
        let actual: Vec<u64> = tracker.segments().iter().map(Segment::media_sequence).collect();
        prop_assert_eq!(&actual, &expected);
        for (i, segment) in tracker.segments().iter().enumerate() {
            let seq = usize::try_from(segment.media_sequence()).unwrap();
            prop_assert_eq!(tracker.segment_start(i).unwrap().as_unix_nanos(), starts[seq]);
            prop_assert_eq!(segment.discontinuity(), s[seq].discontinuity && flag_known[seq]);
        }
        let first_seen = usize::try_from(expected[0]).unwrap();
        let last_seen = usize::try_from(*expected.last().unwrap()).unwrap();
        let end_of = |i: usize| starts[i] + i64::try_from(s[i].millis * 1_000_000).unwrap();
        for q in queries {
            let t = BASE + q;
            let oracle = if t < starts[first_seen] {
                LocateResult::BeforeWindow
            } else if t >= end_of(last_seen) {
                LocateResult::AfterWindow
            } else {
                match (0..s.len()).find(|&i| seen[i] && starts[i] <= t && t < end_of(i)) {
                    Some(i) if s[i].gap => LocateResult::Gap { segment_index: Some(actual.iter().position(|&x| x == i as u64).unwrap()) },
                    Some(i) => {
                        let idx = actual.iter().position(|&x| x == i as u64).unwrap();
                        let got = tracker.locate(UtcTimestamp::from_unix_nanos(t)).unwrap();
                        let LocateResult::Found(target) = got else { panic!("expected Found at {t}, got {got:?}") };
                        prop_assert_eq!((target.segment_index(), target.media_sequence(), target.offset_in_segment().as_nanos()),
                                        (idx, i as u64, u64::try_from(t - starts[i]).unwrap()));
                        continue;
                    }
                    None => LocateResult::Gap { segment_index: None },
                }
            };
            prop_assert_eq!(tracker.locate(UtcTimestamp::from_unix_nanos(t)).unwrap(), oracle);
            if let Some(nearest) = tracker.locate_nearest(UtcTimestamp::from_unix_nanos(t)).unwrap() {
                prop_assert!(!tracker.segments()[nearest.segment_index()].gap());
                prop_assert_eq!(tracker.segments()[nearest.segment_index()].media_sequence(), nearest.media_sequence());
            }
        }
    }
}
