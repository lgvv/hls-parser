//! FFI 경계를 넘는 값 타입
//! 모든 DTO는 복사본, core 참조 없음

use hls_core as hls;

/// [`hls::ErrorCode`]와 같은 variant 목록
/// core가 나중에 추가한 값은 이 바인딩 버전에서 `Unknown`으로 받음
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ParseErrorCode {
    /// 파싱 제한 초과
    InputLimit,
    /// 첫 줄이 `#EXTM3U`가 아니거나 빈 입력
    InvalidHeader,
    /// 태그 문법에 맞지 않는 줄 또는 attribute list
    InvalidSyntax,
    /// 형태는 맞지만 의미가 허용되지 않는 값
    InvalidValue,
    /// 나노초 표현을 넘는 소수 자릿수
    UnsupportedPrecision,
    /// 한 번만 허용되는 태그의 반복
    DuplicateTag,
    /// 문서에 필요한 태그 또는 attribute 누락
    MissingTag,
    /// 한 문서에 섞인 Multivariant 태그와 Media 태그
    MixedPlaylist,
    /// 해석할 수 없는 byte range
    InvalidByteRange,
    /// 정수 연산 또는 변환의 오버플로
    Overflow,
    /// 이 바인딩 버전이 모르는 core 오류 코드
    Unknown,
}
/// [`hls::TimelineError`]와 같은 variant 목록, 모르는 값은 `Unknown`
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum TimelineErrorCode {
    /// segment가 없는 snapshot
    EmptySnapshot,
    /// program-date-time anchor가 없는 discontinuity run
    MissingTimeMapping,
    /// 누적 값과 어긋나는 명시적 anchor, 또는 겹치는 run
    ConflictingTimeMapping,
    /// 시간 의미를 구현하지 않은 태그
    UnsupportedTimingFeature,
    /// 표현 범위를 벗어난 누적 시간
    Overflow,
    /// 이 바인딩 버전이 모르는 core 오류
    Unknown,
}
/// [`hls::TrackerError`]와 같은 variant 목록, 모르는 값은 `Unknown`
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum TrackerErrorCode {
    /// 추적 중인 sequence에 다른 미디어를 실은 segment
    ConflictingSegment,
    /// 시간 의미를 구현하지 않은 태그가 있는 snapshot
    UnsupportedTimingFeature,
    /// 이 바인딩 버전이 모르는 core 오류
    Unknown,
}
/// 문서의 playlist 종류
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum PlaylistKind {
    /// variant stream 목록, segment 없음
    Multivariant,
    /// media segment 목록
    Media,
}
/// `EXT-X-PLAYLIST-TYPE` 값
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum PlaylistType {
    /// segment가 뒤에만 추가되는 playlist
    Event,
    /// 변하지 않는 playlist
    Vod,
}
/// 원문 줄의 문법 분류
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum RawLineKind {
    /// `#EXTM3U` 줄
    Header,
    /// `#EXT`로 시작하는 줄
    Tag,
    /// 태그가 아닌 `#` 줄
    Comment,
    /// segment 또는 variant URI
    Uri,
    /// 빈 줄
    Blank,
}

