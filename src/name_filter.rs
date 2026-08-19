//! File-name filter used by the file list.
//!
//! Pattern language (matched against the full file name, extension included,
//! case-insensitively):
//!
//! - `*`   — any run of characters that does **not** cross a `_`
//! - `**`  — any run of characters, `_` included
//! - `{…}` — a condition on the token at that position:
//!   - numeric comparisons `<` `<=` `>` `>=` `=` `!=` against the run of digits
//!     there, `,`-separated for AND — `{>=20260101,<20260808}`
//!   - `|`-separated literal alternatives — `{EtOH|MeOH}`
//! - everything else is matched literally
//!
//! A pattern with no `*` and no `{` is treated as a plain substring search, so
//! typing `EtOH` just narrows the list without any pattern syntax.

#[derive(Debug, PartialEq)]
enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Debug, PartialEq)]
struct Cond {
    op: Op,
    value: f64,
}

impl Cond {
    fn holds(&self, v: f64) -> bool {
        match self.op {
            Op::Lt => v < self.value,
            Op::Le => v <= self.value,
            Op::Gt => v > self.value,
            Op::Ge => v >= self.value,
            Op::Eq => v == self.value,
            Op::Ne => v != self.value,
        }
    }
}

#[derive(Debug, PartialEq)]
enum Brace {
    /// `{<20260808}` — all conditions must hold for the digit run.
    Num(Vec<Cond>),
    /// `{EtOH|MeOH}` — one of the literal alternatives (lowercased).
    Alt(Vec<Vec<char>>),
}

#[derive(Debug, PartialEq)]
enum Token {
    /// Literal text, already lowercased.
    Literal(Vec<char>),
    /// `*` — no `_` allowed.
    Star,
    /// `**` — `_` allowed.
    DoubleStar,
    Brace(Brace),
}

#[derive(Debug, PartialEq)]
enum Kind {
    Substring(Vec<char>),
    Pattern(Vec<Token>),
}

#[derive(Debug, PartialEq)]
pub struct NameFilter {
    kind: Kind,
}

impl NameFilter {
    /// Parse a filter expression.
    ///
    /// Returns `Ok(None)` for a blank pattern (= no filtering) and `Err` with a
    /// user-facing message for a malformed one.
    pub fn parse(pattern: &str) -> Result<Option<NameFilter>, String> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Ok(None);
        }
        let kind = if pattern.contains('*') || pattern.contains('{') {
            Kind::Pattern(parse_tokens(pattern)?)
        } else {
            Kind::Substring(lower_chars(pattern))
        };
        Ok(Some(NameFilter { kind }))
    }

    /// Does `name` (a file name, extension included) pass the filter?
    pub fn matches(&self, name: &str) -> bool {
        let s = lower_chars(name);
        match &self.kind {
            Kind::Substring(needle) => s.windows(needle.len()).any(|w| w == &needle[..]),
            Kind::Pattern(tokens) => match_tokens(tokens, &s),
        }
    }
}

fn lower_chars(s: &str) -> Vec<char> {
    s.to_lowercase().chars().collect()
}

fn parse_tokens(pattern: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut literal = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                flush_literal(&mut literal, &mut tokens);
                if chars.get(i + 1) == Some(&'*') {
                    tokens.push(Token::DoubleStar);
                    i += 2;
                } else {
                    tokens.push(Token::Star);
                    i += 1;
                }
            }
            '{' => {
                flush_literal(&mut literal, &mut tokens);
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == '}')
                    .map(|p| i + p)
                    .ok_or_else(|| "unclosed '{' in filter".to_string())?;
                let body: String = chars[i + 1..end].iter().collect();
                tokens.push(Token::Brace(parse_brace(&body)?));
                i = end + 1;
            }
            '}' => return Err("stray '}' in filter".to_string()),
            c => {
                literal.extend(c.to_lowercase());
                i += 1;
            }
        }
    }
    flush_literal(&mut literal, &mut tokens);
    Ok(tokens)
}

fn flush_literal(literal: &mut Vec<char>, tokens: &mut Vec<Token>) {
    if !literal.is_empty() {
        tokens.push(Token::Literal(std::mem::take(literal)));
    }
}

fn parse_brace(body: &str) -> Result<Brace, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("empty {} in filter".to_string());
    }
    let starts_with_op = trimmed.starts_with(['<', '>', '=', '!']);
    if trimmed.contains('|') || !starts_with_op {
        let alts: Vec<Vec<char>> = trimmed.split('|').map(|a| lower_chars(a.trim())).collect();
        if alts.iter().any(|a| a.is_empty()) {
            return Err(format!("empty alternative in {{{trimmed}}}"));
        }
        return Ok(Brace::Alt(alts));
    }
    let conds = trimmed
        .split(',')
        .map(parse_cond)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Brace::Num(conds))
}

fn parse_cond(part: &str) -> Result<Cond, String> {
    let part = part.trim();
    let (op, rest) = if let Some(rest) = part.strip_prefix("<=") {
        (Op::Le, rest)
    } else if let Some(rest) = part.strip_prefix(">=") {
        (Op::Ge, rest)
    } else if let Some(rest) = part.strip_prefix("!=") {
        (Op::Ne, rest)
    } else if let Some(rest) = part.strip_prefix("==") {
        (Op::Eq, rest)
    } else if let Some(rest) = part.strip_prefix('<') {
        (Op::Lt, rest)
    } else if let Some(rest) = part.strip_prefix('>') {
        (Op::Gt, rest)
    } else if let Some(rest) = part.strip_prefix('=') {
        (Op::Eq, rest)
    } else {
        return Err(format!("missing comparison operator in '{part}'"));
    };
    let value = rest
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("'{}' is not a number", rest.trim()))?;
    Ok(Cond { op, value })
}

