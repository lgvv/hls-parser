//! 다른 언어에서 hls-core를 쓰기 위한 UniFFI 경계
//! core 타입을 그대로 내보내지 않고 복사본 DTO와 불변 핸들만 넘김
//! 네트워킹, 플레이어, async 런타임은 없음
use hls_core as hls;
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};

mod dto;
pub use dto::*;
uniffi::setup_scaffolding!();

/// snapshot token 발급기, 0은 "token 없음"으로 남겨 두려고 1부터 시작
static NEXT_SNAPSHOT: AtomicU64 = AtomicU64::new(1);
/// 한 번에 넘기는 segment 상한, FFI 복사 비용을 호출자가 예측할 수 있는 크기
const MAX_BATCH: u64 = 1024;

/// 바인딩이 보고하는 모든 실패
/// code는 안정적, message는 바뀔 수 있음
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum HlsError {
    /// 파싱 실패
    #[error("{message}")]
    Parse {
        /// 안정적인 분류
        code: ParseErrorCode,
        /// 실패를 감지한 위치, 알 수 있을 때만
        span: Option<SpanDto>,
        /// 사람이 읽는 상세 설명
        message: String,
    },
    /// timeline 생성 실패
    #[error("{message}")]
    Timeline {
        /// 안정적인 분류
        code: TimelineErrorCode,
        /// 문제를 감지한 segment, 알 수 있을 때만
        segment_index: Option<u64>,
        /// 사람이 읽는 상세 설명
        message: String,
    },
    /// Multivariant 문서에 Media 전용 연산을 호출
    #[error("this operation requires a media playlist")]
    WrongPlaylist,
    /// 인덱스나 배치 범위가 문서 밖, 또는 배치가 1,024개 초과
    #[error("index or requested range is out of bounds (batch maximum: 1024)")]
    OutOfBounds,
    /// 다른 snapshot이 발급한 seek target
    #[error("seek target does not belong to this snapshot")]
    SnapshotMismatch,
    /// 0이거나 플랫폼 word 크기를 넘는 제한값
    #[error("invalid parse limits")]
    InvalidLimits,
    /// token 카운터가 u64 끝까지 가서 새 snapshot을 더 만들 수 없음
    #[error("snapshot identifier space exhausted")]
    IdentifierExhausted,
    /// 해석할 수 없는 base URL 또는 URI
    #[error("invalid URI or base URL: {message}")]
    InvalidUrl {
        /// URL 파서의 상세 설명
        message: String,
    },
    /// tracker가 거절한 갱신, tracker 상태는 그대로
    #[error("{message}")]
    Tracker {
        /// 안정적인 분류
        code: TrackerErrorCode,
        /// 충돌한 media sequence, 실패가 segment 하나에 관한 것일 때만
        media_sequence: Option<u64>,
        /// 사람이 읽는 상세 설명
        message: String,
    },
}

impl From<hls::TrackerError> for HlsError {
    fn from(error: hls::TrackerError) -> Self {
        let (code, media_sequence) = match &error {
            hls::TrackerError::ConflictingSegment { media_sequence } => {
                (TrackerErrorCode::ConflictingSegment, Some(*media_sequence))
            }
            hls::TrackerError::UnsupportedTimingFeature { .. } => {
                (TrackerErrorCode::UnsupportedTimingFeature, None)
            }
            _ => (TrackerErrorCode::Unknown, None),
        };
        Self::Tracker {
            code,
            media_sequence,
            message: error.to_string(),
        }
    }
}

