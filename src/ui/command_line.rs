//! OpenCADStudio-style command line — bottom panel with input and history

use std::time::Instant;

use crate::app::Message;
use iced::widget::{button, column, container, opaque, row, scrollable, text, text_input};
use iced::{Background, Border, Color, Element, Length, Theme};

pub const CMD_INPUT_ID: &str = "cmd_input";

/// How long a history entry stays visible on the overlay before fading
/// out. Picking the full archive happens through the dropdown button.
const HISTORY_VISIBLE_SECS: f32 = 3.0;

/// How many autocomplete matches the suggestion popup shows at once.
const AUTOCOMPLETE_LIMIT: usize = 8;

fn cmd_input_id() -> iced::widget::Id {
    iced::widget::Id::new(CMD_INPUT_ID)
}

const MAX_HISTORY: usize = 64;

#[derive(Clone, Default)]
pub struct CommandLine {
    pub input: String,
    pub history: Vec<HistoryEntry>,
    /// Commands the user has typed (for ↑/↓ recall).
    pub cmd_recall: Vec<String>,
    /// Current position in `cmd_recall` while navigating (None = not navigating).
    recall_cursor: Option<usize>,
    /// Saved draft input before the user started navigating history.
    recall_draft: String,
    /// When `true`, the dropdown showing the full backlog is open.
    pub history_open: bool,
    /// Index of the currently-highlighted autocomplete suggestion, or
    /// `None` when the user hasn't yet started navigating with the
    /// arrow keys. Reset on every keystroke.
    pub autocomplete_cursor: Option<usize>,
    /// The active command step's prompt, mirrored here so a step change
    /// can be detected and the pinned (non-fading) history line updated.
    step_prompt: Option<String>,
}

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub kind: EntryKind,
    pub text: String,
    /// When this entry was pushed. Used by the overlay to fade entries
    /// out after `HISTORY_VISIBLE_SECS`. The dropdown popup ignores it
    /// and always shows the whole list.
    pub created_at: Instant,
    /// The active command step's prompt is pinned so it does not fade
    /// while the user is still working on that step. When the step
    /// completes the pin is cleared and the normal cooldown resumes.
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Command,
    Output,
    Error,
    Info,
}

impl CommandLine {
    pub fn new() -> Self {
        let mut cl = Self::default();
        cl.push_info("Open CAD Studio ready.");
        cl.push_info("Type a command or use the ribbon. Open OBJ: INSERT tab.");
        cl
    }

    pub fn submit(&mut self) -> Option<String> {
        let raw = self.input.trim().to_string();
        if raw.is_empty() {
            return None;
        }
        // Uppercase only the command verb (the first token); keep arguments
        // verbatim so case-sensitive values survive — file paths on
        // case-sensitive filesystems, identifiers, plugin command arguments.
        // Dispatch matches verbs in uppercase and each sub-command handler
        // re-uppercases its own keywords, so only free-form args are affected.
        let cmd = match raw.split_once(char::is_whitespace) {
            Some((verb, rest)) => format!("{} {}", verb.to_uppercase(), rest),
            None => raw.to_uppercase(),
        };
        // Record in recall list (avoid duplicates at the top).
        if self.cmd_recall.last().map(|s| s.as_str()) != Some(raw.as_str()) {
            self.cmd_recall.push(raw);
            if self.cmd_recall.len() > 50 {
                self.cmd_recall.remove(0);
            }
        }
        self.recall_cursor = None;
        self.recall_draft.clear();
        self.push_command(&self.input.clone());
        self.input.clear();
        Some(cmd)
    }

    /// Navigate to the previous command in recall history (↑).
    pub fn history_prev(&mut self) {
        if self.cmd_recall.is_empty() {
            return;
        }
        let cursor = match self.recall_cursor {
            None => {
                self.recall_draft = self.input.clone();
                self.cmd_recall.len() - 1
            }
            Some(c) if c > 0 => c - 1,
            Some(c) => c,
        };
        self.recall_cursor = Some(cursor);
        self.input = self.cmd_recall[cursor].clone();
    }

    /// Navigate to the next command in recall history (↓).
    pub fn history_next(&mut self) {
        match self.recall_cursor {
            None => {}
            Some(c) if c + 1 < self.cmd_recall.len() => {
                let next = c + 1;
                self.recall_cursor = Some(next);
                self.input = self.cmd_recall[next].clone();
            }
            Some(_) => {
                self.recall_cursor = None;
                self.input = self.recall_draft.clone();
            }
        }
    }