/// Backtracking matcher. File names are short, so the naive search is fine.
fn match_tokens(tokens: &[Token], s: &[char]) -> bool {
    let Some((token, rest)) = tokens.split_first() else {
        return s.is_empty();
    };
    match token {
        Token::Literal(lit) => s.starts_with(lit) && match_tokens(rest, &s[lit.len()..]),
        Token::Star => {
            let limit = s.iter().position(|&c| c == '_').unwrap_or(s.len());
            (0..=limit).any(|n| match_tokens(rest, &s[n..]))
        }
        Token::DoubleStar => (0..=s.len()).any(|n| match_tokens(rest, &s[n..])),
        Token::Brace(Brace::Alt(alts)) => alts
            .iter()
            .any(|alt| s.starts_with(alt) && match_tokens(rest, &s[alt.len()..])),
        Token::Brace(Brace::Num(conds)) => {
            // The whole run of digits is one token: `{<100}` must not match the
            // leading `2` of `20260808`.
            let digits = s.iter().position(|c| !c.is_ascii_digit()).unwrap_or(s.len());
            if digits == 0 {
                return false;
            }
            let text: String = s[..digits].iter().collect();
            let Ok(value) = text.parse::<f64>() else {
                return false;
            };
            conds.iter().all(|c| c.holds(value)) && match_tokens(rest, &s[digits..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pattern: &str, name: &str) -> bool {
        NameFilter::parse(pattern)
            .expect("pattern parses")
            .expect("pattern is not blank")
            .matches(name)
    }

    #[test]
    fn blank_pattern_is_no_filter() {
        assert_eq!(NameFilter::parse("").unwrap(), None);
        assert_eq!(NameFilter::parse("   ").unwrap(), None);
    }

    #[test]
    fn plain_text_is_a_substring_search() {
        assert!(m("EtOH", "20260101_a_b_c_EtOH10_scan.003"));
        assert!(m("etoh", "20260101_a_b_c_EtOH10_scan.003"));
        assert!(!m("MeOH", "20260101_a_b_c_EtOH10_scan.003"));
        assert!(m(".003", "20260101_a_b_c_EtOH10_scan.003"));
    }

    #[test]
    fn example_pattern() {
        let p = "{<20260808}_*_*_*_EtOH10_**";
        assert!(m(p, "20260101_a_b_c_EtOH10_scan.003"));
        // `**` spans the remaining underscores.
        assert!(m(p, "20260101_a_b_c_EtOH10_scan_retrace.003"));
        // Date is not below the bound.
        assert!(!m(p, "20261231_a_b_c_EtOH10_scan.003"));
        assert!(!m(p, "20260808_a_b_c_EtOH10_scan.003"));
        // One segment short.
        assert!(!m(p, "20260101_a_b_EtOH10_x.003"));
        // Different solvent.
        assert!(!m(p, "20260101_a_b_c_MeOH10_x.003"));
        // Case-insensitive literals.
        assert!(m(
            "{<20260808}_*_*_*_etoh10_**",
            "20260101_a_b_c_EtOH10_x.003"
        ));
    }

    #[test]
    fn star_does_not_cross_underscore() {
        assert!(m("a_*_c", "a_bb_c"));
        assert!(!m("a_*_c", "a_b_b_c"));
        assert!(m("a_*", "a_.003"));
        // `*` also matches the empty string.
        assert!(m("a_*_c", "a__c"));
    }

    #[test]
    fn double_star_crosses_underscore() {
        assert!(m("a_**_c", "a_b_b_c"));
        assert!(m("**", "anything_at_all.003"));
        assert!(m("**.003", "a_b.003"));
    }

    #[test]
    fn numeric_conditions() {
        assert!(m("{>=20260101,<20260808}_**", "20260501_x.003"));
        assert!(!m("{>=20260101,<20260808}_**", "20251231_x.003"));
        assert!(!m("{>=20260101,<20260808}_**", "20260808_x.003"));
        assert!(m("{<=5}_**", "5_x.003"));
        assert!(m("{>5}_**", "6_x.003"));
        assert!(!m("{>5}_**", "5_x.003"));
        assert!(m("{=5}_**", "5_x.003"));
        assert!(m("{!=5}_**", "6_x.003"));
        assert!(!m("{!=5}_**", "5_x.003"));
    }

    #[test]
    fn digit_run_is_taken_whole() {
        // `2` alone would satisfy `<100`; the whole run must be compared.
        assert!(!m("{<100}**", "20260808_x.003"));
        assert!(m("{<100}**", "42_x.003"));
    }

    #[test]
    fn numeric_condition_needs_digits() {
        assert!(!m("{<100}_**", "abc_x.003"));
    }

    #[test]
    fn alternatives() {
        assert!(m("**_{EtOH|MeOH}10_**", "20260101_a_EtOH10_x.003"));
        assert!(m("**_{EtOH|MeOH}10_**", "20260101_a_meoh10_x.003"));
        assert!(!m("**_{EtOH|MeOH}10_**", "20260101_a_IPA10_x.003"));
        // A brace without an operator or `|` is a plain literal.
        assert!(m("{20260808}_**", "20260808_x.003"));
        assert!(!m("{20260808}_**", "20260101_x.003"));
    }

    #[test]
    fn syntax_errors() {
        assert!(NameFilter::parse("{<20260808_*").is_err());
        assert!(NameFilter::parse("{}_*").is_err());
        assert!(NameFilter::parse("{<abc}_*").is_err());
        assert!(NameFilter::parse("a}_*").is_err());
        assert!(NameFilter::parse("{a|}_*").is_err());
    }
}
