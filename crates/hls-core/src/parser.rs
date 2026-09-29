//! 줄 단위 파서
//! HLS는 태그 몇 줄 뒤에 URI 한 줄이 오는 구조라, 태그를 [`State`]에 쌓아 두다가
//! URI 줄이 오면 그때까지 쌓인 것을 segment 또는 variant 하나로 확정

use crate::{
    attributes::{attr, parse_attributes, required},
    diagnostic::{Diagnostic, ErrorCode, ParseError},
    domain::{
        Attribute, ByteRange, EncryptionKey, EncryptionMethod, InitializationMap, MediaMetadata,
        MediaSnapshot, MultivariantPlaylist, ParsedDocument, Playlist, PlaylistKind, PlaylistType,
        Rendition, Segment, Variant,
    },
    source::{LineKind, RawLine, SourceDocument, SourceSpan},
    time::{NANOS_PER_SECOND, SegmentDuration, UtcTimestamp},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// 입력 크기 제한, 신뢰할 수 없는 서버 응답으로 메모리를 다 쓰지 않게 하기 위한 것
/// 원문을 복사하거나 모델 컬렉션을 늘리기 전에 검사
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// playlist 최대 크기(UTF-8 바이트)
    pub max_input_bytes: usize,
    /// 한 줄 최대 길이(바이트), 줄 종결자 제외
    pub max_line_bytes: usize,
    /// 최대 줄 수, 빈 줄과 주석 줄 포함
    pub max_lines: usize,
    /// segment, variant, rendition 각각의 최대 개수
    pub max_segments: usize,
    /// 태그당 최대 attribute 수, 서로 다른 `KEYFORMAT` 값의 최대 수와 동일
    pub max_attributes: usize,
    /// 보관할 diagnostic 수, 나머지는 `omitted_diagnostics`로 셈
    pub max_diagnostics: usize,
}
impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_input_bytes: 8 * 1024 * 1024,
            max_line_bytes: 64 * 1024,
            max_lines: 300_000,
            max_segments: 100_000,
            max_attributes: 64,
            max_diagnostics: 100,
        }
    }
}

/// 지원하는 HLS 의미 파싱, 미지원 태그는 원문에 남기고 diagnostic 생성
/// 완전한 RFC conformance validator는 아님, 실패 시 부분 문서 없음
pub fn parse(input: &str, options: &ParseOptions) -> Result<ParsedDocument, ParseError> {
    if input.len() > options.max_input_bytes {
        return Err(error(
            ErrorCode::InputLimit,
            None,
            "playlist exceeds input byte limit",
        ));
    }
    let mut state = State::new(options);
    let mut lines = Vec::new();
    let mut offset = 0;
    for (index, raw) in input.split_inclusive('\n').enumerate() {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let span = SourceSpan {
            start: offset,
            end: offset + line.len(),
            line: index + 1,
        };
        if index >= options.max_lines || line.len() > options.max_line_bytes {
            return Err(error(
                ErrorCode::InputLimit,
                Some(span),
                "line count or length exceeds limit",
            ));
        }
        // Apple segmenter가 EXTINF 제목에 탭을 쓰므로 탭은 일반 문자
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(syntax(span, "control character in playlist"));
        }
        let kind = classify_line(index, line, span)?;
        match kind {
            LineKind::Tag => state.tag(line, span)?,
            LineKind::Uri => state.uri(line, span)?,
            LineKind::Header | LineKind::Comment | LineKind::Blank => {}
        }
        lines.push(RawLine { kind, span });
        offset += raw.len();
    }
    if lines.is_empty() {
        return Err(error(ErrorCode::InvalidHeader, None, "empty playlist"));
    }
    state.finish(input, lines)
}

fn classify_line(index: usize, line: &str, span: SourceSpan) -> Result<LineKind, ParseError> {
    if index == 0 {
        return if line == "#EXTM3U" {
            Ok(LineKind::Header)
        } else {
            Err(error(
                ErrorCode::InvalidHeader,
                Some(span),
                "first line must be #EXTM3U (no BOM)",
            ))
        };
    }
    Ok(if line.is_empty() {
        LineKind::Blank
    } else if line.starts_with("#EXT") {
        LineKind::Tag
    } else if line.starts_with('#') {
        LineKind::Comment
    } else {
        LineKind::Uri
    })
}

/// 파서가 아는 태그, 그 외 `#EXT` 줄은 diagnostic으로 보존
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Tag {
    Header,
    Version,
    IndependentSegments,
    Start,
    StreamInf,
    Media,
    IFrameStreamInf,
    SessionData,
    SessionKey,
    TargetDuration,
    MediaSequence,
    DiscontinuitySequence,
    PlaylistType,
    EndList,
    Inf,
    ProgramDateTime,
    ByteRange,
    Discontinuity,
    Gap,
    Key,
    Map,
    DateRange,
    AllowCache,
    Bitrate,
    IFramesOnly,
    Skip,
    Part,
    PartInf,
    ServerControl,
    PreloadHint,
    RenditionReport,
}

