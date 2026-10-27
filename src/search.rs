//! Turns a query into ranked results from all providers.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};

use crate::calc;
use crate::config::Config;
use crate::history::History;
use crate::indexer::AppEntry;
use crate::util;

const MAX_APP_RESULTS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Calc,
    Url,
    Shell,
    Web,
}

impl Kind {
    /// Label of the Enter action, shown on the selected row.
    pub fn verb(self) -> &'static str {
        match self {
            Kind::App | Kind::Url => "Open",
            Kind::Calc => "Copy",
            Kind::Shell => "Run",
            Kind::Web => "Search",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Launch { id: String, path: Option<String> },
    Copy(String),
    Open(String),
    Shell(String),
}

#[derive(Debug, Clone)]
pub struct ResultItem {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    /// Matched ranges in `title` as UTF-16 `(start, len)`, for highlighting.
    pub highlights: Vec<(u32, u32)>,
    pub action: Action,
}

pub struct Searcher {
    matcher: Matcher,
    buf: Vec<char>,
    indices: Vec<u32>,
}

impl Searcher {
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(MatcherConfig::DEFAULT),
            buf: Vec::new(),
            indices: Vec::new(),
        }
    }

    pub fn search(&mut self, query: &str, apps: &[AppEntry], history: &History, config: &Config) -> Vec<ResultItem> {
        let query = query.trim();
        let now = util::now_secs();
        let mut results = Vec::new();

        if query.is_empty() {
            if config.show_recent {
                let mut recent: Vec<_> = apps
                    .iter()
                    .map(|app| (history.boost(&app.id, now), app))
                    .filter(|(boost, _)| *boost > 0)
                    .collect();
                recent.sort_by(|a, b| b.0.cmp(&a.0));
                results.extend(
                    recent
                        .into_iter()
                        .take(config.max_results)
                        .map(|(_, app)| app_result(app, Vec::new())),
                );
            }
            return results;
        }

        if let Some(command) = query.strip_prefix('>') {
            let command = command.trim();
            results.push(ResultItem {
                kind: Kind::Shell,
                title: if command.is_empty() {
                    "Type a command to run".into()
                } else {
                    command.into()
                },
                subtitle: "Run in Command Prompt · Ctrl+Shift+Enter to run as administrator".into(),
                highlights: Vec::new(),
                action: Action::Shell(command.into()),
            });
            return results;
        }

        if let Some(value) = calc::evaluate(query) {
            let text = calc::format(value);
            results.push(ResultItem {
                kind: Kind::Calc,
                title: text.clone(),
                subtitle: format!("{query} · Enter to copy"),
                highlights: Vec::new(),
                action: Action::Copy(text),
            });
        }

        if let Some(url) = as_url(query) {
            results.push(ResultItem {
                kind: Kind::Url,
                title: url.clone(),
                subtitle: "Open in browser".into(),
                highlights: Vec::new(),
                action: Action::Open(url),
            });
        }

        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let lower = query.to_lowercase();
        let mut scored: Vec<_> = apps
            .iter()
            .filter_map(|app| {
                self.indices.clear();
                let score =
                    pattern.indices(haystack(&app.name, &mut self.buf), &mut self.matcher, &mut self.indices)?;
                self.indices.sort_unstable();
                self.indices.dedup();
                if !is_clean_match(&app.name, &self.indices) {
                    return None;
                }
                let prefix = if app.name.to_lowercase().starts_with(&lower) {
                    40
                } else {
                    0
                };
                Some((
                    score + prefix + history.boost(&app.id, now),
                    app,
                    utf16_ranges(&app.name, &self.indices),
                ))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.len().cmp(&b.1.name.len())));
        for (_, app, highlights) in scored.into_iter().take(MAX_APP_RESULTS) {
            results.push(app_result(app, highlights));
        }

        results.push(ResultItem {
            kind: Kind::Web,
            title: format!("Search the web for \u{201C}{query}\u{201D}"),
            subtitle: host(&config.search_url).unwrap_or("Web search").to_string(),
            highlights: Vec::new(),
            action: Action::Open(config.search_url.replace("{query}", &url_encode(query))),
        });
        results
    }
}

fn app_result(app: &AppEntry, highlights: Vec<(u32, u32)>) -> ResultItem {
    ResultItem {
        kind: Kind::App,
        title: app.name.clone(),
        subtitle: app.path.clone().unwrap_or_else(|| "App".into()),
        highlights,
        action: Action::Launch {
            id: app.id.clone(),
            path: app.path.clone(),
        },
    }
}

/// Builds a haystack whose match indices are `char` positions in `text`.
/// (`Utf32Str::new` segments by grapheme instead, which would desync the
/// indices from `text.chars()` for names with combining marks.)
fn haystack<'a>(text: &'a str, buf: &'a mut Vec<char>) -> Utf32Str<'a> {
    if text.is_ascii() {
        Utf32Str::Ascii(text.as_bytes())
    } else {
        buf.clear();
        buf.extend(text.chars());
        Utf32Str::Unicode(buf)
    }
}