impl From<hls::ParseError> for HlsError {
    fn from(error: hls::ParseError) -> Self {
        let code = match error.code {
            hls::ErrorCode::InputLimit => ParseErrorCode::InputLimit,
            hls::ErrorCode::InvalidHeader => ParseErrorCode::InvalidHeader,
            hls::ErrorCode::InvalidSyntax => ParseErrorCode::InvalidSyntax,
            hls::ErrorCode::InvalidValue => ParseErrorCode::InvalidValue,
            hls::ErrorCode::UnsupportedPrecision => ParseErrorCode::UnsupportedPrecision,
            hls::ErrorCode::DuplicateTag => ParseErrorCode::DuplicateTag,
            hls::ErrorCode::MissingTag => ParseErrorCode::MissingTag,
            hls::ErrorCode::MixedPlaylist => ParseErrorCode::MixedPlaylist,
            hls::ErrorCode::InvalidByteRange => ParseErrorCode::InvalidByteRange,
            hls::ErrorCode::Overflow => ParseErrorCode::Overflow,
            _ => ParseErrorCode::Unknown,
        };
        Self::Parse {
            code,
            span: error.span.map(Into::into),
            message: error.message,
        }
    }
}
impl From<hls::TimelineError> for HlsError {
    fn from(error: hls::TimelineError) -> Self {
        let (code, segment_index) = match &error {
            hls::TimelineError::EmptySnapshot => (TimelineErrorCode::EmptySnapshot, None),
            hls::TimelineError::MissingTimeMapping { segment_index } => (
                TimelineErrorCode::MissingTimeMapping,
                Some(*segment_index as u64),
            ),
            hls::TimelineError::ConflictingTimeMapping { segment_index } => (
                TimelineErrorCode::ConflictingTimeMapping,
                Some(*segment_index as u64),
            ),
            hls::TimelineError::UnsupportedTimingFeature { .. } => {
                (TimelineErrorCode::UnsupportedTimingFeature, None)
            }
            hls::TimelineError::Overflow { segment_index } => {
                (TimelineErrorCode::Overflow, Some(*segment_index as u64))
            }
            _ => (TimelineErrorCode::Unknown, None),
        };
        Self::Timeline {
            code,
            segment_index,
            message: error.to_string(),
        }
    }
}

/// 파싱된 문서
/// 불변이고 `Send + Sync`, 어느 스레드에서든 조회 가능
#[derive(uniffi::Object)]
pub struct HlsDocument {
    document: hls::ParsedDocument,
    token: u64,
}

/// core의 기본 제한값, 필드 하나만 바꿔 쓰기 위한 출발점
#[uniffi::export]
pub fn default_parse_limits() -> ParseLimits {
    let o = hls::ParseOptions::default();
    ParseLimits {
        max_input_bytes: o.max_input_bytes as u64,
        max_line_bytes: o.max_line_bytes as u64,
        max_lines: o.max_lines as u64,
        max_segments: o.max_segments as u64,
        max_attributes: o.max_attributes as u64,
        max_diagnostics: o.max_diagnostics as u64,
    }
}

/// 기본 제한값으로 파싱
#[uniffi::export]
pub fn parse_playlist(text: String) -> Result<Arc<HlsDocument>, HlsError> {
    parse_playlist_with_limits(text, default_parse_limits())
}

/// 명시한 제한값으로 파싱
/// 성공할 때마다 새 snapshot token 발급
#[uniffi::export]
pub fn parse_playlist_with_limits(
    text: String,
    limits: ParseLimits,
) -> Result<Arc<HlsDocument>, HlsError> {
    let number = |value| usize::try_from(value).map_err(|_| HlsError::InvalidLimits);
    if limits.max_input_bytes == 0
        || limits.max_line_bytes == 0
        || limits.max_lines == 0
        || limits.max_segments == 0
        || limits.max_attributes == 0
    {
        return Err(HlsError::InvalidLimits);
    }
    let options = hls::ParseOptions {
        max_input_bytes: number(limits.max_input_bytes)?,
        max_line_bytes: number(limits.max_line_bytes)?,
        max_lines: number(limits.max_lines)?,
        max_segments: number(limits.max_segments)?,
        max_attributes: number(limits.max_attributes)?,
        max_diagnostics: number(limits.max_diagnostics)?,
    };
    let document = hls::parse(&text, &options)?;
    let token = next_token()?;
    Ok(Arc::new(HlsDocument { document, token }))
}

/// RFC 3339 timestamp를 Unix 나노초로 파싱, 정밀도 규칙은 core와 동일
#[uniffi::export]
pub fn parse_utc_timestamp(text: String) -> Result<i64, HlsError> {
    Ok(hls::UtcTimestamp::parse_rfc3339(&text)?.as_unix_nanos())
}