impl Tag {
    const ALL: [Self; 31] = [
        Self::Header,
        Self::Version,
        Self::IndependentSegments,
        Self::Start,
        Self::StreamInf,
        Self::Media,
        Self::IFrameStreamInf,
        Self::SessionData,
        Self::SessionKey,
        Self::TargetDuration,
        Self::MediaSequence,
        Self::DiscontinuitySequence,
        Self::PlaylistType,
        Self::EndList,
        Self::Inf,
        Self::ProgramDateTime,
        Self::ByteRange,
        Self::Discontinuity,
        Self::Gap,
        Self::Key,
        Self::Map,
        Self::DateRange,
        Self::AllowCache,
        Self::Bitrate,
        Self::IFramesOnly,
        Self::Skip,
        Self::Part,
        Self::PartInf,
        Self::ServerControl,
        Self::PreloadHint,
        Self::RenditionReport,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Header => "#EXTM3U",
            Self::Version => "#EXT-X-VERSION",
            Self::IndependentSegments => "#EXT-X-INDEPENDENT-SEGMENTS",
            Self::Start => "#EXT-X-START",
            Self::StreamInf => "#EXT-X-STREAM-INF",
            Self::Media => "#EXT-X-MEDIA",
            Self::IFrameStreamInf => "#EXT-X-I-FRAME-STREAM-INF",
            Self::SessionData => "#EXT-X-SESSION-DATA",
            Self::SessionKey => "#EXT-X-SESSION-KEY",
            Self::TargetDuration => "#EXT-X-TARGETDURATION",
            Self::MediaSequence => "#EXT-X-MEDIA-SEQUENCE",
            Self::DiscontinuitySequence => "#EXT-X-DISCONTINUITY-SEQUENCE",
            Self::PlaylistType => "#EXT-X-PLAYLIST-TYPE",
            Self::EndList => "#EXT-X-ENDLIST",
            Self::Inf => "#EXTINF",
            Self::ProgramDateTime => "#EXT-X-PROGRAM-DATE-TIME",
            Self::ByteRange => "#EXT-X-BYTERANGE",
            Self::Discontinuity => "#EXT-X-DISCONTINUITY",
            Self::Gap => "#EXT-X-GAP",
            Self::Key => "#EXT-X-KEY",
            Self::Map => "#EXT-X-MAP",
            Self::DateRange => "#EXT-X-DATERANGE",
            Self::AllowCache => "#EXT-X-ALLOW-CACHE",
            Self::Bitrate => "#EXT-X-BITRATE",
            Self::IFramesOnly => "#EXT-X-I-FRAMES-ONLY",
            Self::Skip => "#EXT-X-SKIP",
            Self::Part => "#EXT-X-PART",
            Self::PartInf => "#EXT-X-PART-INF",
            Self::ServerControl => "#EXT-X-SERVER-CONTROL",
            Self::PreloadHint => "#EXT-X-PRELOAD-HINT",
            Self::RenditionReport => "#EXT-X-RENDITION-REPORT",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tag| tag.name() == name)
    }

    /// 존재 자체가 의미인 태그, 값이 붙으면 문법 오류
    const fn forbids_value(self) -> bool {
        matches!(
            self,
            Self::EndList
                | Self::Discontinuity
                | Self::Gap
                | Self::IndependentSegments
                | Self::IFramesOnly
        )
    }
}

/// URI 줄을 기다리는 `EXTINF`
struct PendingInf {
    duration: SegmentDuration,
    title: String,
    span: SourceSpan,
}

/// URI 줄을 기다리는 `EXT-X-BYTERANGE`, `offset: None`은 직전 range에 이어짐
struct PendingRange {
    length: u64,
    offset: Option<u64>,
    span: SourceSpan,
}

/// URI 줄을 기다리는 `EXT-X-STREAM-INF`
struct PendingVariant {
    attributes: Vec<Attribute>,
    span: SourceSpan,
}

/// URI 줄이 segment를 닫을 때까지 모아 두는 segment별 태그
#[derive(Default)]
struct PendingSegment {
    inf: Option<PendingInf>,
    range: Option<PendingRange>,
    program_time: Option<UtcTimestamp>,
    discontinuity: bool,
    gap: bool,
}
impl PendingSegment {
    fn is_empty(&self) -> bool {
        self.inf.is_none()
            && self.range.is_none()
            && self.program_time.is_none()
            && !self.discontinuity
            && !self.gap
    }
}

