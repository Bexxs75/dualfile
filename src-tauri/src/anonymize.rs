//! Removes personal data from log text before the user copies or saves it. Pure functions only:
//! the caller supplies the facts to hide. File names stay as they are, because they are what
//! one needs to find an error.

use regex::{Captures, Regex};
use std::collections::HashMap;
use std::net::Ipv6Addr;

/// What counts as personal on this machine.
#[derive(Clone, Debug, Default)]
pub struct AnonContext {
    pub home: String,
    pub user: String,
    pub hostname: String,
    /// Hosts of the saved connections.
    pub hosts: Vec<String>,
}

/// Names shorter than this would wreck ordinary words, so they are left alone.
const MIN_NAME_LEN: usize = 3;
/// Words the replacements themselves are made of; a user of that name would be rewritten twice.
const PLACEHOLDER_WORDS: [&str; 4] = ["benutzer", "rechner", "email", "server"];

const URL: &str = r#"(?P<scheme>smb|sftp|ftps?|nfs|system|davs?|webdavs?|https?)://(?:(?P<ui>[^\s/@<>"']+)@)?(?P<uh>\[[0-9a-f:.]+\]|[^\s/@:?#<>"'()\[\]]+)"#;
const EMAIL: &str = r"(?P<email>[a-z0-9._%+\-]+@[a-z0-9\-]+(?:\.[a-z0-9\-]+)+)";
// Only forms with `::` or all eight groups: a bare `12:30:45` must stay a time.
const V6: &str = r"(?P<v6>[0-9a-f:]*::[0-9a-f:.]*|(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4})";
const V4: &str = r"(?P<v4>\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b)";

pub fn anonymize(text: &str, ctx: &AnonContext) -> String {
    let text = replace_home(text, &ctx.home);
    let text = replace_endpoints(&text, &ctx.hosts);
    let text = replace_word(&text, &ctx.user, "<benutzer>", false);
    replace_word(&text, &ctx.hostname, "<rechner>", true)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The text before `start` ends a word: no letter, digit, `_`, `-` or `.` right in front
/// (`.max` is a file ending, not the user).
fn boundary_before(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word(c) && c != '-' && c != '.')
}

/// The text from `end` on starts a new word. A dot only ends a word at the end of a sentence,
/// not in `max.txt`.
fn boundary_after(text: &str, end: usize) -> bool {
    let mut rest = text[end..].chars();
    match rest.next() {
        None => true,
        Some('.') => rest.next().is_none_or(|c| !is_word(c)),
        Some(c) => !is_word(c) && c != '-',
    }
}

