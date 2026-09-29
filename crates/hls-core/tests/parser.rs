//! 파서 계약: 원문 보존, 태그 의미, 제한, 오류 코드

use hls_core::*;

fn media(body: &str) -> ParsedDocument {
    parse(
        &format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n{body}"),
        &ParseOptions::default(),
    )
    .unwrap()
}
fn error(body: &str) -> ParseError {
    parse(
        &format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n{body}"),
        &ParseOptions::default(),
    )
    .unwrap_err()
}

#[test]
fn source_is_exact_and_spans_are_utf8_byte_ranges() {
    let input = "#EXTM3U\r\n#EXT-X-TARGETDURATION:6\r\n#메모\r\n\r\n#EXTINF:5.000000001,제목,쉼표\r\nclip.ts";
    let doc = parse(input, &ParseOptions::default()).unwrap();
    assert_eq!(doc.source().text(), input);
    assert_eq!(doc.source().lines().len(), 6);
    let comment = &doc.source().lines()[2];
    assert_eq!(comment.kind, LineKind::Comment);
    assert_eq!(doc.source().text_at(comment.span), Some("#메모"));
    assert_eq!(comment.span.line, 3);
    assert_eq!(
        doc.source().text_at(SourceSpan {
            start: comment.span.start + 2,
            ..comment.span
        }),
        None
    );
    let segment = &doc.media().unwrap().segments()[0];
    assert_eq!(segment.title(), "제목,쉼표");
    assert_eq!(segment.duration().as_nanos(), 5_000_000_001);
    assert_eq!(
        doc.source().text_at(segment.source_span()),
        Some("#EXTINF:5.000000001,제목,쉼표\r\nclip.ts")
    );
}

#[test]
fn multivariant_preserves_quoted_commas_and_large_integers() {
    let doc = parse("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"한국어\",URI=\"audio.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=18446744073709551615,CODECS=\"avc1.4d401f,mp4a.40.2\",AUDIO=\"audio\"\n# comment\n\nvideo.m3u8\n", &ParseOptions::default()).unwrap();
    assert_eq!(doc.kind(), PlaylistKind::Multivariant);
    assert!(doc.media().is_none());
    let Playlist::Multivariant(master) = doc.playlist() else {
        panic!("wrong kind")
    };
    assert_eq!(master.variants()[0].bandwidth(), u64::MAX);
    assert_eq!(
        master.variants()[0].attributes()[1].value,
        "avc1.4d401f,mp4a.40.2"
    );
    assert_eq!(master.renditions()[0].attributes()[2].value, "한국어");
}

#[test]
fn mixed_playlist_is_rejected_in_both_orders() {
    assert_eq!(
        error("#EXT-X-STREAM-INF:BANDWIDTH=1\na.m3u8\n").code,
        ErrorCode::MixedPlaylist
    );
    let err = parse(
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\na.m3u8\n#EXTINF:1,\na.ts\n",
        &ParseOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::MixedPlaylist);
}

#[test]
fn key_formats_and_maps_persist_until_replaced() {
    let doc = media(
        "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x01\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"drm\",KEYFORMAT=\"vendor\"\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"20@0\"\n#EXTINF:1,\na.m4s\n#EXTINF:1,\nb.m4s\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:1,\nc.m4s\n",
    );
    let segments = doc.media().unwrap().segments();
    assert_eq!(segments[0].effective_keys().len(), 2);
    assert_eq!(
        segments[0].effective_keys().collect::<Vec<_>>(),
        segments[1].effective_keys().collect::<Vec<_>>()
    );
    assert_eq!(segments[2].effective_keys().len(), 0);
    assert_eq!(
        segments[2].effective_map().unwrap().effective_keys().len(),
        2
    );
    assert_eq!(segments[0].effective_map(), segments[2].effective_map());
    assert_eq!(
        segments[1].effective_map().unwrap().byte_range,
        Some(ByteRange {
            offset: 0,
            length: 20
        })
    );
    assert_eq!(
        error("#EXT-X-KEY:METHOD=NONE,URI=\"key\"\n").code,
        ErrorCode::InvalidValue
    );
    assert_eq!(
        error("#EXT-X-KEY:METHOD=AES-128,URI=\"key\"\n#EXT-X-MAP:URI=\"init\"\n").code,
        ErrorCode::InvalidValue
    );
}

