//! 정수 나노초 시간 타입
//! f64를 쓰면 `EXTINF:5.1` 같은 값이 정확히 표현되지 않고 누적할수록 오차가 커지므로 정수로 고정

use crate::{ErrorCode, ParseError};

/// 1초의 나노초 수
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// playlist snapshot 시작부터의 경과 시간, 음수 없음
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ElapsedTime(u64);

impl ElapsedTime {
    /// 나노초 값 감싸기
    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }
    /// 나노초 값
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

/// Unix epoch 기준 부호 있는 나노초, `i64` timestamp 범위
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcTimestamp(i64);

impl UtcTimestamp {
    /// Unix 나노초 값 감싸기
    pub const fn from_unix_nanos(value: i64) -> Self {
        Self(value)
    }
    /// Unix 나노초 값
    pub const fn as_unix_nanos(self) -> i64 {
        self.0
    }
    /// 시간대가 있는 RFC 3339 timestamp 파싱
    /// 소수 10자리 이상, 윤초, `i64` 나노초 범위 밖은 오류
    pub fn parse_rfc3339(value: &str) -> Result<Self, ParseError> {
        if let Some((_, rest)) = value.split_once('.') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 9 {
                return Err(ParseError::new(
                    ErrorCode::UnsupportedPrecision,
                    None,
                    "timestamp exceeds nanosecond precision",
                ));
            }
        }
        let date = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
            ParseError::new(
                ErrorCode::InvalidValue,
                None,
                "expected RFC 3339 timestamp with timezone",
            )
        })?;
        // chrono는 윤초를 허용하지만 Unix 나노초 값은 윤초를 보존하지 못함
        if u64::from(date.timestamp_subsec_nanos()) >= NANOS_PER_SECOND {
            return Err(ParseError::new(
                ErrorCode::InvalidValue,
                None,
                "leap seconds cannot be represented by this API",
            ));
        }
        date.timestamp_nanos_opt().map(Self).ok_or_else(|| {
            ParseError::new(
                ErrorCode::Overflow,
                None,
                "timestamp outside signed nanosecond range",
            )
        })
    }
}

/// `EXTINF`의 초 값을 나노초 정수로 바꾼 것, 소수 10자리 이상은 표현할 수 없어 오류
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentDuration(u64);

impl SegmentDuration {
    /// `5`, `5.5`, `5.000000001` 같은 `EXTINF` 초 파싱, 0은 거절
    pub fn parse_seconds(value: &str) -> Result<Self, ParseError> {
        let invalid = || {
            ParseError::new(
                ErrorCode::InvalidValue,
                None,
                "expected positive decimal seconds",
            )
        };
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || (value.contains('.') && fraction.is_empty())
        {
            return Err(invalid());
        }
        if fraction.len() > 9 {
            return Err(ParseError::new(
                ErrorCode::UnsupportedPrecision,
                None,
                "duration exceeds nanosecond precision",
            ));
        }
        let scale = u32::try_from(9 - fraction.len()).map_err(|_| invalid())?;
        let whole: u64 = whole
            .parse()
            .map_err(|_| ParseError::new(ErrorCode::Overflow, None, "duration overflow"))?;
        let fractional = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u64>().map_err(|_| invalid())? * 10u64.pow(scale)
        };
        let nanos = whole
            .checked_mul(NANOS_PER_SECOND)
            .and_then(|n| n.checked_add(fractional))
            .ok_or_else(|| ParseError::new(ErrorCode::Overflow, None, "duration overflow"))?;
        if nanos == 0 {
            return Err(invalid());
        }
        Ok(Self(nanos))
    }
    /// 나노초 길이, 항상 양수
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_scale_fractions_exactly() {
        let nanos = |s: &str| SegmentDuration::parse_seconds(s).map(SegmentDuration::as_nanos);
        assert_eq!(nanos("5").unwrap(), 5_000_000_000);
        assert_eq!(nanos("5.5").unwrap(), 5_500_000_000);
        assert_eq!(nanos("0.000000001").unwrap(), 1);
        assert_eq!(nanos("5.000000001").unwrap(), 5_000_000_001);
        assert_eq!(nanos("18446744073.709551615").unwrap(), u64::MAX);
        assert_eq!(
            nanos("0.0000000001").unwrap_err().code,
            ErrorCode::UnsupportedPrecision
        );
        assert_eq!(
            nanos("18446744073.709551616").unwrap_err().code,
            ErrorCode::Overflow
        );
        for bad in ["", "0", "0.0", ".5", "5.", "-1", "1e3", " 1", "1,5"] {
            assert_eq!(
                nanos(bad).unwrap_err().code,
                ErrorCode::InvalidValue,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn timestamps_require_timezone_and_reject_leap_seconds() {
        let parse = |s: &str| UtcTimestamp::parse_rfc3339(s).map(UtcTimestamp::as_unix_nanos);
        assert_eq!(parse("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(parse("1970-01-01T00:00:00.000000001Z").unwrap(), 1);
        assert_eq!(parse("1970-01-01T09:00:00+09:00").unwrap(), 0);
        assert_eq!(parse("1969-12-31T23:59:59Z").unwrap(), -1_000_000_000);
        assert_eq!(
            parse("1970-01-01T00:00:00.0000000001Z").unwrap_err().code,
            ErrorCode::UnsupportedPrecision
        );
        assert_eq!(
            parse("2016-12-31T23:59:60Z").unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(
            parse("2300-01-01T00:00:00Z").unwrap_err().code,
            ErrorCode::Overflow
        );
        for bad in ["1970-01-01T00:00:00", "1970-01-01", "now", ""] {
            assert_eq!(
                parse(bad).unwrap_err().code,
                ErrorCode::InvalidValue,
                "{bad:?}"
            );
        }
    }
}
