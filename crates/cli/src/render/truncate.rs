//! Width-budgeted source-line truncation for human output.
//!
//! This module exists because codespan-reporting has no width concept: its
//! `term::Config` covers only styles and label context lines, so a source
//! line wider than the terminal soft-wraps and drags its caret marks away
//! from the flagged word. The fix is a truncated copy of the source whose
//! label ranges are remapped into the copy's coordinates before the copy is
//! registered with `SimpleFiles` — marks and text move together.
//!
//! THE invariant: for every span byte the renderer remaps, the byte at the
//! remapped offset in [`Truncated::copy`] is the byte at the original offset
//! in the source. Codespan computes caret columns by subtracting line-range
//! starts from label ranges, so a copy whose ranges point anywhere else
//! would draw marks into the void.
//!
//! The module is pure: no I/O, no environment reads. The same
//! `(src, span, budget)` always yields the same [`Truncated`].

use unicode_segmentation::UnicodeSegmentation;

/// Terminal cells reserved as slack so the window is not flush against the
/// right edge of the terminal.
const PADDING_CELLS: usize = 4;

/// Below this budget a truncating renderer would produce more marker than
/// content; lines pass through untouched instead.
const MIN_BUDGET: usize = 10;

/// codespan's default tab width; used to expand tabs into cells so caret
/// columns computed by codespan match the cell counts computed here.
const TAB_WIDTH: usize = 4;

/// Marker placed where the line was cut.
const ELLIPSIS: &str = "…";
const ELLIPSIS_CELLS: usize = 1;
const ELLIPSIS_BYTES: usize = ELLIPSIS.len();

/// The outcome of building a width-budgeted copy of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    /// Rebuilt source: one window (or the original line) per source line,
    /// `'\n'`-joined, trailing `'\r'` stripped. A final `'\n'` is appended
    /// even when the source lacks one.
    pub copy: String,
    /// Remapped span start byte offset into [`Truncated::copy`].
    pub start: usize,
    /// Remapped span end byte offset into [`Truncated::copy`]; always
    /// `>=` [`Truncated::start`].
    pub end: usize,
}

/// One grapheme cluster of a line: its byte offset relative to the line's
/// display content, its byte length, and its width in terminal cells.
struct Cluster {
    offset: usize,
    len: usize,
    cells: usize,
}

/// A source line as the windowing algorithm sees it: its original byte
/// range in the source, its display content (trailing `'\r'` stripped),
/// and its grapheme clusters.
struct LineLayout<'a> {
    /// Byte offset of the line's first byte in the source.
    start: usize,
    /// Display content with any trailing `'\r'` removed.
    content: &'a str,
    clusters: Vec<Cluster>,
    /// Sum of cluster widths in terminal cells.
    cells: usize,
}

impl LineLayout<'_> {
    /// Lay out the raw line `raw` whose first byte sits at `line_start`.
    fn parse(line_start: usize, raw: &str) -> LineLayout<'_> {
        let content = raw.strip_suffix('\r').unwrap_or(raw);
        let mut clusters = Vec::new();
        let mut cells = 0;
        for (offset, grapheme) in content.grapheme_indices(true) {
            let width = cluster_cells(grapheme, cells);
            clusters.push(Cluster {
                offset,
                len: grapheme.len(),
                cells: width,
            });
            cells += width;
        }
        LineLayout {
            start: line_start,
            content,
            clusters,
            cells,
        }
    }

    /// Byte length of the display content.
    fn byte_len(&self) -> usize {
        self.content.len()
    }

    /// Whether the line is the empty tail after a trailing newline.
    fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Width in cells of clusters `from..=to`.
    fn cells_between(&self, from: usize, to: usize) -> usize {
        self.clusters[from..=to].iter().map(|c| c.cells).sum()
    }

    /// Index of the cluster containing content-relative byte offset `rel`.
    /// The last cluster answers for `rel == byte_len` (a span end resting on
    /// the line boundary).
    fn cluster_at(&self, rel: usize) -> usize {
        self.clusters
            .iter()
            .position(|c| rel < c.offset + c.len)
            .unwrap_or(self.clusters.len().saturating_sub(1))
    }
}