#[test]
fn implicit_ranges_require_immediately_previous_same_resource() {
    let doc = media(
        "#EXT-X-BYTERANGE:100@20\n#EXTINF:1,\nall.ts\n#EXT-X-BYTERANGE:50\n#EXTINF:1,\nall.ts\n",
    );
    assert_eq!(
        doc.media().unwrap().segments()[1].byte_range(),
        Some(ByteRange {
            offset: 120,
            length: 50
        })
    );
    for body in [
        "#EXT-X-BYTERANGE:1\n#EXTINF:1,\na.ts\n",
        "#EXT-X-BYTERANGE:1@0\n#EXTINF:1,\na.ts\n#EXT-X-BYTERANGE:1\n#EXTINF:1,\nb.ts\n",
        "#EXT-X-BYTERANGE:1@0\n#EXTINF:1,\na.ts\n#EXTINF:1,\na.ts\n#EXT-X-BYTERANGE:1\n#EXTINF:1,\na.ts\n",
    ] {
        assert_eq!(error(body).code, ErrorCode::InvalidByteRange);
    }
    assert_eq!(
        error("#EXT-X-BYTERANGE:1@18446744073709551615\n#EXTINF:1,\na.ts\n").code,
        ErrorCode::Overflow
    );
}

#[test]
fn exact_duration_precision_and_checked_overflow() {
    for invalid in ["NaN", "inf", "-1", "+1", "0", ".5", "1.", "1e2", "1.0.1"] {
        assert!(
            SegmentDuration::parse_seconds(invalid).is_err(),
            "{invalid}"
        );
    }
    assert_eq!(
        SegmentDuration::parse_seconds("0.000000001")
            .unwrap()
            .as_nanos(),
        1
    );
    assert_eq!(
        SegmentDuration::parse_seconds("0.0000000001")
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedPrecision
    );
    assert_eq!(
        SegmentDuration::parse_seconds("18446744073.709551615")
            .unwrap()
            .as_nanos(),
        u64::MAX
    );
    assert_eq!(
        SegmentDuration::parse_seconds("18446744073.709551616")
            .unwrap_err()
            .code,
        ErrorCode::Overflow
    );
    let doc = parse("#EXTM3U\n#EXT-X-TARGETDURATION:18446744073709551615\n#EXTINF:18446744073.709551615,\na\n#EXTINF:1,\nb\n", &ParseOptions::default()).unwrap();
    assert!(matches!(
        RelativeTimeline::build(doc.media().unwrap().clone()),
        Err(TimelineError::Overflow { segment_index: 1 })
    ));
}

#[test]
fn timestamp_range_offsets_and_precision() {
    assert_eq!(
        UtcTimestamp::parse_rfc3339("1970-01-01T09:00:00+09:00")
            .unwrap()
            .as_unix_nanos(),
        0
    );
    assert_eq!(
        UtcTimestamp::parse_rfc3339("1969-12-31T23:59:59.999999999Z")
            .unwrap()
            .as_unix_nanos(),
        -1
    );
    for invalid in [
        "2026-01-01T00:00:00",
        "2016-12-31T23:59:60Z",
        "2500-01-01T00:00:00Z",
    ] {
        assert!(UtcTimestamp::parse_rfc3339(invalid).is_err());
    }
    assert_eq!(
        UtcTimestamp::parse_rfc3339("1970-01-01T00:00:00.1234567891Z")
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedPrecision
    );
}

#[test]
fn sequence_identity_and_discontinuity_metadata() {
    let doc = media(
        "#EXT-X-MEDIA-SEQUENCE:100\n#EXT-X-DISCONTINUITY-SEQUENCE:8\n#EXTINF:1,\na\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\nb\n#EXTINF:1,\nc\n",
    );
    let snapshot = doc.media().unwrap();
    assert_eq!(snapshot.metadata().discontinuity_sequence(), 8);
    assert_eq!(
        snapshot
            .segments()
            .iter()
            .map(Segment::media_sequence)
            .collect::<Vec<_>>(),
        [100, 101, 102]
    );
    assert_eq!(
        snapshot
            .segments()
            .iter()
            .map(Segment::discontinuity_sequence)
            .collect::<Vec<_>>(),
        [8, 9, 9]
    );
    assert_eq!(
        error("#EXT-X-MEDIA-SEQUENCE:18446744073709551615\n#EXTINF:1,\na\n#EXTINF:1,\nb\n").code,
        ErrorCode::Overflow
    );
    assert_eq!(error("#EXT-X-DISCONTINUITY-SEQUENCE:18446744073709551615\n#EXT-X-DISCONTINUITY\n#EXTINF:1,\na\n").code, ErrorCode::Overflow);
}

