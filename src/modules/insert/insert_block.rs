use acadrust::entities::{AttributeEntity, Insert};
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub fn tool() -> ToolDef {
    ToolDef {
        id: "INSERT",
        label: "Insert Block",
        icon: IconKind::Svg(include_bytes!("../../../assets/icons/blocks/insert.svg")),
        event: ModuleEvent::Command("INSERT".to_string()),
    }
}

enum Step {
    Name,
    Point {
        name: String,
    },
    /// Attribute filling: prompt the user tag by tag.
    FillAttr {
        /// Attribute definitions: (tag, prompt_text, default_value).
        attdefs: Vec<(String, String, String)>,
        /// Index of the attdef currently being prompted.
        idx: usize,
        /// (tag, value) pairs collected so far.
        values: Vec<(String, String)>,
    },
}

pub struct InsertBlockCommand {
    available: Vec<String>,
    step: Step,
    /// Pending Insert entity stored while attr-filling is in progress.
    pending_insert: Option<Insert>,
}

impl InsertBlockCommand {
    pub fn new(available: Vec<String>) -> Self {
        Self {
            available,
            step: Step::Name,
            pending_insert: None,
        }
    }
}

impl CadCommand for InsertBlockCommand {
    fn name(&self) -> &'static str {
        "INSERT"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Name => {
                let hint = if self.available.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", self.available.join(", "))
                };
                format!("INSERT  Enter block name:{hint}")
            }
            Step::Point { name } => format!("INSERT  Specify insertion point for \"{}\":", name),
            Step::FillAttr { attdefs, idx, .. } => {
                if let Some((tag, prompt, default)) = attdefs.get(*idx) {
                    let default_hint = if default.is_empty() {
                        String::new()
                    } else {
                        format!("  <{default}>")
                    };
                    let prompt_text = if prompt.is_empty() {
                        tag.as_str()
                    } else {
                        prompt.as_str()
                    };
                    format!("INSERT  {prompt_text}{default_hint}:")
                } else {
                    "INSERT  Filling attributes...".into()
                }
            }
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::Name => CmdResult::NeedPoint,
            Step::Point { name } => {
                let ins = Insert::new(
                    name.clone(),
                    Vector3::new(pt.x as f64, pt.y as f64, pt.z as f64),
                );
                let block_name = name.clone();
                self.pending_insert = Some(ins);
                // Signal the host to check for attdefs.
                CmdResult::AttreqNeeded { block_name }
            }
            Step::FillAttr { .. } => CmdResult::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match &self.step {
            Step::Name | Step::Point { .. } => CmdResult::Cancel,
            Step::FillAttr { .. } => {
                // Treat Enter as accepting the default.
                self.accept_attr_value("")
            }
        }
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::Name | Step::FillAttr { .. })
    }

    fn wants_text_with_spaces(&self) -> bool {
        // Block names don't embed whitespace, but attribute values do.
        matches!(self.step, Step::FillAttr { .. })
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        match &self.step {
            Step::Name => {
                let name = text.trim();
                if !self.available.iter().any(|c| c.eq_ignore_ascii_case(name)) {
                    return None;
                }
                self.step = Step::Point {
                    name: name.to_string(),
                };
                Some(CmdResult::NeedPoint)
            }
            Step::FillAttr { .. } => Some(self.accept_attr_value(text)),
            Step::Point { .. } => None,
        }
    }

    fn on_preview_wires(&mut self, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }

    fn attreq_set_attdefs(&mut self, attdefs: Vec<(String, String, String)>) {
        self.step = Step::FillAttr {
            attdefs,
            idx: 0,
            values: vec![],
        };
    }

    fn attreq_take_insert(&mut self) -> Option<acadrust::EntityType> {
        self.pending_insert
            .take()
            .map(|ins| EntityType::Insert(ins))
    }
}

impl InsertBlockCommand {
    /// Accept the current attribute value (empty = use default) and advance.
    /// Returns CommitAndExit when all attdefs have been filled.
    fn accept_attr_value(&mut self, text: &str) -> CmdResult {
        let (tag, default, next_idx, total) = match &self.step {
            Step::FillAttr { attdefs, idx, .. } => {
                let (tag, _, default) = &attdefs[*idx];
                (tag.clone(), default.clone(), idx + 1, attdefs.len())
            }
            _ => return CmdResult::Cancel,
        };

        let value = if text.trim().is_empty() {
            default
        } else {
            text.trim().to_string()
        };

        if let Step::FillAttr {
            ref mut values,
            ref mut idx,
            ..
        } = self.step
        {
            values.push((tag, value));
            *idx = next_idx;
        }

        if next_idx >= total {
            // All attdefs filled — build the INSERT with AttributeEntity list.
            let values = match &self.step {
                Step::FillAttr { values, .. } => values.clone(),
                _ => vec![],
            };
            let mut ins = match self.pending_insert.take() {
                Some(i) => i,
                None => return CmdResult::Cancel,
            };
            for (tag, value) in values {
                let mut attr = AttributeEntity {
                    tag,
                    value: value.clone(),
                    ..Default::default()
                };
                attr.set_value(&value);
                ins.attributes.push(attr);
            }
            CmdResult::CommitAndExit(EntityType::Insert(ins))
        } else {
            CmdResult::NeedPoint
        }
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["INSERT"] });  // InsertBlockCommand