fn replace_home(text: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if home.len() < 2 {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, _) in text.match_indices(home) {
        let end = start + home.len();
        let before_ok = start >= last
            && text[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !is_word(c) && c != '/');
        if before_ok && boundary_after(text, end) {
            out.push_str(&text[last..start]);
            out.push('~');
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn replace_word(text: &str, word: &str, replacement: &str, ignore_case: bool) -> String {
    if word.chars().count() < MIN_NAME_LEN
        || PLACEHOLDER_WORDS.contains(&word.to_lowercase().as_str())
    {
        return text.to_owned();
    }
    // ASCII lowering keeps every byte offset, so matches map back onto the original.
    let (haystack, needle) = if ignore_case {
        (text.to_ascii_lowercase(), word.to_ascii_lowercase())
    } else {
        (text.to_owned(), word.to_owned())
    };
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (start, _) in haystack.match_indices(&needle) {
        let end = start + needle.len();
        if start >= last && boundary_before(text, start) && boundary_after(text, end) {
            out.push_str(&text[last..start]);
            out.push_str(replacement);
            last = end;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// One pass over everything that points at another machine or person, so the `<server-N>`
/// numbers follow the order of appearance and equal hosts get equal numbers.
fn replace_endpoints(text: &str, hosts: &[String]) -> String {
    let mut saved: Vec<&str> = hosts
        .iter()
        .map(|h| h.trim())
        .filter(|h| !h.is_empty())
        .collect();
    saved.sort_by_key(|h| std::cmp::Reverse(h.len()));
    saved.dedup();
    let mut pattern = format!("(?i){URL}|{EMAIL}|{V6}|{V4}");
    if !saved.is_empty() {
        let alternatives: Vec<String> = saved.iter().map(|h| regex::escape(h)).collect();
        pattern.push_str(&format!("|(?P<host>{})", alternatives.join("|")));
    }
    let Ok(regex) = Regex::new(&pattern) else {
        return text.to_owned();
    };
    let mut numbers: HashMap<String, usize> = HashMap::new();
    let mut server = |host: &str| -> String {
        let key = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let next = numbers.len() + 1;
        format!("<server-{}>", numbers.entry(key).or_insert(next))
    };
    regex
        .replace_all(text, |caps: &Captures| {
            let whole = caps.get(0).expect("group 0 always exists");
            let (start, end) = (whole.start(), whole.end());
            let keep = || whole.as_str().to_owned();
            if let Some(host) = caps.name("uh") {
                let scheme = &caps["scheme"];
                let web =
                    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
                let lower = host.as_str().to_ascii_lowercase();
                if web && (lower == "github.com" || lower.ends_with(".github.com")) {
                    if caps.name("ui").is_none() {
                        return keep();
                    }
                    return format!("{scheme}://<benutzer>@{}", host.as_str());
                }
                let user = if caps.name("ui").is_some() {
                    "<benutzer>@"
                } else {
                    ""
                };
                return format!("{scheme}://{user}{}", server(host.as_str()));
            }
            if caps.name("email").is_some() {
                return "<email>".to_owned();
            }
            if !(boundary_before(text, start) && boundary_after(text, end)) {
                // `:` and `.` are part of an address, so a word in front of it is not a boundary
                // problem for v6 (handled below); everything else must stand alone.
                if caps.name("v6").is_none() {
                    return keep();
                }
            }
            if caps.name("v4").is_some() {
                let valid = whole
                    .as_str()
                    .split('.')
                    .all(|octet| octet.parse::<u16>().is_ok_and(|n| n <= 255));
                return if valid {
                    server(whole.as_str())
                } else {
                    keep()
                };
            }
            if caps.name("v6").is_some() {
                return v6_replacement(text, whole.as_str(), start, &mut server);
            }
            server(whole.as_str())
        })
        .into_owned()
}

fn v6_replacement(
    text: &str,
    matched: &str,
    start: usize,
    server: &mut impl FnMut(&str) -> String,
) -> String {
    let mut core = matched;
    let mut lead = "";
    if core.starts_with(':') && !core.starts_with("::") {
        lead = ":";
        core = &core[1..];
    }
    let mut trail = "";
    while core.ends_with(':') && !core.ends_with("::") {
        trail = ":";
        core = &core[..core.len() - 1];
    }
    let core_start = start + lead.len();
    let core_end = core_start + core.len();
    let standalone = text[..core_start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word(c) && c != ':' && c != '.')
        && boundary_after(text, core_end);
    let has_digit = core.chars().any(|c| c.is_ascii_hexdigit());
    if standalone && has_digit && core.parse::<Ipv6Addr>().is_ok() {
        format!("{lead}{}{trail}", server(core))
    } else {
        matched.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> AnonContext {
        AnonContext {
            home: "/home/max".into(),
            user: "max".into(),
            hostname: "workbench".into(),
            hosts: vec!["nas.local".into(), "files".into()],
        }
    }

    fn run(text: &str) -> String {
        anonymize(text, &ctx())
    }

    #[test]
    fn home_folder_becomes_a_tilde_with_or_without_trailing_slash() {
        assert_eq!(
            run("Datei /home/max/Dokumente/a.pdf"),
            "Datei ~/Dokumente/a.pdf"
        );
        assert_eq!(run("Ordner /home/max"), "Ordner ~");
        assert_eq!(run("Ordner '/home/max'"), "Ordner '~'");
        let slash = AnonContext {
            home: "/home/max/".into(),
            ..ctx()
        };
        assert_eq!(
            anonymize("in /home/max/x und /home/max/", &slash),
            "in ~/x und ~/"
        );
    }

    #[test]
    fn home_folder_needs_a_whole_path_component() {
        assert_eq!(run("/home/maximum/x"), "/home/maximum/x");
        assert_eq!(run("/var/home/max/x"), "/var/home/<benutzer>/x");
        let root = AnonContext {
            home: "/".into(),
            ..ctx()
        };
        assert_eq!(anonymize("/etc/hosts", &root), "/etc/hosts");
    }

    #[test]
    fn user_name_is_replaced_as_a_whole_word_only() {
        assert_eq!(
            run("Benutzer max meldet sich"),
            "Benutzer <benutzer> meldet sich"
        );
        assert_eq!(
            run("maximum und Maxwell und max."),
            "maximum und Maxwell und <benutzer>."
        );
        assert_eq!(run("(max)"), "(<benutzer>)");
        assert_eq!(run("max_x xmax max-1"), "max_x xmax max-1");
    }

    #[test]
    fn a_file_named_like_the_user_keeps_its_name() {
        assert_eq!(run("max.txt und archiv.max"), "max.txt und archiv.max");
    }

    #[test]
    fn user_name_in_urls_becomes_a_placeholder_even_with_a_password_part() {
        assert_eq!(
            run("sftp://max@example.org/pfad"),
            "sftp://<benutzer>@<server-1>/pfad"
        );
        assert_eq!(
            run("ftp://otto:geheim@example.org:2121/x"),
            "ftp://<benutzer>@<server-1>:2121/x"
        );
    }

    #[test]
    fn short_or_placeholder_like_names_are_not_replaced() {
        let short = AnonContext {
            user: "al".into(),
            hostname: "pc".into(),
            home: String::new(),
            hosts: vec![],
        };
        assert_eq!(anonymize("al und pc", &short), "al und pc");
        let word = AnonContext {
            user: "server".into(),
            ..short
        };
        assert_eq!(anonymize("server", &word), "server");
    }

    #[test]
    fn host_name_of_this_computer_is_replaced_ignoring_case() {
        assert_eq!(
            run("Rechner Workbench, workbench2"),
            "Rechner <rechner>, workbench2"
        );
    }

    #[test]
    fn saved_hosts_are_replaced_everywhere_and_numbered_by_first_appearance() {
        assert_eq!(
            run("Verbindung zu files getrennt, nas.local ok, files wieder"),
            "Verbindung zu <server-1> getrennt, <server-2> ok, <server-1> wieder"
        );
        assert_eq!(run("profiles filesystem"), "profiles filesystem");
    }

    #[test]
    fn urls_of_every_supported_scheme_lose_their_host() {
        for scheme in [
            "smb", "sftp", "ftp", "ftps", "dav", "davs", "webdav", "webdavs", "http", "https",
        ] {
            assert_eq!(
                run(&format!("{scheme}://box.example.net/daten")),
                format!("{scheme}://<server-1>/daten")
            );
        }
        assert_eq!(run("SMB://Box.Example.Net/a"), "SMB://<server-1>/a");
    }

    #[test]
    fn github_links_stay() {
        assert_eq!(
            run("https://github.com/Bexxs75/dualfile/releases und https://api.github.com/x"),
            "https://github.com/Bexxs75/dualfile/releases und https://api.github.com/x"
        );
        assert_eq!(run("smb://github.com/x"), "smb://<server-1>/x");
        assert_eq!(
            run("https://otto:geheim@github.com/x"),
            "https://<benutzer>@github.com/x"
        );
    }

    #[test]
    fn the_same_host_gets_the_same_number_within_one_call() {
        assert_eq!(
            run("smb://a.example/x smb://b.example/y smb://A.example/z 10.0.0.2"),
            "smb://<server-1>/x smb://<server-2>/y smb://<server-1>/z <server-3>"
        );
        assert_eq!(run("smb://b.example/y"), "smb://<server-1>/y");
    }

    #[test]
    fn ip_addresses_are_replaced() {
        assert_eq!(
            run("Server 192.168.2.40 und 192.168.2.40:445"),
            "Server <server-1> und <server-1>:445"
        );
        assert_eq!(
            run("Adresse fe80::1 und 2001:db8::8a2e:370:7334 und ::1"),
            "Adresse <server-1> und <server-2> und <server-3>"
        );
        assert_eq!(
            run("voll 2001:0db8:0000:0000:0000:ff00:0042:8329."),
            "voll <server-1>."
        );
        assert_eq!(run("smb://[fe80::1]:445/x"), "smb://<server-1>:445/x");
    }

    #[test]
    fn things_that_only_look_like_addresses_stay() {
        assert_eq!(
            run("2026-11-16 14:58:02 INFO Version 0.3.0"),
            "2026-11-16 14:58:02 INFO Version 0.3.0"
        );
        assert_eq!(
            run("1.2.3.4.5 und 999.1.1.1 und 300.2.2.2"),
            "1.2.3.4.5 und 999.1.1.1 und 300.2.2.2"
        );
        assert_eq!(
            run("dualfile_core::ops::copy und std::io"),
            "dualfile_core::ops::copy und std::io"
        );
    }

    #[test]
    fn email_addresses_are_replaced() {
        assert_eq!(
            run("Kontakt otto@example.com, anna.b+x@mail.example.org."),
            "Kontakt <email>, <email>."
        );
        assert_eq!(run("max@nas.local"), "<email>");
    }

    #[test]
    fn file_names_stay() {
        assert_eq!(
            run("Kopieren ~/Dokumente/bericht 2026.pdf fehlgeschlagen"),
            "Kopieren ~/Dokumente/bericht 2026.pdf fehlgeschlagen"
        );
    }

    #[test]
    fn mockup_line_is_fully_anonymized() {
        let home_ctx = AnonContext {
            home: "/home/andreas".into(),
            user: "andreas".into(),
            hostname: "anakin".into(),
            hosts: vec![],
        };
        assert_eq!(
            anonymize(
                "ERROR Kopieren /home/andreas/Dokumente/bericht.pdf → smb://192.168.2.40/daten/bericht.pdf",
                &home_ctx
            ),
            "ERROR Kopieren ~/Dokumente/bericht.pdf → smb://<server-1>/daten/bericht.pdf"
        );
    }

    #[test]
    fn anonymizing_twice_changes_nothing() {
        let text = "Verbindung SMB 192.168.2.40/daten als max, /home/max/x, \
                    sftp://max@nas.local/a, otto@example.com, fe80::1, workbench, files";
        let once = run(text);
        assert_eq!(run(&once), once);
    }

    #[test]
    fn unusual_text_does_not_panic() {
        assert_eq!(run(""), "");
        assert_eq!(run("äöü → ✓ max"), "äöü → ✓ <benutzer>");
        let odd = AnonContext {
            hosts: vec!["(".into(), "a|b".into(), "  ".into()],
            ..ctx()
        };
        assert_eq!(anonymize("x a|b (", &odd), "x <server-1> <server-2>");
    }
}