/// 최종 playlist 응답 URL 기준의 URI 해석, 네트워크 요청 없음
#[uniffi::export]
pub fn resolve_playlist_uri(base_url: String, uri: String) -> Result<String, HlsError> {
    hls::resolve_uri(&base_url, &uri).map_err(|e| HlsError::InvalidUrl {
        message: e.to_string(),
    })
}

impl HlsDocument {
    fn media(&self) -> Result<&Arc<hls::MediaSnapshot>, HlsError> {
        self.document.media().ok_or(HlsError::WrongPlaylist)
    }
}

#[uniffi::export]
impl HlsDocument {
    /// 이 snapshot의 프로세스 전역 식별자, 프로세스 밖에서는 의미 없음
    pub fn snapshot_token(&self) -> u64 {
        self.token
    }
    /// 내용 복사 없는 개수 정보
    pub fn summary(&self) -> DocumentSummary {
        let (kind, segment_count, variant_count) = match self.document.playlist() {
            hls::Playlist::Media(m) => (PlaylistKind::Media, m.segment_count() as u64, 0),
            hls::Playlist::Multivariant(m) => {
                (PlaylistKind::Multivariant, 0, m.variants().len() as u64)
            }
        };
        DocumentSummary {
            kind,
            segment_count,
            variant_count,
            diagnostic_count: self.document.diagnostics().len() as u64,
            omitted_diagnostics: self.document.omitted_diagnostics() as u64,
        }
    }
    /// Media playlist의 playlist 수준 태그
    pub fn media_metadata(&self) -> Result<MediaMetadataDto, HlsError> {
        Ok(self.media()?.metadata().into())
    }
    /// Media playlist의 segment 수
    pub fn segment_count(&self) -> Result<u64, HlsError> {
        Ok(self.media()?.segment_count() as u64)
    }
    /// `index`번째 segment, 첫 segment가 0
    pub fn segment_at(&self, index: u64) -> Result<SegmentDto, HlsError> {
        Ok(self
            .media()?
            .segment_at(to_index(index)?)
            .ok_or(HlsError::OutOfBounds)?
            .into())
    }
    /// `start`번째부터 `count`개 segment, 한 번에 최대 1,024개
    pub fn segments_in_range(&self, start: u64, count: u64) -> Result<Vec<SegmentDto>, HlsError> {
        let media = self.media()?;
        let end = start.checked_add(count).ok_or(HlsError::OutOfBounds)?;
        if count > MAX_BATCH || end > media.segment_count() as u64 {
            return Err(HlsError::OutOfBounds);
        }
        let range = to_index(start)?..to_index(end)?;
        Ok(media.segments()[range].iter().map(Into::into).collect())
    }
    /// timeline 조회가 가리킨 segment, 이 snapshot 소속인지 검사 후 반환
    pub fn segment_for_target(&self, target: SeekTargetDto) -> Result<SegmentDto, HlsError> {
        if target.snapshot_token != self.token {
            return Err(HlsError::SnapshotMismatch);
        }
        let media = self.media()?;
        let segment = media
            .segment_at(to_index(target.segment_index)?)
            .ok_or(HlsError::OutOfBounds)?;
        if segment.media_sequence() != target.media_sequence
            || target.offset_nanos >= segment.duration().as_nanos()
        {
            return Err(HlsError::SnapshotMismatch);
        }
        Ok(segment.into())
    }
    /// 명시적인 O(N) 변환, 반복 검색에는 timeline 핸들 사용
    pub fn export_playlist(&self) -> PlaylistDto {
        match self.document.playlist() {
            hls::Playlist::Media(media) => PlaylistDto::Media {
                metadata: media.metadata().into(),
                segments: media.segments().iter().map(Into::into).collect(),
            },
            hls::Playlist::Multivariant(master) => PlaylistDto::Multivariant {
                variants: master
                    .variants()
                    .iter()
                    .map(|v| VariantDto {
                        uri: v.uri().into(),
                        bandwidth: v.bandwidth(),
                        attributes: attributes(v.attributes()),
                    })
                    .collect(),
                renditions: master
                    .renditions()
                    .iter()
                    .map(|r| RenditionDto {
                        attributes: attributes(r.attributes()),
                    })
                    .collect(),
            },
        }
    }
    /// 파싱한 그대로의 입력
    pub fn source_text(&self) -> String {
        self.document.source().text().into()
    }
    /// 원문 줄 수
    pub fn source_line_count(&self) -> u64 {
        self.document.source().lines().len() as u64
    }
    /// `index`번째 원문 줄, 첫 줄이 0
    /// 반환값 안의 `SpanDto.line`은 사람이 읽는 번호라 1부터
    pub fn source_line_at(&self, index: u64) -> Result<RawLineDto, HlsError> {
        let line = self
            .document
            .source()
            .lines()
            .get(to_index(index)?)
            .ok_or(HlsError::OutOfBounds)?;
        let kind = match line.kind {
            hls::LineKind::Header => RawLineKind::Header,
            hls::LineKind::Tag => RawLineKind::Tag,
            hls::LineKind::Comment => RawLineKind::Comment,
            hls::LineKind::Uri => RawLineKind::Uri,
            hls::LineKind::Blank => RawLineKind::Blank,
        };
        Ok(RawLineDto {
            kind,
            span: line.span.into(),
            text: self
                .document
                .source()
                .text_at(line.span)
                .unwrap_or_default()
                .into(),
        })
    }
    /// 의미 해석 없이 보존한 태그, diagnostics 제한까지만
    pub fn diagnostics(&self) -> Vec<DiagnosticDto> {
        self.document
            .diagnostics()
            .iter()
            .map(|d| DiagnosticDto {
                tag: d.tag.clone(),
                span: d.span.into(),
                message: d.message.clone(),
            })
            .collect()
    }
    /// 첫 segment부터의 경과 시간을 키로 하는 인덱스 생성
    pub fn build_relative_timeline(&self) -> Result<Arc<HlsRelativeTimeline>, HlsError> {
        Ok(Arc::new(HlsRelativeTimeline {
            timeline: hls::RelativeTimeline::build(self.media()?.clone())?,
            token: self.token,
        }))
    }
    /// UTC를 키로 하는 인덱스 생성, program-date-time이 불완전하면 실패
    pub fn build_wall_clock_timeline(&self) -> Result<Arc<HlsWallClockTimeline>, HlsError> {
        Ok(Arc::new(HlsWallClockTimeline {
            timeline: hls::WallClockTimeline::build(self.media()?.clone())?,
            token: self.token,
        }))
    }
}

