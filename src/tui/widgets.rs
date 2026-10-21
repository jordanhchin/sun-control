//! Small reusable pieces: text inputs, forms and layout helpers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Flex, Layout, Rect};

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub value: String,
    pub masked: bool,
}

impl Input {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            masked: false,
        }
    }

    pub fn masked() -> Self {
        Self {
            value: String::new(),
            masked: true,
        }
    }

    /// Returns true if the key edited the input.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.clear()
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let trimmed = self.value.trim_end().len();
                let cut = self.value[..trimmed].rfind(' ').map(|i| i + 1).unwrap_or(0);
                self.value.truncate(cut);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.value.push(c)
            }
            KeyCode::Backspace => {
                self.value.pop();
            }
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