/// 이후 segment와 `MAP` 선언에 적용되는 암호화 문맥
struct Encryption {
    by_format: BTreeMap<String, Arc<EncryptionKey>>,
    /// `by_format` 값의 snapshot, 같은 key 아래 선언된 모든 segment가 공유
    shared: Arc<[Arc<EncryptionKey>]>,
    map: Option<Arc<InitializationMap>>,
}

struct State<'a> {
    options: &'a ParseOptions,
    mode: Option<PlaylistKind>,
    singletons: BTreeSet<Tag>,
    metadata: MediaMetadata,
    target_seen: bool,
    variants: Vec<Variant>,
    renditions: Vec<Rendition>,
    pending_variant: Option<PendingVariant>,
    segments: Vec<Segment>,
    pending: PendingSegment,
    encryption: Encryption,
    /// 다음 `EXT-X-BITRATE`가 나올 때까지 이후 모든 segment에 적용
    bitrate_kbps: Option<u64>,
    current_discontinuity_sequence: u64,
    diagnostics: Vec<Diagnostic>,
    omitted: usize,
    blockers: Vec<String>,
}

impl<'a> State<'a> {
    fn new(options: &'a ParseOptions) -> Self {
        Self {
            options,
            mode: None,
            singletons: BTreeSet::new(),
            metadata: MediaMetadata {
                target_duration: 0,
                media_sequence: 0,
                discontinuity_sequence: 0,
                end_list: false,
                playlist_type: None,
                version: None,
                i_frames_only: false,
            },
            target_seen: false,
            variants: vec![],
            renditions: vec![],
            pending_variant: None,
            segments: vec![],
            pending: PendingSegment::default(),
            encryption: Encryption {
                by_format: BTreeMap::new(),
                shared: Arc::from([]),
                map: None,
            },
            bitrate_kbps: None,
            current_discontinuity_sequence: 0,
            diagnostics: vec![],
            omitted: 0,
            blockers: vec![],
        }
    }

    fn tag(&mut self, line: &str, span: SourceSpan) -> Result<(), ParseError> {
        let (name, value) = line
            .split_once(':')
            .map_or((line, None), |(name, value)| (name, Some(value)));
        if self.pending_variant.is_some() {
            return Err(syntax(
                span,
                "STREAM-INF must be followed by URI, comments or blanks",
            ));
        }
        let Some(tag) = Tag::from_name(name) else {
            // RFC 8216 §4.1: 모르는 태그는 무시가 원칙
            // SCTE-35 cue marker나 분석 URL 같은 vendor 태그는 흔하고 시간 의미가 없음
            self.diagnostic(name, span, false);
            return Ok(());
        };
        if tag.forbids_value() && value.is_some() {
            return Err(syntax(span, "tag must not have a value"));
        }
        match tag {
            Tag::Header => Err(error(
                ErrorCode::DuplicateTag,
                Some(span),
                "duplicate header",
            )),
            Tag::Version => {
                self.singleton(tag, span)?;
                self.metadata.version = Some(positive_integer(value_of(value, span)?, span)?);
                Ok(())
            }
            Tag::IndependentSegments => self.singleton(tag, span),
            Tag::Start => {
                self.singleton(tag, span)?;
                self.attributes(value, span)?;
                self.diagnostic(tag.name(), span, false);
                Ok(())
            }
            Tag::StreamInf => self.stream_inf(value, span),
            Tag::Media => self.rendition(value, span),
            Tag::IFrameStreamInf | Tag::SessionData | Tag::SessionKey => {
                self.mode(PlaylistKind::Multivariant, span)?;
                self.attributes(value, span)?;
                self.diagnostic(tag.name(), span, false);
                Ok(())
            }
            Tag::TargetDuration => {
                self.media_singleton(tag, span)?;
                self.metadata.target_duration = positive_integer(value_of(value, span)?, span)?;
                self.target_seen = true;
                Ok(())
            }
            Tag::MediaSequence => self.media_sequence(value, span),
            Tag::DiscontinuitySequence => self.discontinuity_sequence(value, span),
            Tag::PlaylistType => self.playlist_type(value, span),
            Tag::EndList => {
                self.media_singleton(tag, span)?;
                self.metadata.end_list = true;
                Ok(())
            }
            // 각 segment는 I-frame 하나, EXTINF는 다음 I-frame까지의 시간(RFC 8216 §4.3.3.6)
            // 그래서 길이 합산만으로 시간과 항목이 대응됨
            Tag::IFramesOnly => {
                self.media_singleton(tag, span)?;
                self.metadata.i_frames_only = true;
                Ok(())
            }
            Tag::Inf => self.inf(value, span),
            Tag::ProgramDateTime => self.program_date_time(value, span),
            Tag::ByteRange => self.byte_range(value, span),
            Tag::Discontinuity | Tag::Gap => self.segment_flag(tag, span),
            Tag::Key => self.key(value, span),
            Tag::Map => self.map(value, span),
            Tag::DateRange => {
                self.mode(PlaylistKind::Media, span)?;
                self.attributes(value, span)?;
                self.diagnostic(tag.name(), span, false);
                Ok(())
            }
            // Apple segmenter가 segment마다 출력, 정보 제공 목적뿐
            Tag::Bitrate => {
                self.mode(PlaylistKind::Media, span)?;
                self.bitrate_kbps = Some(positive_integer(value_of(value, span)?, span)?);
                Ok(())
            }
            // 프로토콜 버전 7에서 삭제됐지만 오래된 packager가 아직 출력
            // 캐시 힌트일 뿐이라 시간에 영향 없음
            Tag::AllowCache => {
                self.mode(PlaylistKind::Media, span)?;
                self.singleton(tag, span)?;
                if !matches!(value_of(value, span)?, "YES" | "NO") {
                    return Err(invalid(span, "expected YES or NO"));
                }
                self.diagnostic(tag.name(), span, false);
                Ok(())
            }
            Tag::Skip
            | Tag::Part
            | Tag::PartInf
            | Tag::ServerControl
            | Tag::PreloadHint
            | Tag::RenditionReport => {
                self.mode(PlaylistKind::Media, span)?;
                self.diagnostic(tag.name(), span, true);
                Ok(())
            }
        }
    }

