//! The connection-rule list, and the row component it is built from.
//!
//! A rule is four strings and a flag, and the previous editor showed all five
//! controls for every rule at once: three stacked rows of unlabelled text
//! fields per entry, roughly 110 points each, with node names clipped by the
//! fixed field width. A dozen rules were unreadable and unscannable, and
//! there was no way to add one at all — only to remove.
//!
//! So a rule now presents the way it reads aloud: one card showing
//! `output → input`, with pin and remove on the trailing edge, expanding in
//! place to a labelled full-width form when you actually want to edit it.
//! That is the same shape the relay panel's device rows use, which is
//! deliberate — both are "a list of things you occasionally open".

use super::super::components::{document_button, document_setting_text};
use super::super::shared::{panel_section, text_colors};
use crate::app::QpwgraphApp;
use crate::icons::Icon;
use eframe::egui::{self, RichText, Ui};
use pw_graph_patchbay::PatchConnection;
use pw_graph_ui::{CardProps, Icon as UiIcon, IconButtonProps, Theme, ThemeToken, UiDocument};

/// Room for the chevron, pin, and remove buttons on a rule's trailing edge:
/// three 26 pt icon buttons and the gaps between them, plus a little slack so
/// an elided endpoint never touches the first button.
const RULE_ACTIONS_WIDTH: f32 = 104.0;

/// What the user asked a rule row to do, resolved after the list has been
/// drawn so the loop never mutates the vector it is iterating.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleAction {
    None,
    ToggleExpand(usize),
    TogglePin(usize),
    Remove(usize),
}

/// The four editable strings of a rule, lifted out of the vector so the row
/// can be drawn without holding a borrow on `self.patchbay`.
#[derive(Clone, Default)]
struct RuleFields {
    output_node: String,
    output_name: String,
    input_node: String,
    input_name: String,
}

impl RuleFields {
    fn read(connection: &PatchConnection) -> Self {
        Self {
            output_node: connection.output_node.clone(),
            output_name: connection.output_name.clone(),
            input_node: connection.input_node.clone(),
            input_name: connection.input_name.clone(),
        }
    }

    fn write(self, connection: &mut PatchConnection) {
        connection.output_node = self.output_node;
        connection.output_name = self.output_name;
        connection.input_node = self.input_node;
        connection.input_name = self.input_name;
    }

    /// One endpoint as `node / port`. A rule that has not been filled in yet
    /// shows nothing rather than a bare separator.
    fn endpoint(node: &str, port: &str) -> Option<String> {
        match (node.trim(), port.trim()) {
            ("", "") => None,
            (node, "") => Some(node.to_owned()),
            ("", port) => Some(port.to_owned()),
            (node, port) => Some(format!("{node} / {port}")),
        }
    }
}