#[test]
fn malformed_syntax_and_pending_state_are_errors() {
    for body in [
        "a.ts\n",
        "#EXTINF:1,\n",
        "#EXTINF:1,\n#EXTINF:1,\na\n",
        "#EXT-X-ENDLIST:value\n",
        "#EXT-X-TARGETDURATION:10\n",
        "#EXT-X-GAP\n",
        "#EXTINF:1,\na.ts\n#EXT-X-MEDIA-SEQUENCE:2\n",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"key\",IV=0xz\n",
        "#EXT-X-KEY:METHOD=AES-128,URI=\"key\",URI=\"other\"\n",
        "#EXT-X-MAP:URI=\"init\"trailing\n",
        "#EXT-X-MAP:URI=\"init\",\n",
        "#EXT-X-MAP:URI=init\n",
    ] {
        assert!(
            parse(
                &format!("#EXTM3U\n#EXT-X-TARGETDURATION:10\n{body}"),
                &ParseOptions::default()
            )
            .is_err(),
            "{body}"
        );
    }
    for input in [
        "",
        "\u{feff}#EXTM3U\n",
        "\n#EXTM3U\n",
        "#EXTM3U\n#EXTM3U\n",
        "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#\0\n",
    ] {
        assert!(parse(input, &ParseOptions::default()).is_err());
    }
}

#[test]
fn target_duration_uses_nearest_integer_not_ceiling() {
    let input = |duration| format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:{duration},\na\n");
    assert!(parse(&input("6.49"), &ParseOptions::default()).is_ok());
    assert!(parse(&input("6.5"), &ParseOptions::default()).is_err());
}

#[test]
fn limits_and_diagnostic_truncation_do_not_enable_unsupported_semantics() {
    let input = "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-FUTURE:abc\n#EXT-X-PART:DURATION=1,URI=\"x\"\n#EXTINF:1,\na\n";
    let options = ParseOptions {
        max_diagnostics: 0,
        ..ParseOptions::default()
    };
    let doc = parse(input, &options).unwrap();
    assert!(doc.diagnostics().is_empty());
    assert_eq!(doc.omitted_diagnostics(), 2);
    assert_eq!(doc.source().text(), input);
    assert!(matches!(
        RelativeTimeline::build(doc.media().unwrap().clone()),
        Err(TimelineError::UnsupportedTimingFeature { .. })
    ));
    let limits = [
        ParseOptions {
            max_input_bytes: 10,
            ..ParseOptions::default()
        },
        ParseOptions {
            max_line_bytes: 7,
            ..ParseOptions::default()
        },
        ParseOptions {
            max_lines: 2,
            ..ParseOptions::default()
        },
        ParseOptions {
            max_segments: 0,
            ..ParseOptions::default()
        },
        ParseOptions {
            max_attributes: 1,
            ..ParseOptions::default()
        },
    ];
    for (i, limit) in limits.iter().enumerate() {
        let input = if i == 4 {
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc\"\nx\n"
        } else {
            input
        };
        assert_eq!(parse(input, limit).unwrap_err().code, ErrorCode::InputLimit);
    }
}

#[test]
fn resolve_relative_uris_after_redirect_without_network() {
    assert_eq!(
        resolve_uri(
            "https://cdn.test/live/index.m3u8?token=1",
            "../a.ts?token=2"
        )
        .unwrap(),
        "https://cdn.test/a.ts?token=2"
    );
    assert!(resolve_uri("not a URL", "a").is_err());
}