/// Display width of one grapheme cluster; `column` is the tab-stop column
/// of the cluster's first cell (only tabs consume it), mirroring the
/// expansion codespan's renderer applies.
fn cluster_cells(grapheme: &str, column: usize) -> usize {
    match grapheme {
        "\t" => TAB_WIDTH - (column % TAB_WIDTH),
        other => unicode_width::UnicodeWidthStr::width(other),
    }
}

/// How many terminal cells a source line may occupy in a spanned excerpt.
///
/// THE budget contract: a rendered source line costs `line_digits + 3`
/// gutter cells (`{n:>digits} │ `), plus two cells per multi-line label
/// column in codespan's inner gutter (`╭ `/`│ `), plus [`PADDING_CELLS`] of
/// slack; the remainder is the source-line budget. Anchorless lines cost one
/// more cell for their gutter (`number, ` │ `, `= ` note bullet).
///
/// The result never drops below [`MIN_BUDGET`] so tiny widths cannot
/// produce marker-only lines.
#[must_use]
pub fn budget_spanned(width: usize, line_digits: usize, num_multi_labels: usize) -> usize {
    let gutter = line_digits + 3 + 2 * num_multi_labels;
    width
        .saturating_sub(gutter + PADDING_CELLS)
        .max(MIN_BUDGET)
        .min(width.max(MIN_BUDGET))
}

/// How many terminal cells an anchorless excerpt line may occupy
/// (`digits + 4` gutter: number, ` │ `, one-cell note bullet).
#[must_use]
pub fn budget_anchorless(width: usize, line_digits: usize) -> usize {
    let gutter = line_digits + 4;
    width
        .saturating_sub(gutter + PADDING_CELLS)
        .max(MIN_BUDGET)
        .min(width.max(MIN_BUDGET))
}

/// Build a width-budgeted copy of `src` with the span's anchors kept
/// visible.
///
/// Lines within `budget` pass through byte-identically (minus any trailing
/// `'\r'`); wider lines get a sliding window anchored on the span, cut on
/// each side with `…`. The returned `start`/`end` are the span's remapped
/// byte offsets in [`Truncated::copy`], both inside the visible window.
///
/// Out-of-range span bounds are clamped to the source; a span that touches
/// no line (possible only for an empty source) remaps to `0..0`.
#[must_use]
pub fn truncate_source(src: &str, span_start: usize, span_end: usize, budget: usize) -> Truncated {
    let span_end = span_end.max(span_start).min(src.len());
    let span_start = span_start.min(src.len());

    let layouts: Vec<LineLayout<'_>> = src
        .split('\n')
        .scan(0usize, |offset, raw| {
            let start = *offset;
            *offset += raw.len() + 1;
            Some(LineLayout::parse(start, raw))
        })
        .collect();

    let mut copy = String::with_capacity(src.len());
    let mut start = None;
    let mut end = None;
    // The last layout from `split('\n')` is the empty tail after a trailing
    // newline; render only real lines so the copy never grows a second,
    // spurious `'\n'`.
    let real_lines = layouts.len() - usize::from(layouts.last().is_some_and(LineLayout::is_empty));

    for layout in &layouts[..real_lines] {
        let line_end = layout.start + layout.byte_len();
        let touched = span_start < line_end && span_end > layout.start;
        let needs_window = layout.cells > budget && budget >= MIN_BUDGET;
        let anchor = match (touched, needs_window) {
            (true, true) => {
                let first = span_start.max(layout.start);
                let last = span_end.min(line_end) - 1;
                Some((
                    layout.cluster_at(first - layout.start),
                    layout.cluster_at(last - layout.start),
                ))
            }
            _ => None,
        };
        let window = Window::build(layout, anchor, budget);
        let copy_line_start = copy.len();
        if touched {
            if span_start >= layout.start {
                start.get_or_insert(window.remap(layout, copy_line_start, span_start));
            }
            if span_end > layout.start && span_end <= line_end {
                end.get_or_insert(window.remap(layout, copy_line_start, span_end));
            }
        }
        copy.push_str(&window.text);
        copy.push('\n');
    }

    Truncated {
        copy,
        start: start.unwrap_or(0),
        end: end.unwrap_or(0),
    }
}

