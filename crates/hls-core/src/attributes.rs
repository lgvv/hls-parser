//! RFC 8216 attribute list 문법: `NAME=value,NAME="quoted value"`

use crate::{Attribute, ErrorCode, ParseError, SourceSpan};

pub(crate) fn parse_attributes(
    input: &str,
    span: SourceSpan,
    limit: usize,
) -> Result<Vec<Attribute>, ParseError> {
    let err = |message| ParseError::new(ErrorCode::InvalidSyntax, Some(span), message);
    let mut result: Vec<Attribute> = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        if result.len() >= limit {
            return Err(ParseError::new(
                ErrorCode::InputLimit,
                Some(span),
                "too many attributes",
            ));
        }
        let (name, tail) = rest
            .split_once('=')
            .ok_or_else(|| err("attribute needs '='"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(err("invalid attribute name"));
        }
        if result.iter().any(|a| a.name == name) {
            return Err(err("duplicate attribute name"));
        }
        let (value, remaining, quoted) = if let Some(tail) = tail.strip_prefix('"') {
            let end = tail
                .find('"')
                .ok_or_else(|| err("unterminated quoted attribute"))?;
            (&tail[..end], &tail[end + 1..], true)
        } else {
            let end = tail.find(',').unwrap_or(tail.len());
            let value = &tail[..end];
            if value.is_empty() || value.bytes().any(|b| b.is_ascii_whitespace() || b == b'"') {
                return Err(err("invalid unquoted attribute"));
            }
            (value, &tail[end..], false)
        };
        result.push(Attribute {
            name: name.into(),
            value: value.into(),
            quoted,
        });
        rest = if remaining.is_empty() {
            ""
        } else {
            // 쉼표 뒤 공백은 RFC 8216 위반이지만 오래된 packager가 흔히 출력
            let tail = remaining
                .strip_prefix(',')
                .ok_or_else(|| err("expected comma after attribute"))?
                .trim_start_matches(' ');
            if tail.is_empty() {
                return Err(err("trailing attribute comma"));
            }
            tail
        };
    }
    if result.is_empty() {
        return Err(err("empty attribute list"));
    }
    Ok(result)
}

pub(crate) fn attr<'a>(attributes: &'a [Attribute], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|a| a.name == name)
        .map(|a| a.value.as_str())
}

pub(crate) fn required<'a>(
    attributes: &'a [Attribute],
    name: &str,
    quoted: bool,
    span: SourceSpan,
) -> Result<&'a str, ParseError> {
    let a = attributes.iter().find(|a| a.name == name).ok_or_else(|| {
        ParseError::new(
            ErrorCode::MissingTag,
            Some(span),
            format!("missing {name} attribute"),
        )
    })?;
    if a.quoted != quoted || a.value.is_empty() {
        return Err(ParseError::new(
            ErrorCode::InvalidValue,
            Some(span),
            format!("invalid {name} attribute"),
        ));
    }
    Ok(&a.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPAN: SourceSpan = SourceSpan {
        start: 0,
        end: 0,
        line: 1,
    };

    fn parse(input: &str) -> Result<Vec<(String, String, bool)>, ErrorCode> {
        parse_attributes(input, SPAN, 8)
            .map(|list| {
                list.into_iter()
                    .map(|a| (a.name, a.value, a.quoted))
                    .collect()
            })
            .map_err(|e| e.code)
    }

    #[test]
    fn quoted_values_keep_commas_and_unquoted_values_stop_at_commas() {
        let list = parse("A=1,B=\"x,y\",C-2=z").unwrap();
        assert_eq!(list[0], ("A".into(), "1".into(), false));
        assert_eq!(list[1], ("B".into(), "x,y".into(), true));
        assert_eq!(list[2], ("C-2".into(), "z".into(), false));
        assert_eq!(parse("A=\"\"").unwrap()[0].1, "");
    }

    #[test]
    fn grammar_violations_are_syntax_errors() {
        for bad in [
            "", "A", "=1", "a=1", "A =1", "A=", "A=1,", "A=1,,B=2", "A=\"x", "A=\"x\"y", "A=x\"y",
            "A=1 2", "A=1,A=2",
        ] {
            assert_eq!(parse(bad), Err(ErrorCode::InvalidSyntax), "{bad:?}");
        }
        assert_eq!(
            parse("A=1,B=2,C=3,D=4,E=5,F=6,G=7,H=8,I=9"),
            Err(ErrorCode::InputLimit)
        );
    }

    #[test]
    fn required_checks_presence_quoting_and_emptiness() {
        let list = parse_attributes("URI=\"k\",METHOD=AES-128,EMPTY=\"\"", SPAN, 8).unwrap();
        assert_eq!(attr(&list, "METHOD"), Some("AES-128"));
        assert_eq!(attr(&list, "NONE"), None);
        assert_eq!(required(&list, "URI", true, SPAN).unwrap(), "k");
        assert_eq!(
            required(&list, "URI", false, SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(
            required(&list, "EMPTY", true, SPAN).unwrap_err().code,
            ErrorCode::InvalidValue
        );
        assert_eq!(
            required(&list, "IV", false, SPAN).unwrap_err().code,
            ErrorCode::MissingTag
        );
    }
}