#[test]
fn shipped_fixtures_match_the_documented_models() {
    let live = parse(
        include_str!("../../../fixtures/live.m3u8"),
        &ParseOptions::default(),
    )
    .unwrap();
    assert_eq!(live.media().unwrap().segment_count(), 4);
    assert!(WallClockTimeline::build(live.media().unwrap().clone()).is_ok());
    let master = parse(
        include_str!("../../../fixtures/master.m3u8"),
        &ParseOptions::default(),
    )
    .unwrap();
    assert_eq!(master.kind(), PlaylistKind::Multivariant);
    let encrypted = parse(
        include_str!("../../../fixtures/encrypted.m3u8"),
        &ParseOptions::default(),
    )
    .unwrap();
    assert_eq!(
        encrypted.media().unwrap().segments()[1].byte_range(),
        Some(ByteRange {
            offset: 5120,
            length: 4096
        })
    );
}

/// 공개 스트림 75개를 훑어 보니 흔했던 RFC 8216 위반, 규격대로 거절하면 실제 방송 대부분을 못 읽음
#[test]
fn real_world_deviations_are_tolerated_with_diagnostics() {
    let doc = media(concat!(
        "#EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:00Z\n",
        "#EXT-X-DISCONTINUITY\n",
        "#EXT-X-PROGRAM-DATE-TIME:2026-09-27T10:00:05Z\n",
        "#EXT-X-BITRATE:1500\n",
        "#EXTINF:4\n",
        "seg0.ts\n",
        "#EXT-X-PROGRAM-DATE-TIME:not-a-date\n",
        "#EXT-X-CUE-OUT:30.000\n",
        "#EXTINF:4, no desc\n",
        "seg1.ts\n",
        "#EXT-X-CUE-IN\n",
        "#EXT-X-BITRATE:900\n",
        "#EXTINF:4,\n",
        "seg2.ts\n",
    ));
    let snapshot = doc.media().unwrap();
    let s = snapshot.segments();
    // 쉼표 없는 EXTINF는 빈 제목, PDT 둘 중 마지막 값 채택
    assert_eq!(s[0].title(), "");
    assert_eq!(
        s[0].declared_program_time(),
        Some(UtcTimestamp::parse_rfc3339("2026-09-27T10:00:05Z").unwrap())
    );
    // 해석 불가 PDT는 무시, 치명적 오류 아님
    assert_eq!(s[1].declared_program_time(), None);
    assert_eq!(s[1].title(), " no desc");
    // BITRATE는 교체될 때까지 유지
    assert_eq!(s[0].bitrate_kbps(), Some(1500));
    assert_eq!(s[1].bitrate_kbps(), Some(1500));
    assert_eq!(s[2].bitrate_kbps(), Some(900));
    // vendor 태그는 기록만 하고 시간 인덱스를 막지 않음(RFC 8216 §4.1)
    let tags: Vec<&str> = doc.diagnostics().iter().map(|d| d.tag.as_str()).collect();
    assert_eq!(
        tags,
        [
            "#EXT-X-PROGRAM-DATE-TIME",
            "#EXT-X-PROGRAM-DATE-TIME",
            "#EXT-X-CUE-OUT",
            "#EXT-X-CUE-IN"
        ]
    );
    assert!(doc.diagnostics()[0].message.contains("repeated"));
    assert!(doc.diagnostics()[1].message.contains("ignored"));
    assert!(snapshot.timeline_blockers().is_empty());
    assert!(WallClockTimeline::build(snapshot.clone()).is_ok());
    // attribute 쉼표 뒤 공백
    let master = parse(
        "#EXTM3U\n#EXT-X-STREAM-INF:PROGRAM-ID=1, BANDWIDTH=759977, RESOLUTION=1280x720\nv.m3u8\n",
        &ParseOptions::default(),
    )
    .unwrap();
    let Playlist::Multivariant(m) = master.playlist() else {
        panic!()
    };
    assert_eq!(m.variants()[0].bandwidth(), 759_977);
    assert_eq!(m.variants()[0].attributes()[2].value, "1280x720");
    // 알려진 시간 태그는 여전히 차단
    assert_eq!(
        media("#EXT-X-SKIP:SKIPPED-SEGMENTS=1\n#EXTINF:1,\na\n")
            .media()
            .unwrap()
            .timeline_blockers(),
        ["#EXT-X-SKIP"]
    );
    assert!(
        parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXT-X-BITRATE:0\n#EXTINF:1,\na\n",
            &ParseOptions::default()
        )
        .is_err()
    );
}
