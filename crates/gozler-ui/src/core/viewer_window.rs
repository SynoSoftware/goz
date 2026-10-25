//! The Viewer window's decisions (DESIGN.md: The Viewer) on top of
//! [`super::viewer`]'s: which blocks of the file it keeps, the first shown row,
//! the page of rows that the window paints, the scroll bar, the keys, the menus
//! and the status bar.
//!
//! The window owns the file and its reading thread. It gives a [`Document`]
//! each block that the thread reads, and reads the block that
//! [`Document::advance`] asks for. A page and a move go on row by row from
//! where they waited for a block, so one that needs more blocks than the
//! Viewer keeps still ends, and reads each block once.

use super::commands::{Key, Keystroke, Mark, MenuNode};
use super::glyph::Glyph;
use super::layout::{Dpi, Px, Rect, Size};
use super::text::{self, Numbers, Strings, menu_label};
use super::viewer::{self, Detected, HEX_ROW_BYTES, LINE_BYTES, Mode, Place, SNIFF_BYTES};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::Range;

/// The bytes the reading thread reads at once. The first block holds the
/// bytes that choose Text or Hex and the encoding.
pub(crate) const BLOCK_BYTES: usize = SNIFF_BYTES;

/// How many of the blocks read last the Viewer keeps.
const KEPT_BLOCKS: usize = 16;

/// The wheel delta of one row: a notch of 120 scrolls three rows, as
/// Windows does by default.
const WHEEL_ROW: i32 = 40;

const HEX_ROW: u64 = HEX_ROW_BYTES as u64;

/// A block that the page or a move waits for, by its index.
type Need<T> = Result<T, u64>;

/// The blocks the Viewer keeps, and the file's length.
struct Blocks {
    /// The length when the file opened, or where its bytes ended since.
    len: u64,
    /// The oldest first.
    kept: VecDeque<(u64, Box<[u8]>)>,
}

impl Blocks {
    fn range(&self, index: u64) -> Range<u64> {
        let start = index.saturating_mul(BLOCK_BYTES as u64).min(self.len);
        start..start.saturating_add(BLOCK_BYTES as u64).min(self.len)
    }

    /// Keeps the block `index`, and forgets the oldest beyond
    /// [`KEPT_BLOCKS`]. Fewer bytes than the length promises mean that the
    /// file shrank since it opened, so it ends there; more mean that it
    /// grew, which the Viewer does not show.
    fn keep(&mut self, index: u64, mut bytes: Vec<u8>) {
        let range = self.range(index);
        let expected = (range.end - range.start) as usize;
        if bytes.len() < expected {
            self.len = range.start + bytes.len() as u64;
            let len = self.len;
            self.kept
                .retain(|(i, _)| i.saturating_mul(BLOCK_BYTES as u64) < len);
            for (i, block) in &mut self.kept {
                let room = (len - *i * BLOCK_BYTES as u64).min(BLOCK_BYTES as u64) as usize;
                if block.len() > room {
                    *block = block[..room].into();
                }
            }
        }
        bytes.truncate(expected);
        if bytes.is_empty() {
            return;
        }
        self.kept.retain(|(i, _)| *i != index);
        self.kept.push_back((index, bytes.into_boxed_slice()));
        while self.kept.len() > KEPT_BLOCKS {
            self.kept.pop_front();
        }
    }

    /// The bytes of `range`, cut at the file's end; `Err` names a block
    /// that is not kept.
    fn bytes(&self, range: Range<u64>) -> Need<Cow<'_, [u8]>> {
        let range = range.start.min(self.len)..range.end.min(self.len);
        if range.is_empty() {
            return Ok(Cow::Borrowed(&[]));
        }
        let block_bytes = BLOCK_BYTES as u64;
        let (first, last) = (range.start / block_bytes, (range.end - 1) / block_bytes);
        if first == last {
            return self.part(first, &range).map(Cow::Borrowed);
        }
        let mut joined = Vec::with_capacity((range.end - range.start) as usize);
        for index in first..=last {
            joined.extend_from_slice(self.part(index, &range)?);
        }
        Ok(Cow::Owned(joined))
    }

    /// The bytes of `range`, within the file, that the block `index` holds.
    fn part(&self, index: u64, range: &Range<u64>) -> Need<&[u8]> {
        let block = self
            .kept
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, block)| &**block)
            .ok_or(index)?;
        let base = index * BLOCK_BYTES as u64;
        let from = (range.start.max(base) - base) as usize;
        let to = (range.end.min(base + BLOCK_BYTES as u64) - base) as usize;
        Ok(block
            .get(from..to)
            .expect("a kept block holds its bytes up to the file's end"))
    }
}

/// Where a row starts: the line that holds it, and which of the line's
/// wrapped rows it is. In Hex a line is a row of 16 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RowAt {
    line: u64,
    row: usize,
}