/// 첫 segment부터의 경과 시간을 키로 하는 인덱스, 문서 해제 후에도 유효
#[derive(uniffi::Object)]
pub struct HlsRelativeTimeline {
    timeline: hls::RelativeTimeline,
    token: u64,
}
#[uniffi::export]
impl HlsRelativeTimeline {
    /// 이 timeline이 색인한 snapshot의 token
    pub fn snapshot_token(&self) -> u64 {
        self.token
    }
    /// 모든 segment 길이의 합
    pub fn duration_nanos(&self) -> u64 {
        self.timeline.duration().as_nanos()
    }
    /// `elapsed_nanos`를 포함하는 segment의 이진 탐색
    pub fn locate(&self, elapsed_nanos: u64) -> LocateResult {
        result(
            self.timeline
                .locate(hls::ElapsedTime::from_nanos(elapsed_nanos)),
            self.token,
        )
    }
}

/// UTC를 키로 하는 인덱스, 문서 해제 후에도 유효
#[derive(uniffi::Object)]
pub struct HlsWallClockTimeline {
    timeline: hls::WallClockTimeline,
    token: u64,
}
#[uniffi::export]
impl HlsWallClockTimeline {
    /// 이 timeline이 색인한 snapshot의 token
    pub fn snapshot_token(&self) -> u64 {
        self.token
    }
    /// 첫 segment의 시작 시각
    pub fn start_unix_nanos(&self) -> i64 {
        self.timeline.start().as_unix_nanos()
    }
    /// 마지막 segment의 종료 시각(exclusive)
    pub fn end_unix_nanos(&self) -> i64 {
        self.timeline.end().as_unix_nanos()
    }
    /// `unix_nanos`를 포함하는 segment의 이진 탐색
    pub fn locate(&self, unix_nanos: i64) -> LocateResult {
        result(
            self.timeline
                .locate(hls::UtcTimestamp::from_unix_nanos(unix_nanos)),
            self.token,
        )
    }
}