/// Head-truncate a single display line to `budget` cells, appending `…`
/// when the line is cut. Used for anchorless excerpt lines, which have no
/// span to anchor a window on.
#[must_use]
pub fn truncate_line_head(line: &str, budget: usize) -> String {
    let layout = LineLayout::parse(0, line);
    if budget < MIN_BUDGET || layout.cells <= budget {
        return layout.content.to_owned();
    }
    Window::build(&layout, None, budget).text
}

/// A rendered line: its display text plus everything the offset remap needs.
struct Window {
    text: String,
    /// `Some((from, to))` cluster range when the line was rebuilt from a
    /// window; `None` when the content passes through verbatim.
    cut: Option<(usize, usize)>,
    /// Byte offset of the first visible cluster within the layout's clusters.
    first_offset: usize,
    /// Byte offsets of every cluster in the layout.
    offsets: Vec<usize>,
}

impl Window {
    /// Build the window for one line.
    ///
    /// Without an anchor the line either passes through or gets a head
    /// window. With an anchor the span stays visible: when the anchor alone
    /// exceeds the budget its head is kept; otherwise slack is distributed
    /// around it — half (rounded up) to the left, the rest right, then any
    /// remainder back to the left.
    fn build(layout: &LineLayout<'_>, anchor: Option<(usize, usize)>, budget: usize) -> Window {
        let last = layout.clusters.len().saturating_sub(1);
        let Some((gs, ge)) = anchor else {
            if layout.cells <= budget || budget < MIN_BUDGET {
                return Window::passthrough(layout);
            }
            // Head window: keep one cell for the right marker.
            let mut used = 0;
            let mut to = 0;
            for (index, cluster) in layout.clusters.iter().enumerate() {
                if used + cluster.cells > budget - ELLIPSIS_CELLS {
                    break;
                }
                used += cluster.cells;
                to = index;
            }
            return Window::cut(layout, 0, to);
        };

        let left_cut = gs > 0;
        let right_cut = ge < last;
        let markers =
            usize::from(left_cut) * ELLIPSIS_CELLS + usize::from(right_cut) * ELLIPSIS_CELLS;
        let avail = budget.saturating_sub(markers);
        let (wa, wb) = if layout.cells_between(gs, ge) > avail {
            // The anchor alone overflows: keep its head, and remember the
            // right marker still consumes a cell of the anchor budget.
            let mut wb = gs;
            while wb < ge
                && layout.cells_between(gs, wb + 1) <= avail.saturating_sub(ELLIPSIS_CELLS)
            {
                wb += 1;
            }
            (gs, wb)
        } else {
            expand_around_anchor(layout, gs, ge, avail)
        };
        Window::cut(layout, wa, wb)
    }

    /// The original content, byte-identical.
    fn passthrough(layout: &LineLayout<'_>) -> Window {
        Window {
            text: layout.content.to_owned(),
            cut: None,
            first_offset: 0,
            offsets: layout.clusters.iter().map(|c| c.offset).collect(),
        }
    }

    /// Clusters `from..=to` with `…` on each cut side.
    fn cut(layout: &LineLayout<'_>, from: usize, to: usize) -> Window {
        let last = layout.clusters.len().saturating_sub(1);
        let mut text = String::new();
        if from > 0 {
            text.push_str(ELLIPSIS);
        }
        for cluster in &layout.clusters[from..=to] {
            text.push_str(&layout.content[cluster.offset..cluster.offset + cluster.len]);
        }
        if to < last {
            text.push_str(ELLIPSIS);
        }
        Window {
            text,
            cut: Some((from, to)),
            first_offset: layout.clusters[from].offset,
            offsets: layout.clusters.iter().map(|c| c.offset).collect(),
        }
    }