    fn uri(&mut self, uri: &str, span: SourceSpan) -> Result<(), ParseError> {
        if uri.chars().any(char::is_whitespace) {
            return Err(invalid(span, "URI contains whitespace"));
        }
        if let Some(variant) = self.pending_variant.take() {
            return self.variant(variant, uri, span);
        }
        self.mode(PlaylistKind::Media, span)?;
        let inf = self.pending.inf.take().ok_or_else(|| {
            error(
                ErrorCode::MissingTag,
                Some(span),
                "segment URI requires EXTINF",
            )
        })?;
        if self.segments.len() >= self.options.max_segments {
            return Err(limit(span));
        }
        let byte_range = self
            .pending
            .range
            .take()
            .map(|range| self.resolve_range(&range, uri))
            .transpose()?;
        let pending = std::mem::take(&mut self.pending);
        if pending.discontinuity {
            self.current_discontinuity_sequence = self
                .current_discontinuity_sequence
                .checked_add(1)
                .ok_or_else(|| overflow(span))?;
        }
        let media_sequence = self
            .metadata
            .media_sequence
            .checked_add(self.segments.len() as u64)
            .ok_or_else(|| overflow(span))?;
        self.segments.push(Segment {
            uri: uri.into(),
            duration: inf.duration,
            title: inf.title,
            media_sequence,
            discontinuity_sequence: self.current_discontinuity_sequence,
            discontinuity: pending.discontinuity,
            gap: pending.gap,
            declared_program_time: pending.program_time,
            bitrate_kbps: self.bitrate_kbps,
            byte_range,
            keys: self.encryption.shared.clone(),
            map: self.encryption.map.clone(),
            span: SourceSpan {
                start: inf.span.start,
                end: span.end,
                line: inf.span.line,
            },
        });
        Ok(())
    }

    fn finish(self, input: &str, lines: Vec<RawLine>) -> Result<ParsedDocument, ParseError> {
        if !self.pending.is_empty() || self.pending_variant.is_some() {
            return Err(error(
                ErrorCode::MissingTag,
                lines.last().map(|l| l.span),
                "unfinished segment or variant at end of playlist",
            ));
        }
        let playlist = if self.mode == Some(PlaylistKind::Multivariant) {
            Playlist::Multivariant(MultivariantPlaylist {
                variants: self.variants,
                renditions: self.renditions,
            })
        } else {
            if !self.target_seen {
                return Err(error(
                    ErrorCode::MissingTag,
                    None,
                    "media playlist requires TARGETDURATION",
                ));
            }
            check_target_duration(&self.segments, self.metadata.target_duration)?;
            Playlist::Media(Arc::new(MediaSnapshot {
                metadata: self.metadata,
                segments: self.segments,
                timeline_blockers: self.blockers,
            }))
        };
        Ok(ParsedDocument {
            source: SourceDocument {
                text: Arc::from(input),
                lines,
            },
            playlist,
            diagnostics: self.diagnostics,
            omitted_diagnostics: self.omitted,
        })
    }

    // ----- media header 태그 -----

    fn media_sequence(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.media_singleton(Tag::MediaSequence, span)?;
        if !self.segments.is_empty() || self.pending.inf.is_some() {
            return Err(order_error(span));
        }
        self.metadata.media_sequence = integer(value_of(value, span)?, span)?;
        Ok(())
    }