/// 자원 제한, `max_diagnostics` 외의 필드에서 0은 거절
#[derive(Debug, Clone, uniffi::Record)]
pub struct ParseLimits {
    /// playlist 최대 크기(UTF-8 바이트)
    pub max_input_bytes: u64,
    /// 한 줄 최대 길이(바이트)
    pub max_line_bytes: u64,
    /// 최대 줄 수
    pub max_lines: u64,
    /// segment, variant, rendition 각각의 최대 개수
    pub max_segments: u64,
    /// 태그당 최대 attribute 수, 서로 다른 key format의 최대 수와 동일
    pub max_attributes: u64,
    /// 보관할 diagnostic 수, 나머지는 개수만 셈
    pub max_diagnostics: u64,
}
/// 원문에서의 위치, 바이트 범위 `[start, end)`와 줄 번호
/// 예: 세 번째 줄 `#EXTINF:5.5,`는 line 3, start는 앞 두 줄과 개행의 바이트 수
#[derive(Debug, Clone, uniffi::Record)]
pub struct SpanDto {
    /// 시작 바이트 오프셋, 이 바이트 포함
    pub start: u64,
    /// 종료 바이트 오프셋, 이 바이트 제외
    pub end: u64,
    /// 사람이 읽는 줄 번호, 1부터
    pub line: u64,
}
impl From<hls::SourceSpan> for SpanDto {
    fn from(span: hls::SourceSpan) -> Self {
        Self {
            start: span.start as u64,
            end: span.end as u64,
            line: span.line as u64,
        }
    }
}
/// 텍스트를 포함한 원문 줄 하나
#[derive(Debug, Clone, uniffi::Record)]
pub struct RawLineDto {
    /// 문법 분류
    pub kind: RawLineKind,
    /// 줄 종결자를 제외한 위치
    pub span: SpanDto,
    /// 줄 종결자를 제외한 텍스트
    pub text: String,
}
/// 의미 해석 없이 보존한 태그
#[derive(Debug, Clone, uniffi::Record)]
pub struct DiagnosticDto {
    /// 원문에 쓰인 태그 이름
    pub tag: String,
    /// 태그 줄의 위치
    pub span: SpanDto,
    /// 사람이 읽는 상세 설명
    pub message: String,
}
/// 내용 복사 없이 문서를 설명하는 개수 정보
#[derive(Debug, Clone, uniffi::Record)]
pub struct DocumentSummary {
    /// 파싱한 playlist 종류
    pub kind: PlaylistKind,
    /// Media playlist의 segment 수, 그 외에는 0
    pub segment_count: u64,
    /// Multivariant playlist의 variant 수, 그 외에는 0
    pub variant_count: u64,
    /// 보관한 diagnostic 수
    pub diagnostic_count: u64,
    /// 제한에 걸려 버린 diagnostic 수
    pub omitted_diagnostics: u64,
}
/// attribute list의 `NAME=value` 쌍 하나
#[derive(Debug, Clone, uniffi::Record)]
pub struct AttributeDto {
    /// 대문자 attribute 이름
    pub name: String,
    /// 따옴표를 뺀 값
    pub value: String,
    /// 원문에서 값이 따옴표로 감싸였는지 여부
    pub quoted: bool,
}
pub(crate) fn attributes(values: &[hls::Attribute]) -> Vec<AttributeDto> {
    values
        .iter()
        .map(|a| AttributeDto {
            name: a.name.clone(),
            value: a.value.clone(),
            quoted: a.quoted,
        })
        .collect()
}
/// `EXT-X-STREAM-INF` 항목과 그 URI
#[derive(Debug, Clone, uniffi::Record)]
pub struct VariantDto {
    /// 원문에 쓰인 URI 줄
    pub uri: String,
    /// 검증된 `BANDWIDTH` attribute
    pub bandwidth: u64,
    /// 모든 attribute
    pub attributes: Vec<AttributeDto>,
}
/// `EXT-X-MEDIA` 항목
#[derive(Debug, Clone, uniffi::Record)]
pub struct RenditionDto {
    /// 모든 attribute
    pub attributes: Vec<AttributeDto>,
}
/// Media playlist의 playlist 수준 태그
#[derive(Debug, Clone, uniffi::Record)]
pub struct MediaMetadataDto {
    /// `EXT-X-TARGETDURATION`(정수 초)
    pub target_duration_seconds: u64,
    /// `EXT-X-MEDIA-SEQUENCE`, 없으면 0
    pub media_sequence: u64,
    /// `EXT-X-DISCONTINUITY-SEQUENCE`, 없으면 0
    pub discontinuity_sequence: u64,
    /// `EXT-X-ENDLIST` 존재 여부
    pub end_list: bool,
    /// `EXT-X-PLAYLIST-TYPE`, 있을 때만
    pub playlist_type: Option<PlaylistType>,
    /// `EXT-X-VERSION`, 있을 때만
    pub version: Option<u64>,
    /// `EXT-X-I-FRAMES-ONLY` 존재 여부, 모든 segment가 keyframe 하나
    pub i_frames_only: bool,
}
impl From<&hls::MediaMetadata> for MediaMetadataDto {
    fn from(m: &hls::MediaMetadata) -> Self {
        Self {
            target_duration_seconds: m.target_duration_seconds(),
            media_sequence: m.media_sequence(),
            discontinuity_sequence: m.discontinuity_sequence(),
            end_list: m.end_list(),
            version: m.version(),
            i_frames_only: m.i_frames_only(),
            playlist_type: m.playlist_type().map(|p| match p {
                hls::PlaylistType::Event => PlaylistType::Event,
                hls::PlaylistType::Vod => PlaylistType::Vod,
            }),
        }
    }
}
/// 해석이 끝난 byte range
#[derive(Debug, Clone, uniffi::Record)]
pub struct ByteRangeDto {
    /// 첫 바이트
    pub offset: u64,
    /// 바이트 수, 항상 양수
    pub length: u64,
}
impl From<hls::ByteRange> for ByteRangeDto {
    fn from(range: hls::ByteRange) -> Self {
        Self {
            offset: range.offset,
            length: range.length,
        }
    }
}
/// `EXT-X-KEY`의 `METHOD` attribute
#[derive(Debug, Clone, uniffi::Enum)]
pub enum EncryptionMethod {
    /// `AES-128`
    Aes128,
    /// `SAMPLE-AES`
    SampleAes,
    /// 그 외 method, 원문 그대로 보존
    Other {
        /// 원문에 쓰인 method 이름
        name: String,
    },
}
/// method가 `NONE`이 아닌 `EXT-X-KEY` 하나
#[derive(Debug, Clone, uniffi::Record)]
pub struct EncryptionKeyDto {
    /// 암호화 method
    pub method: EncryptionMethod,
    /// 원문에 쓰인 key URI
    pub uri: String,
    /// 원문에 쓰인 `IV` attribute
    pub iv: Option<String>,
    /// `KEYFORMAT`, 기본값 `identity`
    pub key_format: String,
    /// 원문에 쓰인 `KEYFORMATVERSIONS`
    pub key_format_versions: Option<String>,
}
impl From<&hls::EncryptionKey> for EncryptionKeyDto {
    fn from(k: &hls::EncryptionKey) -> Self {
        Self {
            method: match &k.method {
                hls::EncryptionMethod::Aes128 => EncryptionMethod::Aes128,
                hls::EncryptionMethod::SampleAes => EncryptionMethod::SampleAes,
                hls::EncryptionMethod::Other(name) => {
                    EncryptionMethod::Other { name: name.clone() }
                }
            },
            uri: k.uri.clone(),
            iv: k.iv.clone(),
            key_format: k.key_format.clone(),
            key_format_versions: k.key_format_versions.clone(),
        }
    }
}
/// `EXT-X-MAP` initialization section
#[derive(Debug, Clone, uniffi::Record)]
pub struct InitializationMapDto {
    /// 원문에 쓰인 map URI
    pub uri: String,
    /// `BYTERANGE` attribute, 있을 때만
    pub byte_range: Option<ByteRangeDto>,
    /// `MAP` 선언 시점의 key 목록
    pub keys: Vec<EncryptionKeyDto>,
}
/// 적용된 태그 상태를 포함한 media segment 하나
#[derive(Debug, Clone, uniffi::Record)]
pub struct SegmentDto {
    /// 원문에 쓰인 URI 줄
    pub uri: String,
    /// `EXTINF` 길이(나노초), 항상 양수
    pub duration_nanos: u64,
    /// `EXTINF` 제목, 빈 문자열 가능
    pub title: String,
    /// media sequence 번호
    pub media_sequence: u64,
    /// discontinuity sequence 번호
    pub discontinuity_sequence: u64,
    /// 이 segment 앞의 `EXT-X-DISCONTINUITY` 존재 여부
    pub discontinuity: bool,
    /// 명시적 `EXT-X-PROGRAM-DATE-TIME`의 Unix 나노초, 추론 값 제외
    pub declared_program_time_unix_nanos: Option<i64>,
    /// 적용 중인 `EXT-X-BITRATE`(kbps), 선언했을 때만
    pub bitrate_kbps: Option<u64>,
    /// 이 segment 앞의 `EXT-X-GAP` 존재 여부
    pub gap: bool,
    /// 해석이 끝난 `EXT-X-BYTERANGE`, 있을 때만
    pub byte_range: Option<ByteRangeDto>,
    /// 적용 중인 key 목록, key format당 하나, 비어 있으면 암호화 없음
    pub keys: Vec<EncryptionKeyDto>,
    /// 적용 중인 `EXT-X-MAP`
    pub map: Option<InitializationMapDto>,
    /// `EXTINF` 줄부터 URI 줄까지
    pub source_span: SpanDto,
}
impl From<&hls::Segment> for SegmentDto {
    fn from(s: &hls::Segment) -> Self {
        Self {
            uri: s.uri().into(),
            duration_nanos: s.duration().as_nanos(),
            title: s.title().into(),
            media_sequence: s.media_sequence(),
            discontinuity_sequence: s.discontinuity_sequence(),
            discontinuity: s.discontinuity(),
            declared_program_time_unix_nanos: s
                .declared_program_time()
                .map(hls::UtcTimestamp::as_unix_nanos),
            bitrate_kbps: s.bitrate_kbps(),
            gap: s.gap(),
            byte_range: s.byte_range().map(Into::into),
            source_span: s.source_span().into(),
            keys: s.effective_keys().map(Into::into).collect(),
            map: s.effective_map().map(|m| InitializationMapDto {
                uri: m.uri.clone(),
                byte_range: m.byte_range.map(Into::into),
                keys: m.effective_keys().map(Into::into).collect(),
            }),
        }
    }
}
/// playlist 전체 복사본, 명시적인 export 메서드만 생성
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PlaylistDto {
    /// Multivariant playlist
    Multivariant {
        /// 원문 순서의 `EXT-X-STREAM-INF` 항목
        variants: Vec<VariantDto>,
        /// 원문 순서의 `EXT-X-MEDIA` 항목
        renditions: Vec<RenditionDto>,
    },
    /// Media playlist
    Media {
        /// playlist 수준 태그
        metadata: MediaMetadataDto,
        /// 원문 순서의 segment
        segments: Vec<SegmentDto>,
    },
}
/// snapshot 하나 안의 위치, `snapshot_token`을 발급한 문서에서만 유효
#[derive(Debug, Clone, uniffi::Record)]
pub struct SeekTargetDto {
    /// 이 target이 속한 snapshot의 식별자
    pub snapshot_token: u64,
    /// 0부터 세는 segment 인덱스
    pub segment_index: u64,
    /// 해당 segment의 media sequence 번호
    pub media_sequence: u64,
    /// segment 시작부터의 시간(나노초)
    pub offset_nanos: u64,
}
/// timeline 조회 결과, 모든 구간은 `[start, end)`라 end 시각은 다음 segment 소속
#[derive(Debug, Clone, uniffi::Enum)]
pub enum LocateResult {
    /// 재생 가능한 segment 안의 시각
    Found {
        /// seek할 위치
        target: SeekTargetDto,
    },
    /// 재생할 것이 없는 시각
    Gap {
        /// `Some(index)`는 명시적 `EXT-X-GAP`, `None`은 run 사이의 매핑되지 않은 구간
        segment_index: Option<u64>,
    },
    /// 첫 segment보다 이른 시각
    BeforeWindow,
    /// 마지막 segment의 끝 또는 그 이후 시각
    AfterWindow,
}

