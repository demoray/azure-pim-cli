use crate::models::roles::{RoleAssignment, RoleAssignments};
use anyhow::Result;
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
        Block, BorderType, HighlightSpacing, Paragraph, Row, ScrollbarState, Table, TableState,
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

pub struct Selected {
    pub assignments: RoleAssignments,
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
        assignments: RoleAssignments,
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
            scroll_state: ScrollbarState::new((assignments.0.len() - 1) * usize::from(ITEM_HEIGHT)),
            items: assignments
                .0
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
        if self.justification.as_ref().map_or(false, String::is_empty) {
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

        let rects = Layout::vertical(sections).split(f.size());
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
            frame.set_cursor(area.x + justification.len() as u16 + 1, area.y + 1);
        }
    }

    fn render_scopes(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_stateful_widget(
            Table::new(
                self.items.iter().map(|data| {
                    Row::new(vec![
                        format!(
                            "{} {}",
                            if data.enabled { ENABLED } else { DISABLED },
                            data.value.role
                        ),
                        format!("{}\n{}", data.value.scope_name, data.value.scope),
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
                            let items = self
                                .items
                                .into_iter()
                                .filter(|entry| entry.enabled)
                                .map(|entry| entry.value)
                                .collect();
                            return Ok(Some(Selected {
                                assignments: RoleAssignments(items),
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
    items: RoleAssignments,
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

fn column_widths(items: &RoleAssignments) -> Result<(u16, u16)> {
    let (scope_name_len, role_len, scope_len) =
        items
            .0
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
