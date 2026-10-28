use super::{MessageButton, failure_text};
use crate::core::model::{Column, Model};
use crate::core::results::{ListMessage, Shown};
use crate::core::settings::Mode;
use crate::core::text::{Strings, Text};
use gozler_core::query::{Malformed, QueryError};

/// What the list's empty area says instead of rows: the message, and its
/// buttons, in [`Model::list_buttons`]' order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListCaption {
    pub(crate) text: String,
    pub(crate) buttons: Vec<MessageButton>,
}

/// A column's header.
pub(crate) fn header(strings: &Strings, column: Column) -> Text {
    let row = column.row();
    Text {
        full: strings.text(row.header).to_owned(),
        short: row.short_header.map(|key| strings.text(key).to_owned()),
    }
}

/// What the list's empty area says instead of rows, if anything. Commander mode
/// hides the list, and its message with it.
pub(crate) fn list_message(model: &Model) -> Option<ListCaption> {
    if model.mode() == Mode::Commander {
        return None;
    }
    let strings = model.strings();
    let text = match model.search_list().results().shown() {
        Shown::Rows(_) if model.search_list().results().total() == 0 => {
            strings.text("list.no_results").to_owned()
        }
        Shown::Nothing | Shown::Rows(_) => return None,
        Shown::Message(ListMessage::Parse(error)) => query_error(strings, error),
        Shown::Message(ListMessage::MissingFolder(folder)) => {
            strings.format("list.missing_folder", &[folder])
        }
        Shown::Message(ListMessage::Failure(failure)) => failure_text(strings, failure),
        Shown::Message(ListMessage::TypeToSearch) => strings.text("list.type_to_search").to_owned(),
        Shown::Message(ListMessage::NotIndexed { root }) => {
            strings.format("panel.search_not_indexed", &[root])
        }
    };
    let buttons = model
        .list_buttons()
        .iter()
        .map(|button| MessageButton::new(model, button))
        .collect();
    Some(ListCaption { text, buttons })
}

/// What is wrong with a search that does not parse, and how to fix it, in
/// the person's language; text from a library stays English after it.
pub(super) fn query_error(strings: &Strings, error: &QueryError) -> String {
    let as_typed = |function: &str, value: &str| format!("{function}{value}");
    match error {
        QueryError::UnsupportedOperator(c) => {
            strings.format("query.unsupported_operator", &[&c.to_string()])
        }
        QueryError::UnknownFunction(name) => {
            strings.format("query.unknown_function", &[&format!("{name}:")])
        }
        QueryError::RelativePath(path) => strings.format("query.relative_path", &[path]),
        QueryError::NoQueryStart(_) => strings.text("query.no_query_start").to_owned(),
        QueryError::TooManyTerms(n) => strings.format("query.too_many_terms", &[&n.to_string()]),
        QueryError::TooManyRegexes(n) => {
            strings.format("query.too_many_regexes", &[&n.to_string()])
        }
        QueryError::Malformed(reason) => match reason {
            Malformed::MisplacedNot => strings.text("query.misplaced_not").to_owned(),
            Malformed::NotWithoutTerm => strings.text("query.not_without_term").to_owned(),
            Malformed::OrWithoutTerms => strings.text("query.or_without_terms").to_owned(),
            Malformed::OperatorOnNothing(term) => {
                strings.format("query.operator_on_nothing", &[term])
            }
            Malformed::Size(value) => strings.format("query.size", &[&as_typed("size:", value)]),
            Malformed::ReversedDates(field, value) => strings.format(
                "query.reversed_dates",
                &[&as_typed(field.function(), value), field.function()],
            ),
            Malformed::DateOutOfRange(field, value) => strings.format(
                "query.date_out_of_range",
                &[&as_typed(field.function(), value), field.function()],
            ),
            Malformed::Days(field, value) => strings.format(
                "query.days",
                &[&as_typed(field.function(), value), field.function()],
            ),
            Malformed::Date(field, value) => strings.format(
                "query.date",
                &[&as_typed(field.function(), value), field.function()],
            ),
            Malformed::NoParentFolder => strings.text("query.no_parent_folder").to_owned(),
            Malformed::AttributeLetter(c) => {
                strings.format("query.attribute_letter", &[&c.to_string()])
            }
            Malformed::Regex(detail) => strings.format("query.regex", &[detail]),
        },
    }
}
