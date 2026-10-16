use super::model::FIELDS;
use crate::error::{ApiError, ApiResult};
use sqlx::{Postgres, QueryBuilder};
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Text(String),
    Open,
    Close,
    Comma,
}
fn tokens(s: &str) -> ApiResult<Vec<Token>> {
    if s.len() > 2048 {
        return Err(ApiError::bad("OData filter is too long"));
    }
    let mut chars = s.chars().peekable();
    let mut out = Vec::new();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '(' => out.push(Token::Open),
            ')' => out.push(Token::Close),
            ',' => out.push(Token::Comma),
            '\'' => {
                let mut v = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => {
                            if chars.peek() == Some(&'\'') {
                                chars.next();
                                v.push('\'');
                            } else {
                                break;
                            }
                        }
                        Some(c) => v.push(c),
                        None => return Err(ApiError::bad("Unterminated OData string")),
                    }
                }
                out.push(Token::Text(v));
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut v = c.to_string();
                while chars
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                {
                    v.push(chars.next().unwrap());
                }
                out.push(Token::Word(v));
            }
            _ => return Err(ApiError::bad("Unsupported OData filter character")),
        }
    }
    if out.len() > 200 {
        return Err(ApiError::bad("OData filter is too complex"));
    }
    Ok(out)
}
pub(super) fn field(name: &str) -> ApiResult<String> {
    let (_, ty) = FIELDS
        .iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| ApiError::bad("Unknown OData field"))?;
    let cast = match *ty {
        "Boolean" => "boolean",
        "Int64" => "bigint",
        "DateTime" => "timestamptz",
        _ => "text",
    };
    Ok(format!("(data->>'{name}')::{cast}"))
}
struct Filter {
    tokens: Vec<Token>,
    at: usize,
}
impl Filter {
    fn word(&mut self, w: &str) -> bool {
        if self.tokens.get(self.at) == Some(&Token::Word(w.into())) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: Token) -> ApiResult<()> {
        if self.tokens.get(self.at) == Some(&t) {
            self.at += 1;
            Ok(())
        } else {
            Err(ApiError::bad("Invalid OData filter syntax"))
        }
    }
    fn expr(&mut self, b: &mut QueryBuilder<'_, Postgres>, depth: usize) -> ApiResult<()> {
        if depth > 16 {
            return Err(ApiError::bad("OData filter nesting is too deep"));
        }
        self.and(b, depth + 1)?;
        while self.word("or") {
            b.push(" OR ");
            self.and(b, depth + 1)?;
        }
        Ok(())
    }
    fn and(&mut self, b: &mut QueryBuilder<'_, Postgres>, depth: usize) -> ApiResult<()> {
        self.compare(b, depth)?;
        while self.word("and") {
            b.push(" AND ");
            self.compare(b, depth)?;
        }
        Ok(())
    }
    fn compare(&mut self, b: &mut QueryBuilder<'_, Postgres>, depth: usize) -> ApiResult<()> {
        if depth > 16 {
            return Err(ApiError::bad("OData filter nesting is too deep"));
        }
        if self.word("not") {
            b.push(" NOT (");
            self.compare(b, depth + 1)?;
            b.push(")");
            return Ok(());
        }
        self.value(b, depth)?;
        for (op, sql) in [
            ("eq", " = "),
            ("ne", " <> "),
            ("gt", " > "),
            ("ge", " >= "),
            ("lt", " < "),
            ("le", " <= "),
        ] {
            if self.word(op) {
                b.push(sql);
                self.value(b, depth)?;
                break;
            }
        }
        Ok(())
    }
    fn value(&mut self, b: &mut QueryBuilder<'_, Postgres>, depth: usize) -> ApiResult<()> {
        if depth > 16 {
            return Err(ApiError::bad("OData filter nesting is too deep"));
        }
        let token = self
            .tokens
            .get(self.at)
            .cloned()
            .ok_or_else(|| ApiError::bad("Incomplete OData filter"))?;
        self.at += 1;
        match token {
            Token::Text(v) => {
                b.push_bind(v);
            }
            Token::Open => {
                b.push("(");
                self.expr(b, depth + 1)?;
                self.expect(Token::Close)?;
                b.push(")");
            }
            Token::Word(w) => match w.as_str() {
                "true" | "false" => {
                    b.push(w);
                }
                "tolower" => {
                    self.expect(Token::Open)?;
                    b.push("lower(");
                    self.value(b, depth + 1)?;
                    self.expect(Token::Close)?;
                    b.push(")");
                }
                "substringof" | "startswith" | "endswith" => {
                    self.expect(Token::Open)?;
                    b.push(match w.as_str() {
                        "substringof" => "(strpos(",
                        "startswith" => "starts_with(",
                        _ => "ends_with(",
                    });
                    // strpos takes the haystack first, unlike OData substringof.
                    if w == "substringof" {
                        let start = self.at;
                        let mut discard = QueryBuilder::<Postgres>::new("");
                        self.value(&mut discard, depth + 1)?;
                        let end = self.at;
                        self.expect(Token::Comma)?;
                        self.value(b, depth + 1)?;
                        b.push(",");
                        let saved = self.at;
                        self.at = start;
                        self.value(b, depth + 1)?;
                        if self.at != end {
                            return Err(ApiError::bad("Invalid function"));
                        }
                        self.at = saved;
                    } else {
                        self.value(b, depth + 1)?;
                        self.expect(Token::Comma)?;
                        b.push(",");
                        self.value(b, depth + 1)?;
                    }
                    self.expect(Token::Close)?;
                    b.push(if w == "substringof" { ") > 0)" } else { ")" });
                }
                "datetime" => {
                    let Some(Token::Text(v)) = self.tokens.get(self.at).cloned() else {
                        return Err(ApiError::bad("Invalid OData datetime"));
                    };
                    self.at += 1;
                    let value = chrono::DateTime::parse_from_rfc3339(&if v.ends_with('Z') {
                        v
                    } else {
                        format!("{v}Z")
                    })
                    .map_err(|_| ApiError::bad("Invalid OData datetime"))?;
                    b.push_bind(value.with_timezone(&chrono::Utc));
                }
                _ => {
                    if let Ok(v) = w.parse::<i64>() {
                        b.push_bind(v);
                    } else {
                        b.push(field(&w)?);
                    }
                }
            },
            _ => return Err(ApiError::bad("Invalid OData expression")),
        }
        Ok(())
    }
}

pub(super) fn append(builder: &mut QueryBuilder<'_, Postgres>, text: &str) -> ApiResult<()> {
    let mut parser = Filter {
        tokens: tokens(text)?,
        at: 0,
    };
    parser.expr(builder, 0)?;
    if parser.at != parser.tokens.len() {
        return Err(ApiError::bad("Unsupported OData expression"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_bind_literals_and_limit_recursion() {
        for s in [
            "Id eq 'O''Reilly'",
            "IsLatestVersion and substringof('json',tolower(Description))",
            "startswith(Id,'Json')",
            "endswith(Id,'Json')",
            "(DownloadCount ge 1) or not IsPrerelease",
            "Published gt datetime'2020-01-01T00:00:00Z'",
        ] {
            let mut b = QueryBuilder::new("");
            append(&mut b, s).unwrap();
            assert!(!b.sql().contains("Reilly"));
        }
        for s in [
            "Id eq 'unterminated",
            "Id; DROP TABLE packages",
            "Unknown eq 'x'",
        ] {
            assert!(append(&mut QueryBuilder::new(""), s).is_err(), "{s}");
        }
    }
}