    pub fn push_command(&mut self, cmd: &str) {
        self.push(EntryKind::Command, format!("Command: {cmd}"));
    }
    pub fn push_output(&mut self, msg: &str) {
        self.push(EntryKind::Output, msg.to_string());
    }
    pub fn push_error(&mut self, msg: &str) {
        self.push(EntryKind::Error, format!("*Invalid*  {msg}"));
    }
    pub fn push_info(&mut self, msg: &str) {
        self.push(EntryKind::Info, msg.to_string());
    }
    fn push(&mut self, kind: EntryKind, text: String) {
        self.history.push(HistoryEntry {
            kind,
            text,
            created_at: Instant::now(),
            pinned: false,
        });
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    /// Mirror the active command step's prompt. While the step is current
    /// its history line is pinned so it does not fade; when the step
    /// changes (or the command ends, `prompt == None`) the previous line's
    /// cooldown restarts so it fades normally from now. The dispatch /
    /// step-transition code already pushes the prompt as an `Info` line, so
    /// the matching tail entry is pinned in place rather than duplicated.
    pub fn set_step_prompt(&mut self, prompt: Option<String>) {
        if prompt == self.step_prompt {
            return;
        }
        // Release the previous pin and let its cooldown start now.
        for e in self.history.iter_mut().filter(|e| e.pinned) {
            e.pinned = false;
            e.created_at = Instant::now();
        }
        if let Some(p) = &prompt {
            // Reuse the prompt line dispatch/step-transition just pushed;
            // otherwise add it.
            if self.history.last().map(|e| &e.text) != Some(p) {
                self.push(EntryKind::Info, p.clone());
            }
            if let Some(last) = self.history.last_mut() {
                last.pinned = true;
            }
        }
        self.step_prompt = prompt;
    }

    /// `true` while at least one history entry is still within the
    /// visible window — the host app uses this to drive a low-frequency
    /// tick subscription so the overlay re-renders and fades the entry
    /// once it expires.
    pub fn has_visible_history(&self) -> bool {
        self.history
            .iter()
            .any(|e| e.pinned || e.created_at.elapsed().as_secs_f32() < HISTORY_VISIBLE_SECS)
    }

    pub fn toggle_history(&mut self) {
        self.history_open = !self.history_open;
    }

    pub fn close_history(&mut self) {
        self.history_open = false;
    }

    /// Move the autocomplete highlight up one entry. Wraps to the last
    /// match. Returns `true` when there was a list to navigate.
    pub fn autocomplete_prev(&mut self) -> bool {
        let len = self.autocomplete_matches().len();
        if len == 0 {
            return false;
        }
        let next = match self.autocomplete_cursor {
            None => len - 1,
            Some(0) => len - 1,
            Some(i) => i - 1,
        };
        self.autocomplete_cursor = Some(next);
        true
    }

    /// Move the autocomplete highlight down one entry. Wraps to the
    /// first match. Returns `true` when there was a list to navigate.
    pub fn autocomplete_next(&mut self) -> bool {
        let len = self.autocomplete_matches().len();
        if len == 0 {
            return false;
        }
        let next = match self.autocomplete_cursor {
            None => 0,
            Some(i) if i + 1 < len => i + 1,
            Some(_) => 0,
        };
        self.autocomplete_cursor = Some(next);
        true
    }

    /// The command name the user has currently highlighted in the
    /// autocomplete popup, if any.
    pub fn selected_suggestion(&self) -> Option<&'static str> {
        let matches = self.autocomplete_matches();
        self.autocomplete_cursor
            .and_then(|i| matches.get(i).copied())
    }