/// One row of the page.
#[derive(Clone, Debug)]
struct Row {
    text: String,
    /// How many characters at the start of `text` show in the gray text
    /// colour: a hex row's offset.
    muted: usize,
    at: RowAt,
}

/// What follows the page's last row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Row(RowAt),
    /// The last row is the file's last.
    End,
}

/// What the document does until its page is complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Task {
    Idle,
    /// Adds rows to the page from [`Document::after`].
    Page,
    /// Moves up by `rows` rows from `at`, then shows the page there.
    Up {
        at: RowAt,
        rows: usize,
    },
    /// Shows the page from the first line that starts at or after
    /// `offset`.
    Jump {
        offset: u64,
    },
}

/// The rows of the line decoded last, which the rows of one wrapped line
/// share.
struct LineRows {
    line: u64,
    rows: Vec<String>,
    muted: usize,
}

/// A file as the Viewer shows it.
pub(crate) struct Document {
    blocks: Blocks,
    detected: Detected,
    mode: Mode,
    wrap: bool,
    /// The characters of a row that fit the area's width.
    columns: usize,
    /// The rows that fit the area's height.
    height: usize,
    /// The first shown column while long lines do not wrap.
    left: usize,
    top: RowAt,
    rows: Vec<Row>,
    after: After,
    task: Task,
    decoded: Option<LineRows>,
    /// The wheel's turn not yet scrolled, less than a row.
    wheel: i32,
}

