//! `regex_replace` for the multi-rename dialog. Patterns are untrusted input and run on the
//! Rust `regex` engine only: linear time (no backtracking), no back-references and no
//! look-around, with limits for pattern, names and compiled size. Never runs in the UI thread.

use crate::convert::UiError;
use regex::RegexBuilder;
use serde::Serialize;

/// Pattern and replacement length in characters.
pub const MAX_PATTERN_CHARS: usize = 1000;
/// Longest file name Linux allows.
pub const MAX_NAME_BYTES: usize = 255;
pub const MAX_NAMES: usize = 10_000;
/// A replacement can multiply the name; results beyond this are reported instead of kept.
pub const MAX_RESULT_BYTES: usize = 4096;
/// Limits for the compiled program and the lazy DFA cache.
const SIZE_LIMIT: usize = 1 << 20;
const DFA_SIZE_LIMIT: usize = 1 << 20;
const NEST_LIMIT: u32 = 100;

/// `{ kind: "ok", name }` or `{ kind: "error", message }`, one per input name, same order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RegexResultDto {
    Ok { name: String },
    Error { message: String },
}

fn invalid_pattern(message: impl Into<String>) -> UiError {
    UiError::new("invalidPattern", message)
}

/// Replaces the first (or every) match of `pattern` in each name. Replacement syntax is that of
/// the `regex` crate: `$1`, `${name}`, `$$` for a literal dollar. A pattern that cannot be used
/// fails the whole call; a bad single name only that entry.
pub fn regex_replace(
    names: &[String],
    pattern: &str,
    replacement: &str,
    ignore_case: bool,
    all: bool,
) -> Result<Vec<RegexResultDto>, UiError> {
    if names.len() > MAX_NAMES {
        return Err(UiError::new(
            "invalidInput",
            format!("Zu viele Namen (höchstens {MAX_NAMES})"),
        ));
    }
    if pattern.is_empty() {
        return Err(invalid_pattern("Das Suchmuster ist leer"));
    }
    if pattern.chars().count() > MAX_PATTERN_CHARS {
        return Err(invalid_pattern(format!(
            "Das Suchmuster ist zu lang (höchstens {MAX_PATTERN_CHARS} Zeichen)"
        )));
    }
    if replacement.chars().count() > MAX_PATTERN_CHARS {
        return Err(invalid_pattern(format!(
            "Der Ersetzungstext ist zu lang (höchstens {MAX_PATTERN_CHARS} Zeichen)"
        )));
    }
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(SIZE_LIMIT)
        .dfa_size_limit(DFA_SIZE_LIMIT)
        .nest_limit(NEST_LIMIT)
        .build()
        .map_err(|error| match error {
            regex::Error::CompiledTooBig(_) => {
                invalid_pattern("Der reguläre Ausdruck ist zu groß oder zu verschachtelt")
            }
            regex::Error::Syntax(detail) => {
                // The crate's message is English; its last line names the actual problem.
                let reason = detail.lines().last().unwrap_or("").trim();
                invalid_pattern(format!(
                    "Ungültiger regulärer Ausdruck (Rückverweise und Lookarounds gibt es nicht): {reason}"
                ))
            }
            _ => invalid_pattern("Der reguläre Ausdruck kann nicht verwendet werden"),
        })?;
    Ok(names
        .iter()
        .map(|name| {
            if name.len() > MAX_NAME_BYTES {
                return RegexResultDto::Error {
                    message: format!("Der Name ist zu lang (höchstens {MAX_NAME_BYTES} Bytes)"),
                };
            }
            let result = if all {
                regex.replace_all(name, replacement)
            } else {
                regex.replace(name, replacement)
            };
            if result.len() > MAX_RESULT_BYTES {
                return RegexResultDto::Error {
                    message: "Das Ergebnis der Ersetzung ist zu lang".to_owned(),
                };
            }
            RegexResultDto::Ok {
                name: result.into_owned(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn ok(name: &str) -> RegexResultDto {
        RegexResultDto::Ok { name: name.into() }
    }

    #[test]
    fn replaces_with_numbered_and_named_groups() {
        let out = regex_replace(
            &names(&["IMG_2024_05.jpg", "x"]),
            r"IMG_(\d+)_(?P<m>\d+)",
            "$2-${1}",
            false,
            true,
        )
        .unwrap();
        assert_eq!(out, vec![ok("05-2024.jpg"), ok("x")]);
        let dollar = regex_replace(&names(&["a"]), "a", "$$", false, true).unwrap();
        assert_eq!(dollar, vec![ok("$")]);
    }

    #[test]
    fn respects_the_case_flag() {
        let list = names(&["Foto.JPG"]);
        let sensitive = regex_replace(&list, "jpg", "png", false, true).unwrap();
        assert_eq!(sensitive, vec![ok("Foto.JPG")]);
        let insensitive = regex_replace(&list, "jpg", "png", true, true).unwrap();
        assert_eq!(insensitive, vec![ok("Foto.png")]);
    }

    #[test]
    fn replaces_only_the_first_or_all_matches() {
        let list = names(&["a-a-a"]);
        assert_eq!(
            regex_replace(&list, "a", "b", false, false).unwrap(),
            vec![ok("b-a-a")]
        );
        assert_eq!(
            regex_replace(&list, "a", "b", false, true).unwrap(),
            vec![ok("b-b-b")]
        );
    }

    #[test]
    fn an_invalid_pattern_fails_the_call_with_a_german_message() {
        for bad in ["(", "[a-", "*a", r"(a)\1", "(?=a)", "(?<!a)b"] {
            let error = regex_replace(&names(&["a"]), bad, "", false, true).unwrap_err();
            assert_eq!(error.kind, "invalidPattern", "{bad}");
            assert!(
                error.message.starts_with("Ungültiger"),
                "{bad}: {}",
                error.message
            );
        }
    }

    #[test]
    fn empty_and_oversized_patterns_are_refused() {
        let empty = regex_replace(&names(&["a"]), "", "x", false, true).unwrap_err();
        assert_eq!(empty.kind, "invalidPattern");
        let long = "a".repeat(MAX_PATTERN_CHARS + 1);
        let error = regex_replace(&names(&["a"]), &long, "", false, true).unwrap_err();
        assert_eq!(error.kind, "invalidPattern");
        assert!(error.message.contains("zu lang"));
        let exact = "a".repeat(MAX_PATTERN_CHARS);
        assert!(regex_replace(&names(&["a"]), &exact, "", false, true).is_ok());
        let long_replacement = "a".repeat(MAX_PATTERN_CHARS + 1);
        assert!(regex_replace(&names(&["a"]), "a", &long_replacement, false, true).is_err());
    }

    #[test]
    fn a_pattern_that_compiles_too_big_is_refused() {
        let error = regex_replace(&names(&["a"]), r"\pL{1000}", "", false, true).unwrap_err();
        assert_eq!(error.kind, "invalidPattern");
        assert!(error.message.contains("zu groß"), "{}", error.message);
    }

    #[test]
    fn catastrophic_backtracking_patterns_finish_quickly() {
        let long = format!("{}b", "a".repeat(MAX_NAME_BYTES - 1));
        let list = vec![long; 200];
        let start = Instant::now();
        let out = regex_replace(&list, "(a+)+$", "x", false, true).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "took {:?}",
            start.elapsed()
        );
        assert_eq!(out.len(), 200);
        assert!(matches!(&out[0], RegexResultDto::Ok { .. }));
    }

    #[test]
    fn name_count_and_length_limits() {
        let many = vec!["a".to_owned(); MAX_NAMES + 1];
        assert_eq!(
            regex_replace(&many, "a", "b", false, true)
                .unwrap_err()
                .kind,
            "invalidInput"
        );
        let exact = vec!["a".to_owned(); MAX_NAMES];
        assert_eq!(
            regex_replace(&exact, "a", "b", false, true).unwrap().len(),
            MAX_NAMES
        );
        let out = regex_replace(
            &[
                "a".repeat(MAX_NAME_BYTES),
                "a".repeat(MAX_NAME_BYTES + 1),
                "ä".repeat(MAX_NAME_BYTES / 2 + 1),
            ],
            "a",
            "b",
            false,
            true,
        )
        .unwrap();
        assert!(matches!(out[0], RegexResultDto::Ok { .. }));
        assert!(matches!(out[1], RegexResultDto::Error { .. }));
        assert!(
            matches!(out[2], RegexResultDto::Error { .. }),
            "bytes, not characters"
        );
    }

    #[test]
    fn a_result_that_explodes_is_reported_for_that_name_only() {
        let long = "a".repeat(200);
        let out = regex_replace(
            &[long, "a".to_owned()],
            "a",
            &"y".repeat(MAX_PATTERN_CHARS),
            false,
            true,
        )
        .unwrap();
        assert!(matches!(out[0], RegexResultDto::Error { .. }));
        assert!(matches!(out[1], RegexResultDto::Ok { .. }));
    }

    #[test]
    fn serializes_to_the_ui_shape() {
        let value = serde_json::to_value(vec![
            ok("a"),
            RegexResultDto::Error {
                message: "m".into(),
            },
        ])
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!([{"kind":"ok","name":"a"},{"kind":"error","message":"m"}])
        );
    }
}
