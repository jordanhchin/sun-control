//! Small reusable pieces: text inputs, forms and layout helpers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub value: String,
    pub masked: bool,
    /// Cursor position in characters.
    cursor: usize,
}

impl Input {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            value,
            masked: false,
            cursor,
        }
    }

    pub fn masked() -> Self {
        Self {
            masked: true,
            ..Self::default()
        }
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map_or(self.value.len(), |(i, _)| i)
    }

    /// Returns true if the key was handled.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let len = self.value.chars().count();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let at = self.byte_at(self.cursor);
                self.value.replace_range(..at, "");
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let before: String = self.value.chars().take(self.cursor).collect();
                let trimmed = before.trim_end();
                let keep = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                let keep_chars = before[..keep].chars().count();
                let (from, to) = (self.byte_at(keep_chars), self.byte_at(self.cursor));
                self.value.replace_range(from..to, "");
                self.cursor = keep_chars;
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = len,
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte_at(self.cursor);
                self.value.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.value.remove(at);
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.value.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }

    pub fn display(&self) -> String {
        if self.masked {
            "•".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }

    /// Render into `width` columns, scrolling horizontally to keep the cursor visible.
    pub fn line(&self, width: usize, focused: bool) -> Line<'static> {
        let chars: Vec<char> = self.display().chars().collect();
        let width = width.max(2);
        if !focused {
            let shown: String = if chars.len() > width {
                chars[..width - 1].iter().collect::<String>() + "…"
            } else {
                chars.iter().collect()
            };
            return Line::raw(shown);
        }
        let start = (self.cursor + 1).saturating_sub(width);
        let end = (start + width).min(chars.len());
        let before: String = chars[start..self.cursor].iter().collect();
        let under: String = chars.get(self.cursor).map_or(" ".into(), |c| c.to_string());
        let after: String = if self.cursor < end {
            chars[self.cursor + 1..end].iter().collect()
        } else {
            String::new()
        };
        Line::from(vec![
            Span::raw(before),
            Span::styled(under, Style::new().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ])
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub label: &'static str,
    pub input: Input,
    pub hint: &'static str,
}

impl Field {
    pub fn new(label: &'static str, input: Input) -> Self {
        Self {
            label,
            input,
            hint: "",
        }
    }

    pub fn hint(mut self, hint: &'static str) -> Self {
        self.hint = hint;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Form<K> {
    pub title: String,
    pub note: Option<String>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub kind: K,
}

pub enum FormEvent {
    Submit,
    Cancel,
    None,
}

impl<K> Form<K> {
    pub fn new(title: impl Into<String>, kind: K, fields: Vec<Field>) -> Self {
        // Start on the first empty field so prefilled values can be skipped.
        let focus = fields
            .iter()
            .position(|f| f.input.value.is_empty())
            .unwrap_or(0);
        Self {
            title: title.into(),
            note: None,
            fields,
            focus,
            kind,
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn value(&self, i: usize) -> &str {
        self.fields[i].input.value.trim()
    }

    pub fn handle(&mut self, key: KeyEvent) -> FormEvent {
        match key.code {
            KeyCode::Esc => return FormEvent::Cancel,
            KeyCode::Enter if self.focus + 1 < self.fields.len() => self.focus += 1,
            KeyCode::Enter => return FormEvent::Submit,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % self.fields.len(),
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + self.fields.len() - 1) % self.fields.len()
            }
            _ => {
                self.fields[self.focus].input.handle(key);
            }
        }
        FormEvent::None
    }
}

/// A rectangle of the given size centred in `area`, clamped to fit.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn edits_at_cursor() {
        let mut i = Input::new("steam");
        i.handle(key(KeyCode::Home));
        for c in "set".chars() {
            i.handle(key(KeyCode::Char(c)));
        }
        i.handle(key(KeyCode::Char('é')));
        i.handle(key(KeyCode::Backspace));
        i.handle(key(KeyCode::Char(' ')));
        assert_eq!(i.value, "set steam");
        i.handle(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.value, "steam");
        i.handle(key(KeyCode::End));
        i.handle(key(KeyCode::Left));
        i.handle(key(KeyCode::Delete));
        assert_eq!(i.value, "stea");
    }
}
