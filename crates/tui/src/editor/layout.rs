/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use std::ops::Range;

use ratatui::layout::{Position, Rect};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// One source of truth for rendering, vertical movement and pointer hit-testing.
/// Bytes identify text; columns identify terminal cells. Never interchange them.
#[derive(Default)]
pub(super) struct Layout {
    pub rows: Vec<Row>,
    pub boundaries: Vec<usize>,
    pub width: u16,
}

pub(super) struct Row {
    pub start: usize,
    pub end: usize,
    pub cells: Vec<Cell>,
}

pub(super) struct Cell {
    pub bytes: Range<usize>,
    pub column: u16,
    pub width: u16,
}

impl Layout {
    pub fn new(text: &str, width: u16) -> Self {
        // Keep a cell for the insertion cursor even on a full logical line.
        let wrap = width.saturating_sub(1).max(1);
        let mut rows = Vec::new();
        let mut row = Row {
            start: 0,
            end: 0,
            cells: Vec::new(),
        };
        let mut column = 0;
        let mut boundaries = Vec::new();
        for (byte, grapheme) in text.grapheme_indices(true) {
            boundaries.push(byte);
            if grapheme == "\n" {
                row.end = byte;
                rows.push(row);
                row = Row {
                    start: byte + 1,
                    end: byte + 1,
                    cells: Vec::new(),
                };
                column = 0;
                continue;
            }
            let cell_width = |column: u16| {
                if grapheme == "\t" {
                    4 - column % 4
                } else {
                    grapheme.width().max(1).min(u16::MAX as usize) as u16
                }
                .min(wrap)
            };
            let mut size = cell_width(column);
            if column + size > wrap {
                row.end = byte;
                rows.push(row);
                row = Row {
                    start: byte,
                    end: byte,
                    cells: Vec::new(),
                };
                column = 0;
                size = cell_width(column);
            }
            row.cells.push(Cell {
                bytes: byte..byte + grapheme.len(),
                column,
                width: size,
            });
            column += size;
            row.end = byte + grapheme.len();
        }
        boundaries.push(text.len());
        rows.push(row);
        Self {
            rows,
            boundaries,
            width,
        }
    }

    pub fn cursor(&self, byte: usize, upstream: bool) -> (usize, u16) {
        let mut index = self
            .rows
            .partition_point(|row| row.start <= byte)
            .saturating_sub(1);
        if upstream && index > 0 && self.rows[index - 1].end == byte {
            index -= 1;
        }
        let row = &self.rows[index];
        let column = row
            .cells
            .iter()
            .find(|cell| cell.bytes.start >= byte)
            .map_or_else(
                || row.cells.last().map_or(0, |cell| cell.column + cell.width),
                |cell| cell.column,
            );
        (index, column)
    }

    pub fn byte_at(&self, row: usize, column: u16) -> usize {
        let row = &self.rows[row.min(self.rows.len() - 1)];
        for cell in &row.cells {
            if column <= cell.column + cell.width / 2 {
                return cell.bytes.start;
            }
        }
        row.end
    }

    pub fn hit(&self, row: usize, column: u16) -> (usize, bool) {
        let row = row.min(self.rows.len() - 1);
        let byte = self.byte_at(row, column);
        (byte, self.cursor(byte, false).0 != row)
    }

    pub fn pointer(&self, area: Rect, top: usize, point: Position) -> (usize, bool) {
        let row = point.y.clamp(area.y, area.bottom().saturating_sub(1)) - area.y;
        let column = point
            .x
            .saturating_sub(area.x)
            .min(area.width.saturating_sub(1));
        self.hit(top + row as usize, column)
    }

    pub fn previous(&self, byte: usize) -> usize {
        let index = self.boundaries.partition_point(|b| *b < byte);
        self.boundaries[index.saturating_sub(1)]
    }

    pub fn next(&self, byte: usize) -> usize {
        let index = self.boundaries.partition_point(|b| *b <= byte);
        self.boundaries[index.min(self.boundaries.len() - 1)]
    }

    /// Inserting/deleting text can merge adjacent graphemes. Snap to the right,
    /// rather than leaving a cursor inside a newly formed emoji/combining sequence.
    pub fn snap_right(&self, byte: usize) -> usize {
        let index = self.boundaries.partition_point(|b| *b < byte);
        self.boundaries[index.min(self.boundaries.len() - 1)]
    }
}