/// 플랫폼에서 넘어온 `u64` 인덱스의 `usize` 변환, 오버플로는 범위 밖으로 취급
/// export 함수의 매개변수 이름은 UniFFI가 생성 바인딩의 인자 이름으로 그대로 쓰므로 `index` 유지
fn to_index(value: u64) -> Result<usize, HlsError> {
    usize::try_from(value).map_err(|_| HlsError::OutOfBounds)
}

/// core의 tracker 기본값
#[uniffi::export]
pub fn default_tracker_options() -> TrackerOptions {
    let o = hls::TrackerOptions::default();
    TrackerOptions {
        max_segments: o.max_segments as u64,
        pdt_tolerance_nanos: o.pdt_tolerance_nanos,
    }
}

/// 갱신을 거쳐 병합한 media playlist 하나의 이력
/// core의 tracker는 `&mut`가 필요하지만 바인딩 객체는 공유 참조로만 오므로 Mutex로 감쌈
/// 그래서 스레드 간 공유가 가능하고 매 호출이 lock을 잡음, 의미는 `hls_core::PlaylistTracker`와 동일
#[derive(uniffi::Object)]
pub struct HlsPlaylistTracker {
    inner: Mutex<hls::PlaylistTracker>,
    token: AtomicU64,
}

impl HlsPlaylistTracker {
    fn lock(&self) -> MutexGuard<'_, hls::PlaylistTracker> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn timed<T>(value: Result<T, hls::TimelineError>) -> Result<T, HlsError> {
        value.map_err(Into::into)
    }
}

