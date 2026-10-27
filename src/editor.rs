//! Search box text editing: caret, selection and movement by word.
//! Positions use byte offsets at character boundaries.

use std::ops::Range;

#[derive(Default)]
pub struct Editor {
    text: String,
    caret: usize,
    anchor: usize,
    high_surrogate: Option<u16>,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    pub fn selection(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn insert(&mut self, s: &str) {
        let range = self.selection();
        self.text.replace_range(range.clone(), s);
        self.caret = range.start + s.len();
        self.anchor = self.caret;
    }

    /// Buffers UTF-16 surrogate pairs from separate `WM_CHAR` messages.
    /// Returns true when a complete character changes the text.
    pub fn push_utf16(&mut self, unit: u16) -> bool {
        match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(unit);
                false
            }
            0xDC00..=0xDFFF => {
                let Some(high) = self.high_surrogate.take() else {
                    return false;
                };
                self.insert(&String::from_utf16_lossy(&[high, unit]));
                true
            }
            _ => {
                self.high_surrogate = None;
                match char::from_u32(unit as u32) {
                    Some(c) if !c.is_control() => {
                        self.insert(c.encode_utf8(&mut [0; 4]));
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    pub fn backspace(&mut self, word: bool) -> bool {
        if self.has_selection() {
            self.insert("");
            return true;
        }
        if self.caret == 0 {
            return false;
        }
        let start = if word {
            self.prev_word(self.caret)
        } else {
            self.prev_char(self.caret)
        };
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
        self.anchor = start;
        true
    }

    pub fn delete(&mut self, word: bool) -> bool {
        if self.has_selection() {
            self.insert("");
            return true;
        }
        if self.caret == self.text.len() {
            return false;
        }
        let end = if word {
            self.next_word(self.caret)
        } else {
            self.next_char(self.caret)
        };
        self.text.replace_range(self.caret..end, "");
        true
    }

    pub fn left(&mut self, word: bool, extend: bool) {
        if !extend && self.has_selection() {
            let start = self.selection().start;
            self.move_to(start, false);
        } else {
            let pos = if word {
                self.prev_word(self.caret)
            } else {
                self.prev_char(self.caret)
            };
            self.move_to(pos, extend);
        }
    }

    pub fn right(&mut self, word: bool, extend: bool) {
        if !extend && self.has_selection() {
            let end = self.selection().end;
            self.move_to(end, false);
        } else {
            let pos = if word {
                self.next_word(self.caret)
            } else {
                self.next_char(self.caret)
            };
            self.move_to(pos, extend);
        }
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        self.caret = pos;
        if !extend {
            self.anchor = pos;
        }
    }

    fn prev_char(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map_or(0, |(j, _)| j)
    }

    fn next_char(&self, i: usize) -> usize {
        self.text[i..].chars().next().map_or(i, |c| i + c.len_utf8())
    }

    fn char_before(&self, i: usize) -> Option<char> {
        self.text[..i].chars().next_back()
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.text[i..].chars().next()
    }

    fn prev_word(&self, mut i: usize) -> usize {
        while self.char_before(i).is_some_and(char::is_whitespace) {
            i = self.prev_char(i);
        }
        while self.char_before(i).is_some_and(|c| !c.is_whitespace()) {
            i = self.prev_char(i);
        }
        i
    }

    fn next_word(&self, mut i: usize) -> usize {
        while self.char_at(i).is_some_and(|c| !c.is_whitespace()) {
            i = self.next_char(i);
        }
        while self.char_at(i).is_some_and(char::is_whitespace) {
            i = self.next_char(i);
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> Editor {
        let mut e = Editor::default();
        for unit in s.encode_utf16() {
            e.push_utf16(unit);
        }
        e
    }

    #[test]
    fn types_and_deletes_characters_and_words() {
        let mut e = typed("hello wörld 🎉");
        assert_eq!(e.text(), "hello wörld 🎉");
        e.backspace(false);
        assert_eq!(e.text(), "hello wörld ");
        e.backspace(true);
        assert_eq!(e.text(), "hello ");
        e.left(true, false);
        e.delete(true);
        assert_eq!(e.text(), "");
    }

    #[test]
    fn typing_replaces_selected_text() {
        let mut e = typed("notepad");
        e.select_all();
        assert_eq!(e.selected_text(), "notepad");
        e.push_utf16('c' as u16);
        assert_eq!(e.text(), "c");
        e.home(false);
        e.end(true);
        assert_eq!(e.selection(), 0..1);
    }
}