impl Document {
    /// A file of `len` bytes, whose first block `first` the thread read,
    /// shown as its first bytes choose, from its start.
    pub(crate) fn new(len: u64, first: Vec<u8>, wrap: bool) -> Document {
        let mode = viewer::choose_mode(&first);
        let detected = viewer::detect_encoding(&first);
        let mut blocks = Blocks {
            len,
            kept: VecDeque::new(),
        };
        blocks.keep(0, first);
        let mut document = Document {
            blocks,
            detected,
            mode,
            wrap,
            columns: 1,
            height: 1,
            left: 0,
            top: RowAt { line: 0, row: 0 },
            rows: Vec::new(),
            after: After::End,
            task: Task::Page,
            decoded: None,
            wheel: 0,
        };
        document.show_from(document.start_row());
        document
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The first byte the page shows, where Retry opens the file again.
    pub(crate) fn first_byte(&self) -> u64 {
        self.top.line
    }

    /// The first byte of the text: after the byte order mark in Text.
    fn start(&self) -> u64 {
        match self.mode {
            Mode::Text => self.detected.data_start.min(self.blocks.len),
            Mode::Hex => 0,
        }
    }

    fn start_row(&self) -> RowAt {
        RowAt {
            line: self.start(),
            row: 0,
        }
    }

    /// Where a row after the file's last would start, from which End
    /// counts back.
    fn end_row(&self) -> RowAt {
        let len = self.blocks.len;
        let line = match self.mode {
            Mode::Text => len,
            Mode::Hex => viewer::hex_row_start(len.saturating_sub(1)) + HEX_ROW,
        };
        RowAt { line, row: 0 }
    }

    fn show_from(&mut self, at: RowAt) {
        self.top = at;
        self.rows.clear();
        self.after = After::Row(at);
        self.task = Task::Page;
    }

    /// Shows the page at once, but from the first line at or after
    /// `offset` as `mode`, after it has read what that needs, as a reopened
    /// file shows where it was.
    pub(crate) fn jump_as(&mut self, mode: Mode, offset: u64) {
        self.mode = mode;
        self.decoded = None;
        self.left = 0;
        self.rows.clear();
        self.after = After::End;
        self.task = Task::Jump { offset };
    }

    /// Fits the page to an area of `columns` characters and `height` rows,
    /// each at least 1, as [`text_room`] gives them.
    pub(crate) fn set_area(&mut self, columns: usize, height: usize) {
        if (columns, height) == (self.columns, self.height) {
            return;
        }
        // A wrapped line has other rows at another width.
        let rewrap = self.wrap && self.mode == Mode::Text && columns != self.columns;
        self.columns = columns;
        self.height = height;
        if rewrap {
            self.decoded = None;
            self.top.row = 0;
        }
        if self.task == Task::Page {
            self.show_from(self.top);
        }
    }

    /// Shows the file as `mode`, from the byte the page shows first.
    pub(crate) fn set_mode(&mut self, mode: Mode) {
        if mode != self.mode {
            self.jump_as(mode, self.top.line);
        }
    }

    pub(crate) fn set_wrap(&mut self, wrap: bool) {
        if wrap == self.wrap {
            return;
        }
        self.wrap = wrap;
        self.left = 0;
        if self.mode == Mode::Text {
            self.decoded = None;
            self.top.row = 0;
            if self.task == Task::Page {
                self.show_from(self.top);
            }
        }
    }

    /// Keeps the block `index` that the thread read.
    pub(crate) fn receive(&mut self, index: u64, bytes: Vec<u8>) {
        let len = self.blocks.len;
        self.blocks.keep(index, bytes);
        if self.blocks.len == len {
            return;
        }
        // The file shrank, so the rows built past its new end are wrong.
        self.decoded = None;
        match self.task {
            _ if self.top.line > self.blocks.len => self.jump_as(self.mode, self.blocks.len),
            Task::Page => self.show_from(self.top),
            // A move up from past the new end, such as End's, starts at
            // the new end instead.
            Task::Up { at, rows } if at.line > self.blocks.len => {
                self.task = Task::Up {
                    at: self.end_row(),
                    rows,
                };
            }
            Task::Idle | Task::Up { .. } | Task::Jump { .. } => {}
        }
    }

    /// Goes on with the page or the move as far as the kept blocks allow,
    /// and returns the block it waits for. `ansi` decodes bytes in
    /// Windows' ANSI code page.
    pub(crate) fn advance(&mut self, ansi: &dyn Fn(&[u8]) -> String) -> Option<u64> {
        loop {
            let waits = match self.task {
                Task::Idle => return None,
                Task::Jump { offset } => match self.jump_line(offset) {
                    Ok(line) => {
                        self.show_from(RowAt { line, row: 0 });
                        continue;
                    }
                    Err(block) => block,
                },
                Task::Up { at, rows: 0 } => {
                    self.show_from(at);
                    continue;
                }
                Task::Up { at, rows } => match self.row_before(at, ansi) {
                    Ok(Some(before)) => {
                        self.task = Task::Up {
                            at: before,
                            rows: rows - 1,
                        };
                        continue;
                    }
                    Ok(None) => {
                        self.show_from(at);
                        continue;
                    }
                    Err(block) => block,
                },
                Task::Page => match self.add_row(ansi) {
                    Ok(true) => continue,
                    Ok(false) => {
                        // Empty rows under the file's last: the top moves
                        // up until the last row is at the bottom, as after
                        // End. Each time it moves up, so this ends at the
                        // file's start at the latest.
                        let short = self.height.saturating_sub(self.rows.len());
                        if self.after == After::End && short > 0 && self.top != self.start_row() {
                            self.task = Task::Up {
                                at: self.top,
                                rows: short,
                            };
                            continue;
                        }
                        return None;
                    }
                    Err(block) => block,
                },
            };
            return Some(waits);
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.task = Task::Idle;
    }

    pub(crate) fn resume(&mut self) {
        if self.task == Task::Idle {
            self.show_from(self.top);
        }
    }

    /// Adds the next row to the page; `false` when the page is complete.
    fn add_row(&mut self, ansi: &dyn Fn(&[u8]) -> String) -> Need<bool> {
        let After::Row(at) = self.after else {
            return Ok(false);
        };
        if self.rows.len() >= self.height {
            return Ok(false);
        }
        let line = self.line_rows(at.line, ansi)?;
        let count = line.rows.len();
        if count == 0 {
            self.after = After::End;
            return Ok(true);
        }
        // A row that a narrower width took away is the line's last.
        let row = at.row.min(count - 1);
        let (text, muted) = (line.rows[row].clone(), line.muted);
        // Before the row is added, because it may wait for a block.
        let after = if row + 1 < count {
            After::Row(RowAt { row: row + 1, ..at })
        } else {
            match self.next_line(at.line)? {
                Some(line) => After::Row(RowAt { line, row: 0 }),
                None => After::End,
            }
        };
        let at = RowAt { row, ..at };
        if self.rows.is_empty() {
            self.top = at;
        }
        self.rows.push(Row { text, muted, at });
        self.after = after;
        Ok(true)
    }

    /// The rows of the line that starts at `line`.
    fn line_rows(&mut self, line: u64, ansi: &dyn Fn(&[u8]) -> String) -> Need<&LineRows> {
        if !matches!(&self.decoded, Some(decoded) if decoded.line == line) {
            let decoded = self.decode_line(line, ansi)?;
            self.decoded = Some(decoded);
        }
        Ok(self.decoded.as_ref().expect("the line was just decoded"))
    }

    fn decode_line(&self, line: u64, ansi: &dyn Fn(&[u8]) -> String) -> Need<LineRows> {
        let len = self.blocks.len;
        match self.mode {
            Mode::Hex => {
                if line >= len {
                    return Ok(LineRows {
                        line,
                        rows: Vec::new(),
                        muted: 0,
                    });
                }
                let bytes = self.blocks.bytes(line..(line + HEX_ROW).min(len))?;
                Ok(LineRows {
                    line,
                    rows: vec![viewer::hex_row(len, line, &bytes)],
                    muted: viewer::offset_text(len, line).len(),
                })
            }
            Mode::Text => {
                let end = self.line_end(line)?;
                let bytes = self.blocks.bytes(line..end)?;
                let decoded = viewer::decode_piece(self.detected.encoding, &bytes, ansi);
                let text = viewer::expand_tabs(viewer::without_terminator(&decoded));
                let rows = if self.wrap {
                    wrapped(text, self.columns)
                } else {
                    vec![text]
                };
                Ok(LineRows {
                    line,
                    rows,
                    muted: 0,
                })
            }
        }
    }

    /// Where the text line, or the piece of a long line, that starts at
    /// `line` ends.
    fn line_end(&self, line: u64) -> Need<u64> {
        let len = self.blocks.len;
        if line >= len {
            return Ok(line);
        }
        let end = line.saturating_add(LINE_BYTES as u64).min(len);
        let ahead = self.blocks.bytes(line..end)?;
        Ok(
            viewer::next_line_start(self.detected.encoding, &ahead, line, end == len)
                .expect("LINE_BYTES bytes, or the rest of the file, decide where a line ends"),
        )
    }

    /// The start of the line after the one at `line`; `None` when that is
    /// the file's last.
    fn next_line(&self, line: u64) -> Need<Option<u64>> {
        let next = match self.mode {
            Mode::Text => self.line_end(line)?,
            Mode::Hex => line + HEX_ROW,
        };
        let len = self.blocks.len;
        Ok((line < len && next < len).then_some(next))
    }

    /// The start of the line before the one at `line`; `None` at the start.
    fn previous_line(&self, line: u64) -> Need<Option<u64>> {
        match self.mode {
            Mode::Hex => Ok(line.checked_sub(HEX_ROW)),
            Mode::Text => {
                let start = self.start();
                let line = line.min(self.blocks.len);
                if line <= start {
                    return Ok(None);
                }
                let from = line.saturating_sub(LINE_BYTES as u64).max(start);
                let before = self.blocks.bytes(from..line)?;
                let previous =
                    viewer::previous_line_start(self.detected.encoding, start, &before, line);
                Ok((previous < line).then_some(previous))
            }
        }
    }

    /// The row before the one at `at`; `None` at the first.
    fn row_before(&mut self, at: RowAt, ansi: &dyn Fn(&[u8]) -> String) -> Need<Option<RowAt>> {
        if at.row > 0 {
            return Ok(Some(RowAt {
                row: at.row - 1,
                ..at
            }));
        }
        let Some(line) = self.previous_line(at.line)? else {
            return Ok(None);
        };
        let count = self.line_rows(line, ansi)?.rows.len();
        Ok(Some(RowAt {
            line,
            row: count.saturating_sub(1),
        }))
    }

    /// The line that a jump to `offset` shows first: in Text the first that
    /// starts at or after it, in Hex the row that holds it.
    fn jump_line(&self, offset: u64) -> Need<u64> {
        let len = self.blocks.len;
        match self.mode {
            Mode::Hex => Ok(viewer::hex_row_start(offset.min(len.saturating_sub(1)))),
            Mode::Text => {
                let start = self.start();
                let position = offset.clamp(start, len);
                let from = position.saturating_sub(LINE_BYTES as u64).max(start);
                let to = position.saturating_add(LINE_BYTES as u64).min(len);
                let window = self.blocks.bytes(from..to)?;
                Ok(viewer::line_at_or_after(
                    self.detected.encoding,
                    start,
                    &window,
                    from,
                    position,
                    to == len,
                )
                .unwrap_or(position))
            }
        }
    }

    /// Whether the page is complete, so a move starts from it. A move
    /// asked for meanwhile is dropped, because the page it starts from is
    /// not known yet; Home, End and a jump replace what runs.
    fn settled(&self) -> bool {
        self.task == Task::Page && (self.rows.len() >= self.height || self.after == After::End)
    }

    /// Whether the page shows the file's last row.
    fn end_shown(&self) -> bool {
        matches!(self.task, Task::Page | Task::Idle) && self.after == After::End
    }

    /// Moves down by `rows` rows, as far as the last row at the bottom.
    fn down(&mut self, rows: usize) {
        let After::Row(after) = self.after else {
            return;
        };
        if !self.settled() || rows == 0 {
            return;
        }
        if rows >= self.rows.len() {
            self.show_from(after);
        } else {
            self.rows.drain(..rows);
            self.top = self.rows[0].at;
        }
    }

    fn up(&mut self, rows: usize) {
        if self.settled() && rows > 0 && self.top != self.start_row() {
            self.task = Task::Up { at: self.top, rows };
        }
    }

    pub(crate) fn line_down(&mut self) {
        self.down(1);
    }

    pub(crate) fn line_up(&mut self) {
        self.up(1);
    }

    pub(crate) fn page_down(&mut self) {
        self.down(self.height);
    }

    pub(crate) fn page_up(&mut self) {
        self.up(self.height);
    }

    /// The file's start, at the left edge.
    pub(crate) fn home(&mut self) {
        self.left = 0;
        self.show_from(self.start_row());
    }

    /// The last page.
    pub(crate) fn end(&mut self) {
        self.task = Task::Up {
            at: self.end_row(),
            rows: self.height,
        };
    }

    /// The mouse wheel turned by `delta`, positive away from the person.
    pub(crate) fn wheel(&mut self, delta: i32) {
        self.wheel = self.wheel.saturating_add(delta);
        let rows = self.wheel / WHEEL_ROW;
        self.wheel -= rows * WHEEL_ROW;
        if rows > 0 {
            self.up(rows.unsigned_abs() as usize);
        } else if rows < 0 {
            self.down(rows.unsigned_abs() as usize);
        }
    }

    /// The scroll box dragged to `position`, which maps to a byte of the
    /// file.
    pub(crate) fn jump(&mut self, position: i32) {
        self.task = Task::Jump {
            offset: viewer::scroll_offset(self.blocks.len, position),
        };
    }

    /// Whether `Left` and `Right` scroll sideways: in Text while long lines
    /// do not wrap.
    fn sideways(&self) -> bool {
        self.mode == Mode::Text && !self.wrap
    }

    pub(crate) fn scroll_left(&mut self) {
        if self.sideways() {
            self.left = self.left.saturating_sub(1);
        }
    }

    /// One column to the right, while a shown row goes on past the area.
    pub(crate) fn scroll_right(&mut self) {
        let widest = self
            .rows
            .iter()
            .map(|row| row.text.chars().count())
            .max()
            .unwrap_or(0);
        if self.sideways() && self.left + self.columns < widest {
            self.left += 1;
        }
    }

    /// Each shown row from the first shown column, cut after the area's
    /// width, with how many of its first characters show in gray.
    pub(crate) fn shown(&self) -> Vec<(&str, usize)> {
        let skip = if self.sideways() { self.left } else { 0 };
        self.rows
            .iter()
            .map(|row| {
                let text = characters(&row.text, skip, self.columns + 1);
                (text, row.muted.saturating_sub(skip))
            })
            .collect()
    }

    /// The status bar's position: "Top", "37 %" or "End" in Text, the first
    /// row's offset in Hex.
    pub(crate) fn place(&self, strings: &Strings, numbers: &dyn Numbers) -> String {
        if self.mode == Mode::Hex {
            return viewer::offset_text(self.blocks.len, self.top.line);
        }
        // A wrapped row after a line's first is past that line's start.
        let first = self.top.line + u64::from(self.top.row > 0);
        match viewer::text_place(self.start(), self.blocks.len, first, self.end_shown()) {
            Place::Top => strings.text("viewer.top").to_owned(),
            Place::Percent(percent) => {
                strings.format("viewer.percent", &[&numbers.integer(u64::from(percent))])
            }
            Place::End => strings.text("viewer.end").to_owned(),
        }
    }

    /// The scroll bar, a position for each byte as
    /// [`viewer::scroll_position`] maps them; its page covers the whole
    /// range, which disables it, while the whole file shows.
    pub(crate) fn scroll_bar(&self) -> ScrollBar {
        let len = self.blocks.len;
        let max = (viewer::scroll_max(len) - 1).max(0);
        let position = viewer::scroll_position(len, self.top.line);
        let page = if self.top == self.start_row() && self.end_shown() {
            max.unsigned_abs() + 1
        } else {
            let shown_end = match self.after {
                After::Row(at) => at.line,
                After::End => len,
            };
            (viewer::scroll_position(len, shown_end) - position)
                .max(1)
                .unsigned_abs()
        };
        ScrollBar {
            max,
            page,
            position,
        }
    }
}

/// `text` in rows of `columns` characters.
fn wrapped(text: String, columns: usize) -> Vec<String> {
    if text.chars().nth(columns).is_none() {
        return vec![text];
    }
    let mut rows = Vec::new();
    let mut row = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 && i % columns == 0 {
            rows.push(std::mem::take(&mut row));
        }
        row.push(c);
    }
    rows.push(row);
    rows
}

/// The part of `text` from its character `skip` on, at most `take`
/// characters long.
fn characters(text: &str, skip: usize, take: usize) -> &str {
    let at = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let from = at(skip);
    let rest = &text[from..];
    &rest[..rest.char_indices().nth(take).map_or(rest.len(), |(i, _)| i)]
}

/// The area's vertical scroll bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScrollBar {
    /// The range is 0 to `max`.
    pub(crate) max: i32,
    pub(crate) page: u32,
    pub(crate) position: i32,
}