impl QpwgraphApp {
    pub(super) fn show_patchbay_rules_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &Theme,
    ) {
        // The count belongs in the section title rather than on a badge of
        // its own: a lone badge on an otherwise empty line reads as a control
        // you can press, and cost a full row to say one digit.
        let title = match self.patchbay.connections.len() {
            0 => self.i18n.text("patchbay.connections"),
            count => format!("{} · {count}", self.i18n.text("patchbay.connections")),
        };
        panel_section(ui, title, theme, |ui| {
            self.show_patchbay_rules_header(document, ui);
            if self.patchbay.connections.is_empty() {
                ui.label(RichText::new(self.i18n.text("patchbay.no_connections")).weak());
                return;
            }
            ui.add_space(4.0);
            let mut action = RuleAction::None;
            for index in 0..self.patchbay.connections.len() {
                let row_action = self.show_patchbay_rule(document, ui, index);
                if row_action != RuleAction::None {
                    action = row_action;
                }
            }
            self.apply_rule_action(action);
        });
    }

    /// The add action. Adding was simply missing before: rules could only
    /// arrive by snapshotting the live graph, so a rule for a node that is
    /// not running yet was impossible to write down.
    fn show_patchbay_rules_header(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if document_button(
                document,
                ui,
                "preferences.patchbay.add_rule",
                self.i18n.text("patchbay.add_rule"),
                true,
            ) {
                self.patchbay.connections.push(PatchConnection::default());
                // A blank rule is useless collapsed — it has nothing to
                // summarise — so open the one that was just added.
                self.patchbay_rule_expanded = Some(self.patchbay.connections.len() - 1);
            }
        });
    }

    fn show_patchbay_rule(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        index: usize,
    ) -> RuleAction {
        let Some(connection) = self.patchbay.connections.get(index) else {
            return RuleAction::None;
        };
        let pinned = connection.pinned;
        let mut fields = RuleFields::read(connection);
        let expanded = self.patchbay_rule_expanded == Some(index);
        let accent = document.theme_color(ThemeToken::Accent);

        let action = document
            .card(
                ui,
                CardProps::new(format!("preferences.patchbay.rule.{index}"))
                    .accent_option(pinned.then_some(accent)),
                |ui, document| {
                    let action = self.show_rule_summary(document, ui, index, &fields, pinned);
                    if expanded {
                        ui.add_space(2.0);
                        ui.separator();
                        fields = self.show_rule_fields(document, ui, index, fields.clone());
                    }
                    action
                },
            )
            .unwrap_or(RuleAction::None);

        if let Some(connection) = self.patchbay.connections.get_mut(index) {
            fields.write(connection);
        }
        action
    }

    /// The collapsed face of a rule: what it connects, and what can be done
    /// to it. The endpoint text is given a bounded column so long node names
    /// elide inside it instead of running over the actions.
    fn show_rule_summary(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        index: usize,
        fields: &RuleFields,
        pinned: bool,
    ) -> RuleAction {
        let expanded = self.patchbay_rule_expanded == Some(index);
        let colors = text_colors(document.theme());
        let accent = document.theme_color(ThemeToken::Accent);
        let output = RuleFields::endpoint(&fields.output_node, &fields.output_name);
        let input = RuleFields::endpoint(&fields.input_node, &fields.input_name);
        let output_label = self.i18n.text("patchbay.output");
        let input_label = self.i18n.text("patchbay.input");
        let placeholder = self.i18n.text("patchbay.new_rule");

        pw_graph_ui::setting_row_sized(
            ui,
            RULE_ACTIONS_WIDTH,
            |ui| {
                ui.vertical(|ui| {
                    match (&output, &input) {
                        (None, None) => {
                            ui.label(RichText::new(placeholder).italics().color(colors.weak));
                        }
                        _ => {
                            endpoint_line(ui, &output_label, output.as_deref(), &colors);
                            endpoint_line(ui, &input_label, input.as_deref(), &colors);
                        }
                    };
                });
            },
            |ui| {
                // Right-to-left: remove sits furthest out, where a
                // destructive action is hardest to hit by accident.
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("preferences.patchbay.rule.{index}.remove"),
                        Icon::Delete.source(),
                    )
                    .tooltip(self.i18n.text("patchbay.remove_rule")),
                ) {
                    return RuleAction::Remove(index);
                }
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("preferences.patchbay.rule.{index}.pin"),
                        Icon::Pin.source(),
                    )
                    .tint(if pinned { accent } else { colors.weak })
                    .tooltip(self.i18n.text("patchbay.pin_rule")),
                ) {
                    return RuleAction::TogglePin(index);
                }
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("preferences.patchbay.rule.{index}.expand"),
                        UiIcon::disclosure(expanded),
                    )
                    .frameless(true)
                    .tooltip(self.i18n.text("patchbay.expand_rule")),
                ) {
                    return RuleAction::ToggleExpand(index);
                }
                RuleAction::None
            },
        )
    }

    /// The open form. Each endpoint half is labelled, full width, and on its
    /// own line — a patchbay rule matches by name, so seeing the whole name
    /// is the entire point of the field.
    fn show_rule_fields(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        index: usize,
        mut fields: RuleFields,
    ) -> RuleFields {
        ui.add_space(2.0);
        for (suffix, label_key, value) in [
            (
                "output_node",
                "patchbay.output_node",
                &mut fields.output_node,
            ),
            (
                "output_name",
                "patchbay.output_port",
                &mut fields.output_name,
            ),
            ("input_node", "patchbay.input_node", &mut fields.input_node),
            ("input_name", "patchbay.input_port", &mut fields.input_name),
        ] {
            *value = document_setting_text(
                document,
                ui,
                &format!("preferences.patchbay.rule.{index}.{suffix}"),
                value,
                self.i18n.text(label_key),
                String::new(),
            );
        }
        fields
    }

    fn apply_rule_action(&mut self, action: RuleAction) {
        match action {
            RuleAction::None => {}
            RuleAction::ToggleExpand(index) => {
                self.patchbay_rule_expanded = if self.patchbay_rule_expanded == Some(index) {
                    None
                } else {
                    Some(index)
                };
            }
            RuleAction::TogglePin(index) => {
                if let Some(connection) = self.patchbay.connections.get_mut(index) {
                    connection.pinned = !connection.pinned;
                }
            }
            RuleAction::Remove(index) => {
                if index < self.patchbay.connections.len() {
                    self.patchbay.connections.remove(index);
                }
                // Rules are addressed by position, so anything at or after
                // the removed index now means a different rule. Collapse
                // rather than silently open the wrong one.
                self.patchbay_rule_expanded = match self.patchbay_rule_expanded {
                    Some(expanded) if expanded >= index => None,
                    other => other,
                };
            }
        }
    }
}

/// `Output  node / port`, with the endpoint itself carrying the emphasis.
///
/// PipeWire node names are routinely 60 characters or more
/// (`alsa_input.usb-EMEET_EMEET_SmartCam_C960_Ultra_A260…analog-stereo`), and
/// a label left to its natural width simply drew over the pin and remove
/// buttons to its right. The endpoint is elided to the column it was given
/// and carries the untruncated value as its tooltip — expanding the rule is
/// the other way to read the whole thing.
fn endpoint_line(
    ui: &mut Ui,
    label: &str,
    endpoint: Option<&str>,
    colors: &super::super::shared::TextColors,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).small().color(colors.weak));
        match endpoint {
            Some(endpoint) => {
                ui.add(egui::Label::new(RichText::new(endpoint).color(colors.primary)).truncate())
                    .on_hover_text(endpoint);
            }
            None => {
                ui.label(RichText::new("—").color(colors.weak));
            }
        }
    });
}