    /// Autocomplete suggestions for the current input, capped at
    /// [`AUTOCOMPLETE_LIMIT`]. Match is substring-anywhere (typing
    /// `LEADER` surfaces `LEADER` and `MLEADER` and `QLEADER`); the
    /// list is sorted so prefix matches come first.
    ///
    /// Names come from `crate::command::all_registered_command_names()`
    /// which collects every `inventory::submit!` block placed next to a
    /// `CadCommand` impl — no central list to maintain.
    pub fn autocomplete_matches(&self) -> Vec<&'static str> {
        let typed = self.input.trim();
        if typed.is_empty() {
            return Vec::new();
        }
        let needle = typed.to_uppercase();
        let mut matches: Vec<&'static str> = crate::command::all_registered_command_names()
            .into_iter()
            .filter(|cmd| cmd.contains(&needle))
            .collect();
        matches.sort();
        matches.dedup();
        // Prefix matches rank above mid-string ones, then alphabetical
        // so the order is stable as the user keeps typing.
        matches.sort_by_key(|cmd| (!cmd.starts_with(&needle), *cmd));
        matches.truncate(AUTOCOMPLETE_LIMIT);
        matches
    }

    pub fn view(&self, show_autocomplete: bool, dyn_capturing: bool) -> Element<'_, Message> {
        // Only the most recent entries pushed within the last few
        // seconds show on the overlay. The dropdown button keeps the
        // full backlog reachable when the user actually wants it.
        let visible: Vec<&HistoryEntry> = self
            .history
            .iter()
            .filter(|e| e.pinned || e.created_at.elapsed().as_secs_f32() < HISTORY_VISIBLE_SECS)
            .collect();
        let start = visible.len().saturating_sub(4);
        let history_rows = visible[start..]
            .iter()
            .fold(column![].spacing(0), |col, entry| {
                let color = match entry.kind {
                    EntryKind::Command => CMD_COLOR,
                    EntryKind::Output => OUT_COLOR,
                    EntryKind::Error => ERR_COLOR,
                    EntryKind::Info => INFO_COLOR,
                };
                col.push(container(text(&entry.text).size(11).color(color)).padding([1, 8]))
            });
        let prompt = container(text("Command:").size(11).color(PROMPT_COLOR)).padding([5, 8]);
        // While dynamic input is capturing keystrokes, the command-line
        // text field is left without an `on_input` handler so it can't
        // grab focus or swallow numeric keys — those flow through the
        // global key subscription into the dynamic-input fields instead.
        let mut input = text_input("", &self.input).id(cmd_input_id());
        if !dyn_capturing {
            input = input
                .on_input(Message::CommandInput)
                .on_submit(Message::CommandSubmit);
        }
        let input = input
            .style(|_: &Theme, _| text_input::Style {
                background: Background::Color(INPUT_BG),
                border: Border {
                    color: Color {
                        r: 0.40,
                        g: 0.60,
                        b: 0.90,
                        a: 1.0,
                    },
                    width: 1.0,
                    radius: 2.0.into(),
                },
                icon: Color::WHITE,
                placeholder: Color {
                    r: 0.4,
                    g: 0.4,
                    b: 0.4,
                    a: 1.0,
                },
                value: Color::WHITE,
                selection: Color {
                    r: 0.20,
                    g: 0.44,
                    b: 0.72,
                    a: 0.5,
                },
            })
            .size(11)
            .padding([4, 6]);
        // Autocomplete suggestions panel, shown above the input row
        // when the user has typed a prefix that matches at least one
        // command. Each row is a button — clicking it dispatches the
        // command directly.
        let autocomplete: Element<'_, Message> = if show_autocomplete {
            let matches = self.autocomplete_matches();
            if matches.is_empty() {
                container(column![]).height(0).into()
            } else {
                let cursor = self.autocomplete_cursor;
                let mut col = column![].spacing(0).width(Length::Fill);
                for (idx, cmd) in matches.iter().enumerate() {
                    let is_selected = cursor == Some(idx);
                    let row = button(text(*cmd).size(11).color(CMD_COLOR))
                        .on_press(Message::CommandSuggestionPick(cmd.to_string()))
                        .width(Length::Fill)
                        .padding([2, 8])
                        .style(move |_: &Theme, status| {
                            let bg = if is_selected {
                                Color {
                                    r: 0.28,
                                    g: 0.40,
                                    b: 0.56,
                                    a: 1.0,
                                }
                            } else if matches!(status, button::Status::Hovered) {
                                Color {
                                    r: 0.22,
                                    g: 0.30,
                                    b: 0.42,
                                    a: 1.0,
                                }
                            } else {
                                PANEL_BG
                            };
                            button::Style {
                                background: Some(Background::Color(bg)),
                                text_color: Color::WHITE,
                                border: Border::default(),
                                ..Default::default()
                            }
                        });
                    col = col.push(row);
                }
                container(col)
                    .style(|_: &Theme| container::Style {
                        background: Some(Background::Color(PANEL_BG)),
                        border: Border {
                            color: BORDER_COLOR,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill)
                    .into()
            }
        } else {
            container(column![]).height(0).into()
        };

        // Dropdown trigger next to the input. Clicking it pops up the
        // full backlog (everything pushed since the app started) so the
        // user can recover anything that has already faded off the
        // overlay.
        let dropdown_label = if self.history_open { "▾" } else { "▸" };
        let dropdown_btn = button(
            text(dropdown_label)
                .size(11)
                .color(PROMPT_COLOR),
        )
        .on_press(Message::CommandHistoryToggle)
        .style(|_: &Theme, _status| button::Style {
            background: Some(Background::Color(INPUT_ROW_BG)),
            text_color: Color::WHITE,
            border: Border {
                color: BORDER_COLOR,
                width: 1.0,
                radius: 3.0.into(),
            },
            ..Default::default()
        })
        .padding([2, 6]);
        let input_row = row![prompt, input, dropdown_btn]
            .spacing(4)
            .align_y(iced::Center);

        // Full backlog dropdown — appears ABOVE the input pill when
        // open. The history `Vec` already contains every line pushed
        // since startup; render them all (newest at the bottom) in a
        // scrollable. `opaque` wraps the panel so mouse-wheel events
        // inside the dropdown don't bubble through to the viewport
        // shader behind it (otherwise scrolling the history zoomed
        // the drawing). `anchor_bottom` keeps the newest line in view
        // when the dropdown first opens.
        let dropdown: Element<'_, Message> = if self.history_open {
            let mut col = column![].spacing(0).width(Length::Fill);
            for entry in &self.history {
                let color = match entry.kind {
                    EntryKind::Command => CMD_COLOR,
                    EntryKind::Output => OUT_COLOR,
                    EntryKind::Error => ERR_COLOR,
                    EntryKind::Info => INFO_COLOR,
                };
                col = col.push(
                    container(text(&entry.text).size(11).color(color)).padding([1, 8]),
                );
            }
            let panel = container(
                scrollable(col).anchor_bottom().width(Length::Fill),
            )
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(PANEL_BG)),
                    border: Border {
                        color: BORDER_COLOR,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                })
                .width(Length::Fill)
                .max_height(200.0)
                .padding([4, 0]);
            opaque(panel).into()
        } else {
            container(column![]).height(0).into()
        };

        container(column![
            autocomplete,
            dropdown,
            container(history_rows)
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(HISTORY_BG)),
                    ..Default::default()
                })
                .width(Length::Fill)
                .padding([2, 0]),
            container(input_row)
                .style(|_: &Theme| container::Style {
                    background: Some(Background::Color(INPUT_ROW_BG)),
                    border: Border {
                        color: BORDER_COLOR,
                        width: 1.0,
                        radius: 3.0.into()
                    },
                    ..Default::default()
                })
                .width(Length::Fill),
        ])
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(PANEL_BG)),
            border: Border {
                color: BORDER_COLOR,
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fixed(720.0))
        .into()
    }
}