impl ScrollBar {
    /// While no file shows: disabled.
    pub(crate) const NONE: ScrollBar = ScrollBar {
        max: 0,
        page: 1,
        position: 0,
    };
}

/// What a Viewer shows in its area.
pub(crate) enum Shown {
    /// Until the first block arrives.
    Reading,
    Cancelled,
    File(Document),
    /// Windows could not open the file, or, when `opened`, read it.
    Failed {
        opened: bool,
        reason: String,
    },
    Removing,
    Removed,
}

impl Shown {
    pub(crate) fn document(&self) -> Option<&Document> {
        match self {
            Shown::File(document) => Some(document),
            _ => None,
        }
    }

    pub(crate) fn document_mut(&mut self) -> Option<&mut Document> {
        match self {
            Shown::File(document) => Some(document),
            _ => None,
        }
    }

    /// The message the area shows instead of the file `name`, and whether
    /// Retry shows under it.
    pub(crate) fn message(&self, strings: &Strings, name: &str) -> Option<(String, bool)> {
        Some(match self {
            Shown::File(_) => return None,
            Shown::Reading => (strings.text("panel.reading").to_owned(), false),
            Shown::Cancelled => (strings.text("work.cancelled").to_owned(), true),
            Shown::Failed { opened, reason } => {
                let key = if *opened {
                    "viewer.cannot_read"
                } else {
                    "panel.cannot_open"
                };
                (strings.format(key, &[name, reason.as_str()]), true)
            }
            Shown::Removing => (strings.text("viewer.removing").to_owned(), true),
            Shown::Removed => (strings.text("viewer.removed").to_owned(), true),
        })
    }
}