#[uniffi::export]
impl HlsPlaylistTracker {
    /// 주어진 제한값의 빈 이력
    #[uniffi::constructor]
    pub fn new(options: TrackerOptions) -> Result<Arc<Self>, HlsError> {
        let max_segments =
            usize::try_from(options.max_segments).map_err(|_| HlsError::InvalidLimits)?;
        if max_segments == 0 {
            return Err(HlsError::InvalidLimits);
        }
        let inner = hls::PlaylistTracker::new(hls::TrackerOptions {
            max_segments,
            pdt_tolerance_nanos: options.pdt_tolerance_nanos,
        });
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            token: AtomicU64::new(next_token()?),
        }))
    }
    /// 이 tracker가 발급하는 seek target의 token, `reset` 때 교체
    pub fn snapshot_token(&self) -> u64 {
        self.token.load(Ordering::Relaxed)
    }
    /// 갱신된 media playlist 병합, Multivariant 문서는 거절
    pub fn apply(&self, document: Arc<HlsDocument>) -> Result<TrackerUpdate, HlsError> {
        let media = document.media()?;
        let update = self.lock().apply(media)?;
        Ok(TrackerUpdate {
            added: update.added as u64,
            evicted: update.evicted as u64,
            stale: update.stale,
            timing_error: update.timing_error.map(Into::into),
        })
    }
    /// 이력이 바뀔 때마다 1씩 증가하는 값
    pub fn generation(&self) -> u64 {
        self.lock().generation()
    }
    /// 추적 중인 segment 수
    pub fn segment_count(&self) -> u64 {
        self.lock().segment_count() as u64
    }
    /// 어느 갱신에서든 `EXT-X-ENDLIST`를 받았는지 여부
    pub fn end_list(&self) -> bool {
        self.lock().end_list()
    }
    /// 가장 새로운 추적 segment
    pub fn live_edge(&self) -> Option<SegmentDto> {
        self.lock().live_edge().map(Into::into)
    }
    /// 이력이 덮는 UTC 구간
    pub fn window(&self) -> Result<TimeWindowDto, HlsError> {
        let (start, end) = Self::timed(self.lock().window())?;
        Ok(TimeWindowDto {
            start_unix_nanos: start.as_unix_nanos(),
            end_unix_nanos: end.as_unix_nanos(),
        })
    }
    /// 전체 이력에 대한 이진 탐색
    pub fn locate(&self, unix_nanos: i64) -> Result<LocateResult, HlsError> {
        let found = Self::timed(
            self.lock()
                .locate(hls::UtcTimestamp::from_unix_nanos(unix_nanos)),
        )?;
        Ok(result(found, self.snapshot_token()))
    }
    /// 재생 순서에서 해당 시각에 가장 가까운 재생 가능 위치
    pub fn locate_nearest(&self, unix_nanos: i64) -> Result<Option<SeekTargetDto>, HlsError> {
        let nearest = Self::timed(
            self.lock()
                .locate_nearest(hls::UtcTimestamp::from_unix_nanos(unix_nanos)),
        )?;
        Ok(nearest.map(|target| seek_target(target, self.snapshot_token())))
    }
    /// target이 가리키는 segment, 이 tracker 소속인지 검사 후 반환
    pub fn segment_for_target(&self, target: SeekTargetDto) -> Result<SegmentDto, HlsError> {
        if target.snapshot_token != self.snapshot_token() {
            return Err(HlsError::SnapshotMismatch);
        }
        let core_target = hls::SeekTarget::new_for_ffi(
            to_index(target.segment_index)?,
            target.media_sequence,
            hls::ElapsedTime::from_nanos(target.offset_nanos),
        );
        self.lock()
            .segment_for_target(core_target)
            .map(Into::into)
            .ok_or(HlsError::SnapshotMismatch)
    }
    /// media sequence 번호로 찾은 segment
    pub fn segment_by_sequence(&self, media_sequence: u64) -> Result<SegmentDto, HlsError> {
        self.lock()
            .segment_by_sequence(media_sequence)
            .map(|(_, segment)| segment.into())
            .ok_or(HlsError::OutOfBounds)
    }
    /// 주어진 sequence 다음에 재생할 segment, live edge에서는 `None`
    pub fn segment_after(&self, media_sequence: u64) -> Option<NextSegmentDto> {
        let tracker = self.lock();
        let next = tracker.segment_after(media_sequence)?;
        Some(NextSegmentDto {
            segment: (&tracker.segments()[next.segment_index]).into(),
            contiguous: next.contiguous,
        })
    }
    /// `start`번째부터 `count`개 추적 segment, 한 번에 최대 1,024개
    pub fn segments_in_range(&self, start: u64, count: u64) -> Result<Vec<SegmentDto>, HlsError> {
        let tracker = self.lock();
        let end = start.checked_add(count).ok_or(HlsError::OutOfBounds)?;
        if count > MAX_BATCH || end > tracker.segment_count() as u64 {
            return Err(HlsError::OutOfBounds);
        }
        let range = to_index(start)?..to_index(end)?;
        Ok(tracker.segments()[range].iter().map(Into::into).collect())
    }
    /// 해당 시각 이전에 끝나는 segment 제거, 반환값은 제거한 개수
    pub fn trim_before(&self, unix_nanos: i64) -> Result<u64, HlsError> {
        let dropped = Self::timed(
            self.lock()
                .trim_before(hls::UtcTimestamp::from_unix_nanos(unix_nanos)),
        )?;
        Ok(dropped as u64)
    }
    /// 이력 전체 삭제와 새 token 발급, 이전 seek target은 무효
    pub fn reset(&self) -> Result<(), HlsError> {
        self.lock().reset();
        self.token.store(next_token()?, Ordering::Relaxed);
        Ok(())
    }
}

fn next_token() -> Result<u64, HlsError> {
    NEXT_SNAPSHOT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| HlsError::IdentifierExhausted)
}

fn seek_target(target: hls::SeekTarget, token: u64) -> SeekTargetDto {
    SeekTargetDto {
        snapshot_token: token,
        segment_index: target.segment_index() as u64,
        media_sequence: target.media_sequence(),
        offset_nanos: target.offset_in_segment().as_nanos(),
    }
}

fn result(value: hls::LocateResult, token: u64) -> LocateResult {
    match value {
        hls::LocateResult::Found(target) => LocateResult::Found {
            target: seek_target(target, token),
        },
        hls::LocateResult::Gap { segment_index } => LocateResult::Gap {
            segment_index: segment_index.map(|i| i as u64),
        },
        hls::LocateResult::BeforeWindow => LocateResult::BeforeWindow,
        hls::LocateResult::AfterWindow => LocateResult::AfterWindow,
    }
}