    fn discontinuity_sequence(
        &mut self,
        value: Option<&str>,
        span: SourceSpan,
    ) -> Result<(), ParseError> {
        self.media_singleton(Tag::DiscontinuitySequence, span)?;
        if !self.segments.is_empty() || self.pending.discontinuity || self.pending.inf.is_some() {
            return Err(order_error(span));
        }
        let sequence = integer(value_of(value, span)?, span)?;
        self.metadata.discontinuity_sequence = sequence;
        self.current_discontinuity_sequence = sequence;
        Ok(())
    }

    fn playlist_type(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.media_singleton(Tag::PlaylistType, span)?;
        self.metadata.playlist_type = Some(match value_of(value, span)? {
            "EVENT" => PlaylistType::Event,
            "VOD" => PlaylistType::Vod,
            _ => return Err(invalid(span, "expected EVENT or VOD")),
        });
        Ok(())
    }

    // ----- multivariant 태그 -----

    fn stream_inf(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Multivariant, span)?;
        let attributes = self.attributes(value, span)?;
        positive_integer(required(&attributes, "BANDWIDTH", false, span)?, span)?;
        self.pending_variant = Some(PendingVariant { attributes, span });
        Ok(())
    }

    fn variant(
        &mut self,
        variant: PendingVariant,
        uri: &str,
        span: SourceSpan,
    ) -> Result<(), ParseError> {
        if self.variants.len() >= self.options.max_segments {
            return Err(limit(span));
        }
        let bandwidth = positive_integer(
            required(&variant.attributes, "BANDWIDTH", false, variant.span)?,
            variant.span,
        )?;
        self.variants.push(Variant {
            uri: uri.into(),
            bandwidth,
            attributes: variant.attributes,
        });
        Ok(())
    }

    fn rendition(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Multivariant, span)?;
        let attributes = self.attributes(value, span)?;
        let kind = required(&attributes, "TYPE", false, span)?;
        if !matches!(kind, "AUDIO" | "VIDEO" | "SUBTITLES" | "CLOSED-CAPTIONS") {
            return Err(invalid(span, "invalid rendition TYPE"));
        }
        required(&attributes, "GROUP-ID", true, span)?;
        required(&attributes, "NAME", true, span)?;
        if kind == "SUBTITLES" {
            required(&attributes, "URI", true, span)?;
        }
        if kind == "CLOSED-CAPTIONS" {
            required(&attributes, "INSTREAM-ID", true, span)?;
            if attr(&attributes, "URI").is_some() {
                return Err(invalid(span, "CLOSED-CAPTIONS must not have URI"));
            }
        }
        if self.renditions.len() >= self.options.max_segments {
            return Err(limit(span));
        }
        self.renditions.push(Rendition { attributes });
        Ok(())
    }

    // ----- media segment 태그 -----

    fn inf(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        if self.pending.inf.is_some() {
            return Err(duplicate(span, "EXTINF without preceding URI"));
        }
        // RFC 8216에서 쉼표는 필수지만 쉼표 없는 `#EXTINF:10`이 실제로 흔함
        let raw = value_of(value, span)?;
        let (seconds, title) = raw.split_once(',').unwrap_or((raw, ""));
        let duration = SegmentDuration::parse_seconds(seconds).map_err(|e| e.at(span))?;
        self.pending.inf = Some(PendingInf {
            duration,
            title: title.into(),
            span,
        });
        Ok(())
    }

    fn program_date_time(
        &mut self,
        value: Option<&str>,
        span: SourceSpan,
    ) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        let raw = value_of(value, span)?;
        // 실제 packager는 discontinuity 근처에서 태그를 반복하거나 해석 불가 값을 출력
        // 둘 다 문서 전체를 거절할 이유는 아님, 마지막 유효 값을 쓰고 나머지는 기록
        match UtcTimestamp::parse_rfc3339(raw) {
            Ok(time) => {
                if self.pending.program_time.is_some() {
                    self.note(
                        Tag::ProgramDateTime.name(),
                        span,
                        "repeated before a segment; the last value is used",
                    );
                }
                self.pending.program_time = Some(time);
            }
            Err(e) => self.note(
                Tag::ProgramDateTime.name(),
                span,
                &format!("ignored: {}", e.message),
            ),
        }
        Ok(())
    }

    fn byte_range(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        if self.pending.range.is_some() {
            return Err(duplicate(span, "multiple ranges for segment"));
        }
        let (length, offset) = parse_range(value_of(value, span)?, span)?;
        self.pending.range = Some(PendingRange {
            length,
            offset,
            span,
        });
        Ok(())
    }

    /// 암묵적 offset은 같은 URI의 직전 segment range에 이어짐
    fn resolve_range(&self, range: &PendingRange, uri: &str) -> Result<ByteRange, ParseError> {
        let offset = if let Some(offset) = range.offset {
            offset
        } else {
            let previous = self
                .segments
                .last()
                .filter(|s| s.uri == uri)
                .and_then(|s| s.byte_range)
                .ok_or_else(|| {
                    error(
                        ErrorCode::InvalidByteRange,
                        Some(range.span),
                        "implicit range needs previous range on same URI",
                    )
                })?;
            previous
                .offset
                .checked_add(previous.length)
                .ok_or_else(|| overflow(range.span))?
        };
        offset
            .checked_add(range.length)
            .ok_or_else(|| overflow(range.span))?;
        Ok(ByteRange {
            offset,
            length: range.length,
        })
    }

    fn segment_flag(&mut self, tag: Tag, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        let slot = if tag == Tag::Gap {
            &mut self.pending.gap
        } else {
            &mut self.pending.discontinuity
        };
        if *slot {
            return Err(duplicate(span, "repeated segment flag"));
        }
        *slot = true;
        Ok(())
    }

    fn key(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        let attributes = self.attributes(value, span)?;
        let method = required(&attributes, "METHOD", false, span)?;
        if method == "NONE" {
            if attributes.len() != 1 {
                return Err(invalid(span, "METHOD=NONE permits no other attributes"));
            }
            self.encryption.by_format.clear();
        } else {
            let key_format = key_format(&attributes, span)?;
            if !self.encryption.by_format.contains_key(&key_format)
                && self.encryption.by_format.len() >= self.options.max_attributes
            {
                return Err(limit(span));
            }
            let key = parse_key(&attributes, method, key_format, span)?;
            self.encryption
                .by_format
                .insert(key.key_format.clone(), Arc::new(key));
        }
        self.encryption.shared = self
            .encryption
            .by_format
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .into();
        Ok(())
    }

    fn map(&mut self, value: Option<&str>, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        let attributes = self.attributes(value, span)?;
        let uri = required(&attributes, "URI", true, span)?.to_owned();
        let byte_range = if attr(&attributes, "BYTERANGE").is_some() {
            let (length, offset) =
                parse_range(required(&attributes, "BYTERANGE", true, span)?, span)?;
            let offset = offset.ok_or_else(|| {
                error(
                    ErrorCode::InvalidByteRange,
                    Some(span),
                    "MAP BYTERANGE requires explicit offset in v0.1",
                )
            })?;
            offset.checked_add(length).ok_or_else(|| overflow(span))?;
            Some(ByteRange { offset, length })
        } else {
            None
        };
        if self
            .encryption
            .by_format
            .values()
            .any(|k| k.method == EncryptionMethod::Aes128 && k.iv.is_none())
        {
            return Err(invalid(
                span,
                "AES-128 encrypted MAP requires an explicit IV",
            ));
        }
        self.encryption.map = Some(Arc::new(InitializationMap {
            uri,
            byte_range,
            keys: self.encryption.shared.clone(),
        }));
        Ok(())
    }

    // ----- 공통 검사 -----

    fn mode(&mut self, kind: PlaylistKind, span: SourceSpan) -> Result<(), ParseError> {
        if self.mode.is_some_and(|mode| mode != kind) {
            return Err(error(
                ErrorCode::MixedPlaylist,
                Some(span),
                "multivariant and media tags cannot be mixed",
            ));
        }
        self.mode = Some(kind);
        Ok(())
    }

    fn singleton(&mut self, tag: Tag, span: SourceSpan) -> Result<(), ParseError> {
        if !self.singletons.insert(tag) {
            return Err(duplicate(span, format!("duplicate {}", tag.name())));
        }
        Ok(())
    }

    fn media_singleton(&mut self, tag: Tag, span: SourceSpan) -> Result<(), ParseError> {
        self.mode(PlaylistKind::Media, span)?;
        self.singleton(tag, span)
    }

    fn attributes(
        &self,
        value: Option<&str>,
        span: SourceSpan,
    ) -> Result<Vec<Attribute>, ParseError> {
        parse_attributes(value_of(value, span)?, span, self.options.max_attributes)
    }

    fn diagnostic(&mut self, tag: &str, span: SourceSpan, blocks_timeline: bool) {
        // blocker는 diagnostic 개수 제한과 따로 셈, diagnostic이 잘려도 "시간 인덱스 불가"는 남아야 함
        // 32는 안전장치, 실제로 들어오는 건 LL-HLS 태그 6종뿐
        if blocks_timeline && self.blockers.len() < 32 && !self.blockers.iter().any(|b| b == tag) {
            self.blockers.push(tag.into());
        }
        self.note(tag, span, "tag preserved; semantics not implemented");
    }

    fn note(&mut self, tag: &str, span: SourceSpan, message: &str) {
        if self.diagnostics.len() < self.options.max_diagnostics {
            self.diagnostics.push(Diagnostic {
                tag: tag.into(),
                span,
                message: message.into(),
            });
        } else {
            self.omitted += 1;
        }
    }
}