/// The status bar's position part at its widest, which sets its width:
/// "Top", "End", "99 %" or a 16-digit offset.
pub(crate) fn place_samples(strings: &Strings, numbers: &dyn Numbers) -> [String; 4] {
    [
        strings.text("viewer.top").to_owned(),
        strings.text("viewer.end").to_owned(),
        strings.format("viewer.percent", &[&numbers.integer(99)]),
        viewer::offset_text(u64::MAX, 0),
    ]
}

/// The status bar's left part: the file's name, its encoding in Text and
/// its size, or only the name while no file shows.
pub(crate) fn file_text(
    strings: &Strings,
    numbers: &dyn Numbers,
    name: &str,
    document: Option<&Document>,
) -> String {
    let Some(document) = document else {
        return name.to_owned();
    };
    let size = text::bytes(strings, numbers, document.blocks.len);
    match document.mode {
        Mode::Text => strings.format(
            "viewer.status_text",
            &[name, document.detected.encoding.name(), size.as_str()],
        ),
        Mode::Hex => strings.format("viewer.status_hex", &[name, size.as_str()]),
    }
}

/// The space between the area's edges and its text, in pixels at 96 DPI,
/// as the prototype draws it.
const INSET: (i32, i32) = (8, 4);

/// The space before the text and above its first row in the area.
pub(crate) fn text_inset(dpi: Dpi) -> (Px, Px) {
    let at = |px: i32| Px(px).rescale(Dpi::new(96).expect("96 is not zero"), dpi);
    (at(INSET.0), at(INSET.1))
}