    /// Remap an original byte offset on this line into copy coordinates.
    fn remap(&self, layout: &LineLayout<'_>, copy_line_start: usize, original: usize) -> usize {
        let rel = original.saturating_sub(layout.start).min(layout.byte_len());
        let Some((from, to)) = self.cut else {
            return copy_line_start + rel;
        };
        let marker = usize::from(from > 0) * ELLIPSIS_BYTES;
        let prefix_bytes = |index: usize| self.offsets[index] - self.first_offset;
        let cluster = layout.cluster_at(rel);
        let position = match cluster {
            c if c < from => 0,
            c if c > to => prefix_bytes(to) + layout.clusters[to].len,
            c => {
                let within = rel.saturating_sub(self.offsets[c]);
                prefix_bytes(c) + within.min(layout.clusters[c].len)
            }
        };
        copy_line_start + marker + position
    }
}

/// Distribute the cells left over after fitting the anchor around it: half
/// rounded up to the left, the remainder right, then whatever the right
/// side could not absorb back to the left. Returns the window range.
fn expand_around_anchor(
    layout: &LineLayout<'_>,
    gs: usize,
    ge: usize,
    avail: usize,
) -> (usize, usize) {
    let last = layout.clusters.len().saturating_sub(1);
    let mut wa = gs;
    let mut wb = ge;
    let mut free = avail - layout.cells_between(gs, ge);

    let mut left_share = free / 2 + free % 2;
    while wa > 0 && layout.clusters[wa - 1].cells <= left_share {
        left_share -= layout.clusters[wa - 1].cells;
        free -= layout.clusters[wa - 1].cells;
        wa -= 1;
    }
    while wb < last && layout.clusters[wb + 1].cells <= free {
        free -= layout.clusters[wb + 1].cells;
        wb += 1;
    }
    while wa > 0 && layout.clusters[wa - 1].cells <= free {
        free -= layout.clusters[wa - 1].cells;
        wa -= 1;
    }
    (wa, wb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn short_line_passes_through_byte_identical() {
        // Given a short line with a span on its first word.
        let src = "hello world\n";

        // When truncating with a budget far above the line's width.
        let got = truncate_source(src, 0, 5, 40);

        // Then the copy is byte-identical and the span remaps to itself.
        assert_eq!(got.copy, "hello world\n");
        assert_eq!(&got.copy[got.start..got.end], "hello");
    }

    #[test]
    fn long_line_cuts_both_sides_and_marks_the_word() {
        // Given a line wider than the budget with a span mid-line.
        let src = "alpha beta gamma delta epsilon\n";

        // When truncating to a ten-cell budget.
        let got = truncate_source(src, 11, 16, 10);

        // Then both sides carry the marker and the remapped span slices
        // exactly the flagged word.
        assert!(got.copy.starts_with(ELLIPSIS));
        assert!(got.copy.trim_end_matches('\n').ends_with(ELLIPSIS));
        assert_eq!(&got.copy[got.start..got.end], "gamma");
        // And the visible line fits the budget in cells.
        let visible = got.copy.trim_end_matches('\n');
        assert_eq!(visible.width(), 10);
    }

    #[test]
    fn span_near_line_start_keeps_head_and_cuts_right() {
        // Given a span at the very start of a line wider than the budget.
        let src = "alpha beta gamma delta epsilon\n";

        // When truncating to a ten-cell budget.
        let got = truncate_source(src, 0, 5, 10);

        // Then the window hugs the head: no left marker, span intact.
        assert!(!got.copy.starts_with(ELLIPSIS));
        assert_eq!(&got.copy[got.start..got.end], "alpha");
        assert!(got.copy.starts_with("alpha bet"));
        assert!(got.copy.trim_end_matches('\n').ends_with(ELLIPSIS));
    }

    #[test]
    fn span_wider_than_budget_anchors_at_span_start() {
        // Given a single long word that alone exceeds the budget.
        let src = "supercalifragilisticexpialidocious\n";

        // When truncating to a ten-cell budget.
        let got = truncate_source(src, 0, 34, 10);

        // Then the window starts at the span head and still shows it.
        assert_eq!(got.copy, "supercali…\n");
        assert_eq!(&got.copy[got.start..got.end], "supercali");
    }

    #[test]
    fn wide_chars_count_double_cells() {
        // Given a line of CJK characters (two cells each) with a span on
        // the fourth character.
        let src = "一二三四五六七八九十\n";

        // When truncating to a ten-cell budget.
        let got = truncate_source(src, 9, 12, 10);

        // Then the cut respects cells, not chars: the visible line is
        // exactly the budget wide and the flagged character survives whole.
        let visible = got.copy.trim_end_matches('\n');
        assert_eq!(visible.width(), 10);
        assert_eq!(&got.copy[got.start..got.end], "四");
    }

    #[test]
    fn grapheme_cluster_is_never_split() {
        // Given a line of flag emoji (one grapheme, two cells, eight bytes
        // each) with a span on the fourth flag.
        let src = "🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸🇺🇸\n";

        // When truncating to a ten-cell budget.
        let got = truncate_source(src, 24, 32, 10);

        // Then the remapped span opens exactly on a flag boundary.
        assert_eq!(&got.copy[got.start..got.start + 8], "🇺🇸");
        // And only whole flags are visible.
        assert_eq!(got.copy.matches("🇺🇸").count(), 4);
    }

    #[test]
    fn tab_expansion_keeps_span_offset() {
        // Given a line whose leading tabs expand to eight cells before the
        // flagged word.
        let src = "\t\tleverage\n";

        // When truncating to a twelve-cell budget.
        let got = truncate_source(src, 2, 10, 12);

        // Then the remapped span still slices the word despite the tabs.
        assert_eq!(got.copy, "…leverage\n");
        assert_eq!(&got.copy[got.start..got.end], "leverage");
    }

    #[test]
    fn multiline_span_remaps_start_and_end_lines_independently() {
        // Given a span from a short first line into a long second line.
        let src = "aaa bbb\n0123456789ABCDEFGHIJKLMNOPQRST\n";

        // When truncating to a twelve-cell budget.
        let got = truncate_source(src, 4, 23, 12);

        // Then the start survives on the untouched first line.
        assert_eq!(&got.copy[got.start..got.start + 3], "bbb");
        // And the end lands inside the truncated second line.
        let second_line_starts = got.copy.find('\n').unwrap() + 1;
        assert!(got.end > second_line_starts);
        assert!(got.end < got.copy.len());
        assert!(got.copy[second_line_starts..].starts_with("0123456789…"));
        assert!(got.copy.trim_end_matches('\n').ends_with(ELLIPSIS));
    }

    #[test]
    fn crlf_line_trailing_carriage_return_is_dropped() {
        // Given a CRLF-terminated line with a span mid-line.
        let src = "0123456789ABCDEF\r\n";

        // When truncating to a twelve-cell budget.
        let got = truncate_source(src, 6, 10, 12);

        // Then no carriage return survives in the copy.
        assert!(!got.copy.contains('\r'));
        // And the remapped span still slices the flagged text.
        assert_eq!(&got.copy[got.start..got.end], "6789");
    }

    #[test]
    fn budget_below_minimum_passes_through() {
        // Given a line wider than any sane window and a tiny budget.
        let src = "0123456789abcdefghij\n";

        // When truncating below the minimum budget.
        let got = truncate_source(src, 5, 9, 9);

        // Then the line passes through untouched and the span remaps to
        // itself.
        assert_eq!(got.copy, src);
        assert_eq!(got.start, 5);
        assert_eq!(got.end, 9);
    }
}
