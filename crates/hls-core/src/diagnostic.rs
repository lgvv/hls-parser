//! 파싱이 실패했을 때와 실패는 아니지만 알려 둘 것이 있을 때의 타입
//! 전자가 `ParseError`, 후자가 `Diagnostic`

use crate::SourceSpan;

/// 파싱 실패의 종류, 호출자는 message가 아니라 이 값으로 분기
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// `ParseOptions` 제한 초과
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
    /// 해석할 수 없는 `EXT-X-BYTERANGE` 또는 `MAP`의 `BYTERANGE`
    InvalidByteRange,
    /// 정수 연산 또는 변환의 오버플로
    Overflow,
}

/// 파싱 실패, 실패하면 부분 문서 없이 이 오류만 반환
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ParseError {
    /// 에러 분류
    pub code: ErrorCode,
    /// 실패를 감지한 위치, 입력에 위치가 있을 때만
    pub span: Option<SourceSpan>,
    /// 사람이 읽는 상세 설명, 안정적인 계약에서 제외
    pub message: String,
}

impl ParseError {
    pub(crate) fn new(
        code: ErrorCode,
        span: Option<SourceSpan>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            span,
            message: message.into(),
        }
    }

    /// 위치 없이 만든 오류(값 파서 등)에 위치 부여
    pub(crate) fn at(mut self, span: SourceSpan) -> Self {
        self.span = Some(span);
        self
    }
}

/// 파싱은 성공했지만 알려 둘 것, 예: 타입으로 만들지 않은 태그, 반복된 PDT
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 원문에 쓰인 태그 이름, `#EXT` 접두사 포함
    pub tag: String,
    /// 태그 줄의 위치
    pub span: SourceSpan,
    /// 사람이 읽는 상세 설명
    pub message: String,
}