pub(crate) fn waiting_layout(
    area: Rect,
    text_height: Px,
    button: Size,
    dpi: Dpi,
) -> (Rect, Rect, Rect) {
    let (x, y) = text_inset(dpi);
    let width = button.width.min((area.width - x - x).max(Px(0)));
    let height = text_height.max(button.height);
    let message = Rect {
        x: area.x + x,
        y: area.y + y,
        width: (area.width - width - x - x - x).max(Px(0)),
        height,
    };
    let button = Rect {
        x: area.right() - x - width,
        y: area.y + y,
        width,
        height: button.height,
    };
    let top = (message.bottom() + y).min(area.bottom());
    let page = Rect {
        y: top,
        height: (area.bottom() - top).max(Px(0)),
        ..area
    };
    (page, message, button)
}

/// The characters and the whole rows that fit an area of `size` inside
/// its inset, in a font whose characters are `char_width` wide and whose
/// rows are `line_height` tall.
pub(crate) fn text_room(size: Size, char_width: Px, line_height: Px, dpi: Dpi) -> (usize, usize) {
    let (x, y) = text_inset(dpi);
    let fit = |room: Px, unit: Px| usize::try_from(room.0 / unit.0.max(1)).unwrap_or(0).max(1);
    (
        fit(size.width - x - x, char_width),
        fit(size.height - y - y, line_height),
    )
}

/// A command of the Viewer's menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerCommand {
    Close,
    Text,
    Hex,
    Wrap,
}

impl ViewerCommand {
    const ALL: [ViewerCommand; 4] = [
        ViewerCommand::Close,
        ViewerCommand::Text,
        ViewerCommand::Hex,
        ViewerCommand::Wrap,
    ];