fn key_format(attributes: &[Attribute], span: SourceSpan) -> Result<String, ParseError> {
    Ok(match attr(attributes, "KEYFORMAT") {
        Some(_) => required(attributes, "KEYFORMAT", true, span)?,
        None => "identity",
    }
    .to_owned())
}

fn parse_key(
    attributes: &[Attribute],
    method: &str,
    key_format: String,
    span: SourceSpan,
) -> Result<EncryptionKey, ParseError> {
    let uri = required(attributes, "URI", true, span)?.to_owned();
    if attr(attributes, "IV").is_some() {
        let iv = required(attributes, "IV", false, span)?;
        let hex = iv.strip_prefix("0x").or_else(|| iv.strip_prefix("0X"));
        if !hex.is_some_and(|h| {
            !h.is_empty() && h.len() <= 32 && h.bytes().all(|b| b.is_ascii_hexdigit())
        }) {
            return Err(invalid(span, "IV must be at most 128-bit hexadecimal"));
        }
    }
    if attr(attributes, "KEYFORMATVERSIONS").is_some() {
        let versions = required(attributes, "KEYFORMATVERSIONS", true, span)?;
        for version in versions.split('/') {
            positive_integer(version, span)?;
        }
    }
    Ok(EncryptionKey {
        method: match method {
            "AES-128" => EncryptionMethod::Aes128,
            "SAMPLE-AES" => EncryptionMethod::SampleAes,
            other => EncryptionMethod::Other(other.into()),
        },
        uri,
        iv: attr(attributes, "IV").map(str::to_owned),
        key_format,
        key_format_versions: attr(attributes, "KEYFORMATVERSIONS").map(str::to_owned),
    })
}

