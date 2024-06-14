use crate::roles::ScopeEntry;
use anyhow::Result;
use crossterm::{
    event::{
        self, Event,
        KeyCode::{Backspace, Char, Down, Enter, Esc, Tab, Up},
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
const JUSTIFICATION_TEXT: &str =
    "Tab to select scopes | Type to enter justification | Enter to activate | Esc to quit";
const SCOPE_TEXT: &str = "Tab to edit justification | ↑ or ↓ to move | Space to toggle | Enter to activate | Esc to quit";
const ITEM_HEIGHT: u16 = 2;

pub enum Action {
    Activate {
        scopes: Vec<ScopeEntry>,
        justification: String,
    },
    Quit,
}

struct Entry {
    value: ScopeEntry,
    enabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InputState {
    Justification,
    Scopes,
}

struct App {
    input_state: InputState,
    table_state: TableState,
    justification: String,
    items: Vec<Entry>,
    longest_item_lens: (u16, u16),
    scroll_state: ScrollbarState,
}

impl App {
    fn new(items: Vec<ScopeEntry>, justification: Option<String>) -> Result<Self> {
        Ok(Self {
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

    #[allow(clippy::indexing_slicing)]
    fn draw(&mut self, f: &mut Frame) {
        let rects = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(f.size());
        Self::render_title(f, rects[0]);
        self.render_justification(f, rects[1]);
        self.render_table(f, rects[2]);
        self.render_footer(f, rects[3]);
    }

    fn render_title(frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(TITLE_TEXT)
                .style(Style::default().add_modifier(Modifier::BOLD))
                .alignment(Alignment::Center),
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

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
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
            Paragraph::new(Line::from(match self.input_state {
                InputState::Justification => JUSTIFICATION_TEXT,
                InputState::Scopes => SCOPE_TEXT,
            }))
            .style(Style::new())
            .centered()
            .block(
                Block::bordered()
                    .border_type(BorderType::Double)
                    .border_style(Style::new()),
            ),
            area,
        );
    }

    fn run<B: Backend>(mut self, terminal: &mut Terminal<B>) -> Result<Action> {
        loop {
            terminal.draw(|f| self.draw(f))?;

            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match (self.input_state, key.code) {
                        (InputState::Justification, Tab) => self.input_state = InputState::Scopes,
                        (InputState::Justification, Char(c)) => {
                            self.justification.push(c);
                        }
                        (InputState::Justification, Backspace) => {
                            self.justification.pop();
                        }
                        (InputState::Scopes, Tab) => self.input_state = InputState::Justification,
                        (InputState::Scopes, Char(' ')) => self.toggle_current(),
                        (InputState::Scopes, Down) => self.next(),
                        (InputState::Scopes, Up) => self.previous(),
                        (_, Esc) => return Ok(Action::Quit),
                        (_, Enter) => {
                            let items = self
                                .items
                                .into_iter()
                                .filter(|entry| entry.enabled)
                                .map(|entry| entry.value)
                                .collect();
                            return Ok(Action::Activate {
                                scopes: items,
                                justification: self.justification,
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub fn interactive_ui(items: Vec<ScopeEntry>, justification: Option<String>) -> Result<Action> {
    // setup terminal
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // create app and run it
    let app = App::new(items, justification)?;
    let res = app.run(&mut terminal);

    // restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen,)?;
    terminal.show_cursor()?;

    res
}

fn column_widths(items: &[ScopeEntry]) -> Result<(u16, u16)> {
    let scope_name_len = items
        .iter()
        .map(|x| x.scope_name.len())
        .max()
        .unwrap_or_default();
    let role_len = items
        .iter()
        .map(|x| x.role.0.len())
        .max()
        .unwrap_or_default();
    let scope_len = items
        .iter()
        .map(|x| x.scope.0.len())
        .max()
        .unwrap_or_default();

    Ok((
        role_len.try_into()?,
        scope_name_len.max(scope_len).try_into()?,
    ))
}