    /// The first `WM_COMMAND` ID, above `IDOK` and `IDCANCEL`, which `Enter`
    /// and `Esc` send through `IsDialogMessageW`.
    const FIRST_ID: u16 = 0x100;

    /// Its `WM_COMMAND` ID in the Viewer's window.
    pub(crate) fn id(self) -> u16 {
        let index = Self::ALL
            .iter()
            .position(|command| *command == self)
            .expect("ALL holds every command");
        Self::FIRST_ID + u16::try_from(index).expect("the list is short")
    }

    pub(crate) fn from_id(id: u16) -> Option<ViewerCommand> {
        let index = id.checked_sub(Self::FIRST_ID)?;
        Self::ALL.get(usize::from(index)).copied()
    }

    fn label(self) -> &'static str {
        match self {
            ViewerCommand::Close => "viewer.close",
            ViewerCommand::Text => "viewer.text",
            ViewerCommand::Hex => "viewer.hex",
            ViewerCommand::Wrap => "viewer.wrap",
        }
    }

    /// The key the menu shows: `F4` switches between Text and Hex.
    fn key(self) -> Key {
        match self {
            ViewerCommand::Close => Key::Escape,
            ViewerCommand::Text | ViewerCommand::Hex => Key::Function(4),
            ViewerCommand::Wrap => Key::Function(2),
        }
    }

    fn mark(self) -> Mark {
        match self {
            ViewerCommand::Close => Mark::None,
            ViewerCommand::Text | ViewerCommand::Hex => Mark::Radio,
            ViewerCommand::Wrap => Mark::Check,
        }
    }

    fn item(self, strings: &Strings) -> MenuNode {
        MenuNode::Item {
            id: self.id(),
            text: menu_label(
                strings.text(self.label()),
                Some(Keystroke::plain(self.key()).text(strings)),
            ),
            mark: self.mark(),
            glyph: (self == ViewerCommand::Close).then_some(Glyph::Cancel),
        }
    }
}

/// The Viewer's menu bar: File: Close. View: Text, Hex | Wrap long lines.
pub(crate) fn menu_bar(strings: &Strings) -> Vec<MenuNode> {
    let menu = |label: &str, children| MenuNode::Submenu {
        text: strings.text(label).to_owned(),
        children,
        aligned_right: false,
    };
    vec![
        menu("menu.file", vec![ViewerCommand::Close.item(strings)]),
        menu(
            "menu.view",
            vec![
                ViewerCommand::Text.item(strings),
                ViewerCommand::Hex.item(strings),
                MenuNode::Separator,
                ViewerCommand::Wrap.item(strings),
            ],
        ),
    ]
}

/// Each menu item's ID, whether it is enabled and whether it is checked,
/// while the Viewer shows a file as `mode`, or none yet, and wraps long
/// lines when `wrap`. Wrap long lines is disabled in Hex, because a hex row
/// is always 16 bytes.
pub(crate) fn menu_states(mode: Option<Mode>, wrap: bool) -> [(u16, bool, bool); 4] {
    ViewerCommand::ALL.map(|command| {
        let (enabled, checked) = match command {
            ViewerCommand::Close => (true, false),
            ViewerCommand::Text => (mode.is_some(), mode == Some(Mode::Text)),
            ViewerCommand::Hex => (mode.is_some(), mode == Some(Mode::Hex)),
            ViewerCommand::Wrap => (mode != Some(Mode::Hex), wrap),
        };
        (command.id(), enabled, checked)
    })
}

/// What a key does in a Viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewerKey {
    /// `Esc` cancels a pending read, and otherwise closes the Viewer.
    Cancel,
    /// `F3`, as in Norton Commander.
    Close,
    /// `F4`: Text to Hex, or Hex to Text.
    SwitchMode,
    /// `F2`: Wrap long lines on or off.
    Wrap,
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Home,
    End,
    Left,
    Right,
}