/// tracker의 제한과 정책
#[derive(Debug, Clone, uniffi::Record)]
pub struct TrackerOptions {
    /// 이 개수를 넘으면 가장 오래된 segment부터 퇴출, 양수 필수
    pub max_segments: u64,
    /// 선언된 program time과 `EXTINF` 누적 사이의 허용 차이
    /// 기본값 0, 실제 스트림에서 어긋남을 재기 전에는 엄격하게 두는 편이 안전
    pub pdt_tolerance_nanos: u64,
}
/// 갱신 한 번이 바꾼 내용
#[derive(Debug, Clone, uniffi::Record)]
pub struct TrackerUpdate {
    /// 추가하거나 채운 segment 수
    pub added: u64,
    /// 제한을 지키려고 버린 가장 오래된 segment 수
    pub evicted: u64,
    /// 새 내용이 없는 갱신인지 여부
    pub stale: bool,
    /// 이력에 UTC 인덱스가 없는 이유, 있으면 `None`
    pub timing_error: Option<TimelineErrorCode>,
}
/// 재생 순서에서 다른 segment 다음에 오는 segment
#[derive(Debug, Clone, uniffi::Record)]
pub struct NextSegmentDto {
    /// 다음 segment
    pub segment: SegmentDto,
    /// `false`면 decoder가 그 앞에서 timestamp 점프를 예상해야 함
    pub contiguous: bool,
}
/// tracker가 덮는 UTC 구간, 첫 시작부터 마지막 종료(exclusive)까지
#[derive(Debug, Clone, uniffi::Record)]
pub struct TimeWindowDto {
    /// 가장 오래된 추적 segment의 시작
    pub start_unix_nanos: i64,
    /// 가장 새로운 추적 segment의 종료(exclusive)
    pub end_unix_nanos: i64,
}
impl From<hls::TimelineError> for TimelineErrorCode {
    fn from(error: hls::TimelineError) -> Self {
        match error {
            hls::TimelineError::EmptySnapshot => Self::EmptySnapshot,
            hls::TimelineError::MissingTimeMapping { .. } => Self::MissingTimeMapping,
            hls::TimelineError::ConflictingTimeMapping { .. } => Self::ConflictingTimeMapping,
            hls::TimelineError::UnsupportedTimingFeature { .. } => Self::UnsupportedTimingFeature,
            hls::TimelineError::Overflow { .. } => Self::Overflow,
            _ => Self::Unknown,
        }
    }
}