/// RFC 8216 §4.3.3.1: 각 `EXTINF`를 정수로 반올림한 값은 target 이하
fn check_target_duration(segments: &[Segment], target: u64) -> Result<(), ParseError> {
    for segment in segments {
        let nanos = segment.duration.as_nanos();
        let rounded =
            nanos / NANOS_PER_SECOND + u64::from(nanos % NANOS_PER_SECOND >= NANOS_PER_SECOND / 2);
        if rounded > target {
            return Err(invalid(
                segment.span,
                "rounded EXTINF exceeds TARGETDURATION",
            ));
        }
    }
    Ok(())
}

fn value_of(value: Option<&str>, span: SourceSpan) -> Result<&str, ParseError> {
    value.ok_or_else(|| syntax(span, "tag requires a value"))
}

fn integer(value: &str, span: SourceSpan) -> Result<u64, ParseError> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(span, "expected decimal integer"));
    }
    value.parse().map_err(|_| overflow(span))
}

fn positive_integer(value: &str, span: SourceSpan) -> Result<u64, ParseError> {
    let n = integer(value, span)?;
    if n == 0 {
        return Err(invalid(span, "expected positive integer"));
    }
    Ok(n)
}

/// `EXT-X-BYTERANGE`와 `MAP`의 `BYTERANGE` attribute가 쓰는 `length[@offset]` 형식
fn parse_range(value: &str, span: SourceSpan) -> Result<(u64, Option<u64>), ParseError> {
    let (length, offset) = value
        .split_once('@')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    Ok((
        positive_integer(length, span)?,
        offset.map(|v| integer(v, span)).transpose()?,
    ))
}