/// The Viewer's key that `keystroke` presses, if any.
pub(crate) fn viewer_key(keystroke: Keystroke) -> Option<ViewerKey> {
    if keystroke.alt || keystroke.shift {
        return None;
    }
    Some(match (keystroke.key, keystroke.ctrl) {
        (Key::Escape, false) => ViewerKey::Cancel,
        (Key::Function(3), false) => ViewerKey::Close,
        (Key::Function(4), false) => ViewerKey::SwitchMode,
        (Key::Function(2), false) => ViewerKey::Wrap,
        (Key::Up, false) => ViewerKey::LineUp,
        (Key::Down, false) => ViewerKey::LineDown,
        (Key::PageUp, false) => ViewerKey::PageUp,
        (Key::PageDown, false) => ViewerKey::PageDown,
        (Key::Home, _) => ViewerKey::Home,
        (Key::End, _) => ViewerKey::End,
        (Key::Left, false) => ViewerKey::Left,
        (Key::Right, false) => ViewerKey::Right,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| char::from(b)).collect()
    }

    /// Gives `document` the blocks of `file` it asks for until its page is
    /// complete, and returns the blocks it asked for.
    fn settle(document: &mut Document, file: &[u8]) -> Vec<u64> {
        let mut read = Vec::new();
        while let Some(index) = document.advance(&latin1) {
            assert!(read.len() < 1_000, "the page keeps asking for blocks");
            read.push(index);
            let start = usize::try_from(index).unwrap() * BLOCK_BYTES;
            let end = (start + BLOCK_BYTES).min(file.len());
            document.receive(index, file.get(start..end).unwrap_or_default().to_vec());
        }
        read
    }

    fn open(file: &[u8], wrap: bool, columns: usize, height: usize) -> Document {
        let first = file[..file.len().min(BLOCK_BYTES)].to_vec();
        let mut document = Document::new(file.len() as u64, first, wrap);
        document.set_area(columns, height);
        settle(&mut document, file);
        document
    }

    fn texts(document: &Document) -> Vec<String> {
        document.rows.iter().map(|row| row.text.clone()).collect()
    }

    #[test]
    fn a_page_that_needs_more_blocks_than_are_kept_reads_each_once() {
        // 120 lines of 10,000 characters: about 19 blocks for one page.
        let file = format!("{}\n", "x".repeat(10_000)).repeat(120).into_bytes();
        let first = file[..BLOCK_BYTES].to_vec();
        let mut document = Document::new(file.len() as u64, first, false);
        document.set_area(80, 120);
        let read = settle(&mut document, &file);
        assert!(read.windows(2).all(|pair| pair[0] < pair[1]), "{read:?}");
        assert_eq!(document.rows.len(), 120);
        assert!(document.end_shown());
    }

    #[test]
    fn up_returns_through_wrapped_rows_to_where_down_came_from() {
        let file = format!("{}\n{}\n{}", "a".repeat(25), "b".repeat(5), "c".repeat(30));
        let mut document = open(file.as_bytes(), true, 10, 3);
        let first = texts(&document);
        assert_eq!(first, ["a".repeat(10), "a".repeat(10), "a".repeat(5)]);
        // Seven rows, three shown: four moves reach the last page, and a
        // fifth moves no further.
        for _ in 0..5 {
            document.line_down();
            settle(&mut document, file.as_bytes());
        }
        assert_eq!(texts(&document), vec!["c".repeat(10); 3]);
        assert!(document.end_shown());
        for _ in 0..4 {
            document.line_up();
            settle(&mut document, file.as_bytes());
        }
        assert_eq!(texts(&document), first);
        assert_eq!(document.top, document.start_row());
    }

    #[test]
    fn end_and_a_jump_near_the_end_show_a_full_last_page() {
        let file: String = (0..10).map(|n| format!("line{n}\r\n")).collect();
        let last = ["line6", "line7", "line8", "line9"];
        let mut document = open(file.as_bytes(), true, 20, 4);
        document.end();
        settle(&mut document, file.as_bytes());
        assert_eq!(texts(&document), last);
        document.home();
        let len = file.len() as u64;
        document.jump(viewer::scroll_position(len, len - 3));
        settle(&mut document, file.as_bytes());
        assert_eq!(texts(&document), last);
        // A file that the area holds whole disables the scroll bar.
        let small = open(b"one\ntwo", true, 20, 4);
        let bar = small.scroll_bar();
        assert!(bar.page > bar.max.unsigned_abs());
    }

    #[test]
    fn a_file_shorter_than_its_size_ends_where_its_bytes_end() {
        // The size said 200,000 bytes, but only 65,546 can be read.
        let file = "x\n".repeat(32_773).into_bytes();
        let first = file[..BLOCK_BYTES].to_vec();
        let mut document = Document::new(200_000, first, true);
        document.set_area(20, 3);
        settle(&mut document, &file);
        document.end();
        settle(&mut document, &file);
        assert_eq!(document.blocks.len, file.len() as u64);
        assert_eq!(texts(&document), ["x"; 3]);
        assert!(document.end_shown());
    }

    #[test]
    fn cancelled_move_does_not_request_its_block_again_after_layout() {
        let mut document = Document::new((BLOCK_BYTES * 3) as u64, vec![0; BLOCK_BYTES], false);
        document.set_area(80, 3);
        assert_eq!(document.advance(&latin1), None);
        document.end();
        assert_eq!(document.advance(&latin1), Some(2));
        document.cancel();
        document.set_area(80, 5);
        assert_eq!(document.advance(&latin1), None);
        document.end();
        assert_eq!(document.advance(&latin1), Some(2));
    }
}