const PANEL_BG: Color = Color {
    r: 0.15,
    g: 0.15,
    b: 0.15,
    a: 1.0,
};
const HISTORY_BG: Color = Color {
    r: 0.15,
    g: 0.15,
    b: 0.15,
    a: 1.0,
};
const INPUT_ROW_BG: Color = Color {
    r: 0.18,
    g: 0.18,
    b: 0.18,
    a: 1.0,
};
const INPUT_BG: Color = Color {
    r: 0.12,
    g: 0.12,
    b: 0.12,
    a: 1.0,
};
const BORDER_COLOR: Color = Color {
    r: 0.30,
    g: 0.30,
    b: 0.30,
    a: 1.0,
};
const PROMPT_COLOR: Color = Color {
    r: 0.55,
    g: 0.78,
    b: 0.55,
    a: 1.0,
};
const CMD_COLOR: Color = Color {
    r: 0.80,
    g: 0.80,
    b: 0.80,
    a: 1.0,
};
const OUT_COLOR: Color = Color {
    r: 0.65,
    g: 0.65,
    b: 0.65,
    a: 1.0,
};
const ERR_COLOR: Color = Color {
    r: 0.90,
    g: 0.35,
    b: 0.35,
    a: 1.0,
};
const INFO_COLOR: Color = Color {
    r: 0.50,
    g: 0.70,
    b: 0.90,
    a: 1.0,
};