fn error(code: ErrorCode, span: Option<SourceSpan>, message: impl Into<String>) -> ParseError {
    ParseError::new(code, span, message)
}
fn syntax(span: SourceSpan, message: impl Into<String>) -> ParseError {
    error(ErrorCode::InvalidSyntax, Some(span), message)
}
fn invalid(span: SourceSpan, message: impl Into<String>) -> ParseError {
    error(ErrorCode::InvalidValue, Some(span), message)
}
fn duplicate(span: SourceSpan, message: impl Into<String>) -> ParseError {
    error(ErrorCode::DuplicateTag, Some(span), message)
}
fn limit(span: SourceSpan) -> ParseError {
    error(
        ErrorCode::InputLimit,
        Some(span),
        "model collection limit exceeded",
    )
}
fn overflow(span: SourceSpan) -> ParseError {
    error(ErrorCode::Overflow, Some(span), "integer overflow")
}
fn order_error(span: SourceSpan) -> ParseError {
    syntax(
        span,
        "sequence tag must precede segments and discontinuities",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPAN: SourceSpan = SourceSpan {
        start: 0,
        end: 0,
        line: 1,
    };

    #[test]
    fn every_tag_name_round_trips_and_is_unique() {
        for tag in Tag::ALL {
            assert_eq!(Tag::from_name(tag.name()), Some(tag));
        }
        let mut names: Vec<_> = Tag::ALL.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Tag::ALL.len());
        assert_eq!(Tag::from_name("#EXT-X-UNKNOWN"), None);
    }

    #[test]
    fn tabs_are_text_but_other_control_characters_are_rejected() {
        let doc = parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.00000,\t\nfileSequence0.mp4\n",
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(doc.media().unwrap().segments()[0].title(), "\t");
        for control in ['\u{0}', '\u{7}', '\u{b}', '\u{1b}'] {
            let text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,{control}\na\n");
            assert_eq!(
                parse(&text, &ParseOptions::default()).unwrap_err().code,
                ErrorCode::InvalidSyntax
            );
        }
    }

    #[test]
    fn allow_cache_is_validated_but_never_blocks_the_timeline() {
        let doc = parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-ALLOW-CACHE:YES\n#EXTINF:6,\na\n",
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(doc.diagnostics()[0].tag, "#EXT-X-ALLOW-CACHE");
        assert!(doc.media().unwrap().timeline_blockers().is_empty());
        for bad in [
            "#EXT-X-ALLOW-CACHE\n",
            "#EXT-X-ALLOW-CACHE:MAYBE\n",
            "#EXT-X-ALLOW-CACHE:YES\n#EXT-X-ALLOW-CACHE:NO\n",
        ] {
            let text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n{bad}#EXTINF:6,\na\n");
            assert!(parse(&text, &ParseOptions::default()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn i_frames_only_is_a_flag_that_keeps_the_timeline() {
        let plain = parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\na\n",
            &ParseOptions::default(),
        )
        .unwrap();
        assert!(!plain.media().unwrap().metadata().i_frames_only());

        let doc = parse(
            "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-I-FRAMES-ONLY\n#EXTINF:2,\na\n",
            &ParseOptions::default(),
        )
        .unwrap();
        assert!(doc.diagnostics().is_empty());
        let media = doc.media().unwrap();
        assert!(media.metadata().i_frames_only());
        assert!(media.timeline_blockers().is_empty());

        for bad in [
            "#EXT-X-I-FRAMES-ONLY:YES\n",
            "#EXT-X-I-FRAMES-ONLY\n#EXT-X-I-FRAMES-ONLY\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n#EXT-X-I-FRAMES-ONLY\n",
        ] {
            let text = format!("#EXTM3U\n#EXT-X-TARGETDURATION:6\n{bad}#EXTINF:6,\na\n");
            assert!(parse(&text, &ParseOptions::default()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn integers_reject_signs_whitespace_and_overflow() {
        assert_eq!(integer("0", SPAN).unwrap(), 0);
        assert_eq!(integer("18446744073709551615", SPAN).unwrap(), u64::MAX);
        assert_eq!(
            integer("18446744073709551616", SPAN).unwrap_err().code,
            ErrorCode::Overflow
        );
        for bad in ["", "+1", "-1", " 1", "1 ", "1.0", "0x10"] {
            assert_eq!(
                integer(bad, SPAN).unwrap_err().code,
                ErrorCode::InvalidValue,
                "{bad:?}"
            );
        }
        assert_eq!(
            positive_integer("0", SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(positive_integer("7", SPAN).unwrap(), 7);
    }

    #[test]
    fn byte_range_syntax_is_length_with_optional_offset() {
        assert_eq!(parse_range("100", SPAN).unwrap(), (100, None));
        assert_eq!(parse_range("100@0", SPAN).unwrap(), (100, Some(0)));
        assert_eq!(
            parse_range("0@5", SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(
            parse_range("10@", SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(
            parse_range("10@-1", SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
    }

    #[test]
    fn target_duration_uses_round_half_up_on_the_nanosecond_value() {
        let segment = |nanos: u64| {
            let seconds = format!(
                "{}.{:09}",
                nanos / NANOS_PER_SECOND,
                nanos % NANOS_PER_SECOND
            );
            let mut doc = parse(
                &format!("#EXTM3U\n#EXT-X-TARGETDURATION:100\n#EXTINF:{seconds},\na\n"),
                &ParseOptions::default(),
            )
            .unwrap();
            let media = Arc::get_mut(match &mut doc.playlist {
                Playlist::Media(media) => media,
                Playlist::Multivariant(_) => unreachable!(),
            })
            .unwrap();
            std::mem::take(&mut media.segments).remove(0)
        };
        let just_under = segment(6_499_999_999);
        let half = segment(6_500_000_000);
        assert!(check_target_duration(std::slice::from_ref(&just_under), 6).is_ok());
        assert_eq!(
            check_target_duration(std::slice::from_ref(&half), 6)
                .unwrap_err()
                .code,
            ErrorCode::InvalidValue
        );
        assert!(check_target_duration(std::slice::from_ref(&half), 7).is_ok());
    }
}
