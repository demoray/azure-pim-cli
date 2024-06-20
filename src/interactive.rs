use crate::roles::Assignment;
use anyhow::Result;
use crossterm::{
    event::{
        self, Event,
        KeyCode::{BackTab, Backspace, Char, Down, Enter, Esc, Tab, Up},
        KeyEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{
        Block, BorderType, Cell, HighlightSpacing, Paragraph, Row, ScrollbarState, Table,
        TableState,
    },
};
use std::io::stdout;

const ENABLED: &str = " ✓ ";
const DISABLED: &str = " ☐ ";
const TITLE_TEXT: &str = "Activate Azure PIM roles";
const JUSTIFICATION_TEXT: &str = "Type to enter justification";
const SCOPE_TEXT: &str = "↑ or ↓ to move | Space to toggle";
const DURATION_TEXT: &str = "↑ or ↓ to update duration";
const ALL_HELP: &str = "Tab or Shift-Tab to change sections | Enter to activate | Esc to quit";
const ITEM_HEIGHT: u16 = 2;

pub enum Action {
    Activate {
        scopes: Vec<Assignment>,
        justification: String,
        duration: u32,
    },
    Quit,
}

struct Entry {
    value: Assignment,
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InputState {
    Duration,
    Justification,
    Scopes,
}

struct App {
    duration: u32,
    input_state: InputState,
    table_state: TableState,
    justification: String,
    items: Vec<Entry>,
    longest_item_lens: (u16, u16),
    scroll_state: ScrollbarState,
    warnings: Vec<String>,
}

impl App {
    fn new(items: Vec<Assignment>, justification: Option<String>, duration: u32) -> Result<Self> {
        Ok(Self {
            duration,
            input_state: if justification.is_none() {
                InputState::Justification
            } else {
                InputState::Scopes
            },
            table_state: TableState::default().with_selected(0),
            justification: justification.unwrap_or_default(),
            longest_item_lens: column_widths(&items)?,
            scroll_state: ScrollbarState::new((items.len() - 1) * usize::from(ITEM_HEIGHT)),
            items: items
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
        self.scroll_state = self.scroll_state.position(i * usize::from(ITEM_HEIGHT));
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
        self.scroll_state = self.scroll_state.position(i * usize::from(ITEM_HEIGHT));
    }

    fn check(&mut self) {
        self.warnings.clear();
        if self.justification.is_empty() {
            self.warnings.push("Justification is required".to_string());
        }
        if self.items.iter().all(|x| !x.enabled) {
            self.warnings
                .push("At least one role must be selected".to_string());
        }
    }

    #[allow(clippy::indexing_slicing)]
    fn draw(&mut self, f: &mut Frame) {
        let mut constraints = vec![
            // title
            Constraint::Length(1),
            // justification
            Constraint::Length(3),
            // roles
            Constraint::Min(5),
            // duration
            Constraint::Length(3),
            // footer
            Constraint::Length(4),
        ];

        if !self.warnings.is_empty() {
            constraints.push(Constraint::Length(
                2 + u16::try_from(self.warnings.len()).unwrap_or(0),
            ));
        }

        let rects = Layout::vertical(constraints).split(f.size());
        Self::render_title(f, rects[0]);
        self.render_justification(f, rects[1]);
        self.render_scopes(f, rects[2]);
        self.render_duration(f, rects[3]);
        self.render_footer(f, rects[4]);
        if !self.warnings.is_empty() {
            self.render_warnings(f, rects[5]);
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
            Paragraph::new(format!("{} minutes", self.duration))
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
        frame.render_widget(
            Paragraph::new(self.justification.as_str())
                .block(Block::bordered().title("Justification")),
            area,
        );
        if self.input_state == InputState::Justification {
            #[allow(clippy::cast_possible_truncation)]
            frame.set_cursor(area.x + self.justification.len() as u16 + 1, area.y + 1);
        }
    }

    fn render_scopes(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_stateful_widget(
            Table::new(
                self.items.iter().map(|data| {
                    Row::new(vec![
                        Cell::from(Text::from(format!(
                            "{} {}",
                            if data.enabled { ENABLED } else { DISABLED },
                            data.value.role
                        ))),
                        Cell::from(Text::from(format!(
                            "{}\n{}",
                            data.value.scope_name, data.value.scope
                        ))),
                    ])
                    .height(ITEM_HEIGHT)
                }),
                [
                    Constraint::Length(self.longest_item_lens.0 + 4),
                    Constraint::Min(self.longest_item_lens.1 + 1),
                ],
            )
            .header(
                ["Role", "Scope"]
                    .into_iter()
                    .map(Cell::from)
                    .collect::<Row>()
                    .style(Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED))
                    .height(1),
            )
            .highlight_style(if self.input_state == InputState::Scopes {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
            .highlight_spacing(HighlightSpacing::Always)
            .block(Block::bordered().title("Scopes")),
            area,
            &mut self.table_state,
        );
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(Text::from(format!(
                "{}\n{ALL_HELP}",
                match self.input_state {
                    InputState::Duration => DURATION_TEXT,
                    InputState::Justification => JUSTIFICATION_TEXT,
                    InputState::Scopes => SCOPE_TEXT,
                }
            )))
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

    fn run<B: Backend>(mut self, terminal: &mut Terminal<B>) -> Result<Action> {
        self.check();
        loop {
            terminal.draw(|f| self.draw(f))?;

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
                            self.justification.push(c);
                        }
                        (InputState::Justification, Backspace) => {
                            self.justification.pop();
                        }
                        (InputState::Duration, Down) => {
                            self.duration = self.duration.saturating_sub(1).max(1);
                        }
                        (InputState::Duration, Up) => {
                            self.duration = self.duration.saturating_add(1).min(480);
                        }
                        (InputState::Scopes, Char(' ')) => self.toggle_current(),
                        (InputState::Scopes, Down) => self.next(),
                        (InputState::Scopes, Up) => self.previous(),
                        (_, Esc) => return Ok(Action::Quit),
                        (_, Enter) if self.warnings.is_empty() => {
                            let items = self
                                .items
                                .into_iter()
                                .filter(|entry| entry.enabled)
                                .map(|entry| entry.value)
                                .collect();
                            return Ok(Action::Activate {
                                scopes: items,
                                justification: self.justification,
                                duration: self.duration,
                            });
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
    items: Vec<Assignment>,
    justification: Option<String>,
    duration: u32,
) -> Result<Action> {
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

fn column_widths(items: &[Assignment]) -> Result<(u16, u16)> {
    let (scope_name_len, role_len, scope_len) =
        items
            .iter()
            .fold((0, 0, 0), |(scope_name_len, role_len, scope_len), x| {
                (
                    scope_name_len.max(x.scope_name.len()),
                    role_len.max(x.role.0.len()),
                    scope_len.max(x.scope.0.len()),
                )
            });

    Ok((
        role_len.try_into()?,
        scope_name_len.max(scope_len).try_into()?,
    ))
}
