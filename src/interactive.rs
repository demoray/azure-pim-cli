use crate::models::roles::RoleAssignment;
use anyhow::{anyhow, Result};
use ratatui::{
    crossterm::{
        event::{
            self, Event,
            KeyCode::{BackTab, Backspace, Char, Down, Enter, Esc, Tab, Up},
            KeyEventKind,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    prelude::*,
    widgets::{
        Block, BorderType, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState,
    },
};
use std::{collections::BTreeSet, io::stdout};

const ENABLED: &str = " ✓ ";
const DISABLED: &str = " ☐ ";
const TITLE_TEXT: &str = "Activate Azure PIM roles";
const JUSTIFICATION_TEXT: &str = "Type to enter justification";
const SCOPE_TEXT: &str = "↑ or ↓ to move | Space to toggle";
const DURATION_TEXT: &str = "↑ or ↓ to update duration";
const ALL_HELP: &str = "Tab or Shift-Tab to change sections | Enter to activate | Esc to quit";
const MIN_ITEM_HEIGHT: u16 = 2;
// Width occupied by table chrome inside the bordered block:
// 2 for borders + 1 leading pad + 4 for the highlight spacing column ("  > ").
const SCOPES_CHROME: u16 = 7;
// Minimum width we will allocate to either column before falling back.
const MIN_COL_WIDTH: u16 = 8;

pub struct Selected {
    pub assignments: BTreeSet<RoleAssignment>,
    pub justification: String,
    pub duration: u64,
}

struct Entry {
    value: RoleAssignment,
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InputState {
    Duration,
    Justification,
    Scopes,
}

struct App {
    duration: Option<u64>,
    input_state: InputState,
    table_state: TableState,
    justification: Option<String>,
    items: Vec<Entry>,
    longest_item_lens: (u16, u16),
    scroll_state: ScrollbarState,
    warnings: Vec<String>,
}

impl App {
    fn new(
        assignments: BTreeSet<RoleAssignment>,
        justification: Option<String>,
        duration: Option<u64>,
    ) -> Result<Self> {
        Ok(Self {
            duration,
            input_state: if justification.is_none() {
                InputState::Scopes
            } else {
                InputState::Justification
            },
            table_state: TableState::default().with_selected(0),
            justification,
            longest_item_lens: column_widths(&assignments)?,
            scroll_state: ScrollbarState::new(assignments.len().saturating_sub(1)),
            items: assignments
                .into_iter()
                .map(|value| Entry {
                    value,
                    enabled: false,
                })
                .collect(),
            warnings: Vec::new(),
        })
    }

    fn toggle_current(&mut self) {
        if let Some(i) = self.table_state.selected() {
            if let Some(item) = self.items.get_mut(i) {
                item.enabled = !item.enabled;
            }
        }
    }

    pub fn next(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i >= self.items.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
        self.scroll_state = self.scroll_state.position(i);
    }

    pub fn previous(&mut self) {
        let i = match self.table_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.items.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.table_state.select(Some(i));
        self.scroll_state = self.scroll_state.position(i);
    }

    fn check(&mut self) {
        self.warnings.clear();
        if self.justification.as_ref().is_some_and(String::is_empty) {
            self.warnings.push("Justification is required".to_string());
        }
        if self.items.iter().all(|x| !x.enabled) {
            self.warnings
                .push("At least one role must be selected".to_string());
        }
    }

    #[allow(clippy::indexing_slicing)]
    fn draw(&mut self, f: &mut Frame) {
        let mut sections = vec![
            // title
            Constraint::Length(1),
        ];

        // justification
        if self.justification.is_some() {
            sections.push(Constraint::Length(3));
        }

        // roles
        sections.push(Constraint::Min(5));

        // duration
        if self.duration.is_some() {
            sections.push(Constraint::Length(3));
        }

        // footer
        sections.push(Constraint::Length(4));

        if !self.warnings.is_empty() {
            sections.push(Constraint::Length(
                2 + u16::try_from(self.warnings.len()).unwrap_or(0),
            ));
        }

        let rects = Layout::vertical(sections).split(f.area());
        let mut rects = rects.iter();

        // from here forward, if the next() call fails, we return early as the
        // rect is missing
        let Some(title) = rects.next() else {
            return;
        };
        Self::render_title(f, *title);

        if self.justification.is_some() {
            let Some(justification) = rects.next() else {
                return;
            };
            self.render_justification(f, *justification);
        }

        let Some(scopes) = rects.next() else {
            return;
        };
        self.render_scopes(f, *scopes);

        if self.duration.is_some() {
            let Some(duration) = rects.next() else {
                return;
            };
            self.render_duration(f, *duration);
        }

        let Some(footer) = rects.next() else {
            return;
        };
        self.render_footer(f, *footer);

        if !self.warnings.is_empty() {
            let Some(warnings) = rects.next() else {
                return;
            };
            self.render_warnings(f, *warnings);
        }
    }

    fn render_warnings(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.warnings.join("\n"))
                .style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
                .alignment(Alignment::Center)
                .block(Block::bordered().title("Warnings!")),
            area,
        );
    }

    fn render_title(frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(TITLE_TEXT)
                .style(Style::default().add_modifier(Modifier::BOLD))
                .alignment(Alignment::Center),
            area,
        );
    }

    fn render_duration(&mut self, frame: &mut Frame, area: Rect) {
        // Style::default().add_modifier(Modifier::REVERSED)
        frame.render_widget(
            Paragraph::new(format!("{} minutes", self.duration.unwrap_or_default()))
                .style(if self.input_state == InputState::Duration {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                })
                .block(Block::bordered().title("Duration")),
            area,
        );
    }

    fn render_justification(&mut self, frame: &mut Frame, area: Rect) {
        let justification = self.justification.clone().unwrap_or_default();
        frame.render_widget(
            Paragraph::new(justification.clone()).block(Block::bordered().title("Justification")),
            area,
        );
        if self.input_state == InputState::Justification {
            #[allow(clippy::cast_possible_truncation)]
            frame.set_cursor_position((area.x + justification.len() as u16 + 1, area.y + 1));
        }
    }

    fn render_scopes(&mut self, frame: &mut Frame, area: Rect) {
        // Carve up the area between the role and scope columns based on the
        // current terminal width, so long scope paths don't get silently
        // truncated on a normal-width terminal.
        let inner_w = area.width.saturating_sub(SCOPES_CHROME);
        let role_desired = self.longest_item_lens.0.saturating_add(4);
        let scope_desired = self.longest_item_lens.1.saturating_add(1);

        let (role_w, scope_w) = if role_desired.saturating_add(scope_desired) <= inner_w {
            (
                role_desired,
                inner_w.saturating_sub(role_desired).max(MIN_COL_WIDTH),
            )
        } else {
            // Cap the role column at ~40% so the scope path always has room.
            let cap = (inner_w * 2 / 5).max(MIN_COL_WIDTH);
            let role = role_desired.min(cap).max(MIN_COL_WIDTH);
            let scope = inner_w.saturating_sub(role).max(MIN_COL_WIDTH);
            (role, scope)
        };

        let rows = self.items.iter().map(|data| {
            let role_text = format!(
                "{} {}",
                if data.enabled { ENABLED } else { DISABLED },
                data.value.role
            );
            let scope_text = if let Some(scope_name) = data.value.scope_name.as_deref() {
                format!("{scope_name}\n{}", data.value.scope)
            } else {
                data.value.scope.to_string()
            };

            let role_lines = wrap_text(&role_text, role_w);
            let scope_lines = wrap_text(&scope_text, scope_w);
            let height = u16::try_from(role_lines.len().max(scope_lines.len()))
                .unwrap_or(MIN_ITEM_HEIGHT)
                .max(MIN_ITEM_HEIGHT);

            Row::new(vec![
                Cell::from(role_lines.join("\n")),
                Cell::from(scope_lines.join("\n")),
            ])
            .height(height)
        });

        frame.render_stateful_widget(
            Table::new(rows, [Constraint::Length(role_w), Constraint::Min(scope_w)])
                .header(
                    ["Role", "Scope"]
                        .into_iter()
                        .collect::<Row>()
                        .style(Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED))
                        .height(1),
                )
                .row_highlight_style(if self.input_state == InputState::Scopes {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                })
                .highlight_spacing(HighlightSpacing::Always)
                .block(Block::bordered().title("Scopes")),
            area,
            &mut self.table_state,
        );

        // Always show a scrollbar so users know whether more rows exist
        // off-screen. Render it inside the bordered block on the right edge.
        if self.items.len() > 1 {
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("▲"))
                    .end_symbol(Some("▼")),
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut self.scroll_state,
            );
        }
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(format!(
                "{}\n{ALL_HELP}",
                match self.input_state {
                    InputState::Duration => DURATION_TEXT,
                    InputState::Justification => JUSTIFICATION_TEXT,
                    InputState::Scopes => SCOPE_TEXT,
                }
            ))
            .style(Style::new())
            .centered()
            .block(
                Block::bordered()
                    .title("Help")
                    .border_type(BorderType::Double)
                    .border_style(Style::new()),
            ),
            area,
        );
    }

    fn run<B: Backend>(mut self, terminal: &mut Terminal<B>) -> Result<Option<Selected>> {
        self.check();
        loop {
            terminal
                .draw(|f| self.draw(f))
                .map_err(|e| anyhow!("Failed to draw terminal: {e}"))?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match (self.input_state, key.code) {
                        (InputState::Justification, Tab) | (InputState::Duration, BackTab) => {
                            self.input_state = InputState::Scopes;
                        }
                        (InputState::Scopes, Tab) | (InputState::Justification, BackTab) => {
                            self.input_state = InputState::Duration;
                        }
                        (InputState::Duration, Tab) | (InputState::Scopes, BackTab) => {
                            self.input_state = InputState::Justification;
                        }
                        (InputState::Justification, Char(c)) => {
                            if let Some(justification) = &mut self.justification {
                                justification.push(c);
                            }
                        }
                        (InputState::Justification, Backspace) => {
                            if let Some(justification) = &mut self.justification {
                                justification.pop();
                            }
                        }
                        (InputState::Duration, Down) => {
                            self.duration = self.duration.map(|x| x.saturating_sub(1).max(1));
                        }
                        (InputState::Duration, Up) => {
                            self.duration = self.duration.map(|x| x.saturating_add(1).min(480));
                        }
                        (InputState::Scopes, Char(' ')) => self.toggle_current(),
                        (InputState::Scopes, Down) => self.next(),
                        (InputState::Scopes, Up) => self.previous(),
                        (_, Esc) => return Ok(None),
                        (_, Enter) if self.warnings.is_empty() => {
                            let assignments = self
                                .items
                                .into_iter()
                                .filter(|entry| entry.enabled)
                                .map(|entry| entry.value)
                                .collect();
                            return Ok(Some(Selected {
                                assignments,
                                justification: self.justification.unwrap_or_default(),
                                duration: self.duration.unwrap_or_default(),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            self.check();
        }
    }
}

pub fn interactive_ui(
    items: BTreeSet<RoleAssignment>,
    justification: Option<String>,
    duration: Option<u64>,
) -> Result<Option<Selected>> {
    // setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // create app and run it
    let app = App::new(items, justification, duration)?;
    let res = app.run(&mut terminal);

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen,)?;
    terminal.show_cursor()?;

    res
}

fn column_widths(items: &BTreeSet<RoleAssignment>) -> Result<(u16, u16)> {
    let (scope_name_len, role_len, scope_len) =
        items
            .iter()
            .fold((0, 0, 0), |(scope_name_len, role_len, scope_len), x| {
                (
                    scope_name_len.max(x.scope_name.as_deref().map_or(0, str::len)),
                    role_len.max(x.role.0.len()),
                    scope_len.max(x.scope.0.len()),
                )
            });

    Ok((
        role_len.try_into()?,
        scope_name_len.max(scope_len).try_into()?,
    ))
}

/// Wrap `text` to lines no wider than `width` columns, breaking on whitespace
/// where possible and falling back to hard breaks for long unbroken runs (such
/// as Azure scope paths).
fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in line.split_inclusive(char::is_whitespace) {
            if current.chars().count() + word.chars().count() <= width {
                current.push_str(word);
                continue;
            }
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            // Hard-break any single token longer than the column width.
            let mut chunk = String::new();
            for ch in word.chars() {
                if chunk.chars().count() == width {
                    out.push(std::mem::take(&mut chunk));
                }
                chunk.push(ch);
            }
            current = chunk;
        }
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::wrap_text;

    #[test]
    fn wrap_short_text_unchanged() {
        assert_eq!(wrap_text("hello", 80), vec!["hello".to_string()]);
    }

    #[test]
    fn wrap_breaks_on_whitespace() {
        assert_eq!(
            wrap_text("one two three four", 8),
            vec![
                "one two ".to_string(),
                "three ".to_string(),
                "four".to_string()
            ],
        );
    }

    #[test]
    fn wrap_hard_breaks_long_token() {
        let scope = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg";
        let wrapped = wrap_text(scope, 20);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 20));
        assert_eq!(wrapped.concat(), scope);
    }

    #[test]
    fn wrap_preserves_explicit_newlines() {
        let wrapped = wrap_text("name\n/path/to/scope", 40);
        assert_eq!(
            wrapped,
            vec!["name".to_string(), "/path/to/scope".to_string()]
        );
    }
}