/// Rejects scattered matches: every run of matched chars must start at a word
/// boundary, except at most one. `vsc` → **V**isual **S**tudio **C**ode is
/// clean; `code` → Re**co**r**d**e**r** is not.
fn is_clean_match(text: &str, char_indices: &[u32]) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let word_start = |i: usize| {
        i == 0 || !chars[i - 1].is_alphanumeric() || (chars[i - 1].is_lowercase() && chars[i].is_uppercase())
    };
    let mid_word_runs = char_indices
        .iter()
        .enumerate()
        .filter(|&(n, &i)| n == 0 || char_indices[n - 1] + 1 != i)
        .filter(|&(_, &i)| !word_start(i as usize))
        .count();
    mid_word_runs <= 1
}

/// Converts sorted char indices into merged UTF-16 `(start, len)` ranges.
fn utf16_ranges(text: &str, char_indices: &[u32]) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut wanted = char_indices.iter().peekable();
    let mut offset = 0u32;
    for (i, c) in text.chars().enumerate() {
        let len = c.len_utf16() as u32;
        if wanted.next_if(|&&w| w as usize == i).is_some() {
            match ranges.last_mut() {
                Some((start, l)) if *start + *l == offset => *l += len,
                _ => ranges.push((offset, len)),
            }
        }
        offset += len;
    }
    ranges
}

fn as_url(query: &str) -> Option<String> {
    if query.contains(char::is_whitespace) {
        return None;
    }
    let lower = query.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(query.into())
    } else if lower.starts_with("www.") && query.len() > 4 {
        Some(format!("https://{query}"))
    } else {
        None
    }
}

fn host(url: &str) -> Option<&str> {
    let rest = url.split("://").nth(1)?;
    rest.split(['/', '?']).next()
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> AppEntry {
        AppEntry {
            name: name.into(),
            id: name.into(),
            path: None,
        }
    }

    #[test]
    fn ranks_and_highlights_apps() {
        let apps = [
            app("Visual Studio Code"),
            app("Notepad"),
            app("Visual Studio Installer"),
        ];
        let mut searcher = Searcher::new();
        let results = searcher.search("vsc", &apps, &History::default(), &Config::default());
        assert_eq!(results[0].title, "Visual Studio Code");
        assert_eq!(results[0].highlights, vec![(0, 1), (7, 1), (14, 1)]);
        assert_eq!(results.last().unwrap().kind, Kind::Web);

        let apps = [
            app("Visual Studio Code"),
            app("Sound Recorder"),
            app("Command Palette"),
            app("Windows Terminal"),
            app("Notepad"),
            app("Google Chrome"),
            app("Character Map"),
        ];
        let mut titles = |query: &str| -> Vec<String> {
            searcher
                .search(query, &apps, &History::default(), &Config::default())
                .into_iter()
                .filter(|r| r.kind == Kind::App)
                .map(|r| r.title)
                .collect()
        };
        assert_eq!(titles("code"), ["Visual Studio Code"]);
        assert_eq!(titles("note"), ["Notepad"]);
        assert_eq!(titles("pad"), ["Notepad"]);
        assert_eq!(titles("chr"), ["Google Chrome", "Character Map"]);
        assert_eq!(titles("chrme"), ["Google Chrome"]);
    }

    #[test]
    fn fzf_style_operators() {
        let apps = [
            app("Visual Studio Code"),
            app("Windows Terminal"),
            app("Microsoft Edge"),
            app("Edge Dev Tools"),
        ];
        let mut searcher = Searcher::new();
        let mut titles = |query: &str| -> Vec<String> {
            searcher
                .search(query, &apps, &History::default(), &Config::default())
                .into_iter()
                .filter(|r| r.kind == Kind::App)
                .map(|r| r.title)
                .collect()
        };
        assert_eq!(titles("^vis"), ["Visual Studio Code"]);
        assert_eq!(titles("terminal$"), ["Windows Terminal"]);
        assert_eq!(titles("'edge"), ["Microsoft Edge", "Edge Dev Tools"]);
        assert_eq!(titles("'edge !dev"), ["Microsoft Edge"]);
    }

    #[test]
    fn unicode_names() {
        // Combining marks: nucleo segments graphemes, so its indices must not be
        // mistaken for char indices.
        let apps = [app("e\u{301}ab"), app("Café Tools"), app("日本語 Editor")];
        let mut searcher = Searcher::new();
        let (h, c) = (History::default(), Config::default());
        for query in ["b", "ab", "cafe", "tools", "editor", "日本"] {
            searcher.search(query, &apps, &h, &c);
        }
        let results = searcher.search("ab", &apps, &h, &c);
        assert_eq!(results[0].title, "e\u{301}ab");
        assert_eq!(results[0].highlights, vec![(2, 2)]);
    }

    #[test]
    fn special_queries() {
        let mut searcher = Searcher::new();
        let (h, c) = (History::default(), Config::default());
        assert_eq!(searcher.search("2+2", &[], &h, &c)[0].title, "4");
        assert_eq!(searcher.search("> dir", &[], &h, &c)[0].kind, Kind::Shell);
        assert_eq!(
            searcher.search("www.rust-lang.org", &[], &h, &c)[0].title,
            "https://www.rust-lang.org"
        );
        assert!(searcher.search("", &[app("Notepad")], &h, &c).is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(url_encode("a b&c"), "a+b%26c");
        assert_eq!(host("https://www.google.com/search?q={query}"), Some("www.google.com"));
        assert_eq!(utf16_ranges("a🎉bc", &[0, 2, 3]), vec![(0, 1), (3, 2)]);
    }
}
