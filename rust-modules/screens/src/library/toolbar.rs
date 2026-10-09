//! One value-chip model supplies both focus geometry and paint runs.
use std::ffi::{CStr, CString};
use super::*;
use plx_ui::value_chip::ValueChip;

pub(super) struct Chip {
    pub name: &'static CStr,
    pub value: CString,
    pub note: Option<CString>,
}
impl Chip {
    pub fn width(&self, measure: &dyn plx_machine::machine::Measure) -> f32 {
        ValueChip::width(measure, self.name, &self.value, self.note.as_deref())
    }
}

impl LibraryScreen {
    /// Every library lists more than one type ([`plx_data::browse::LibraryType::offered`]), so TYPE
    /// always leads; FILTER goes while the listed type takes no filters (collections).
    pub(super) fn toolbar_elems(&self) -> &'static [u32] {
        if self.listed.filters() { &[TYPE, SORT, FILTER] } else { &[TYPE, SORT] }
    }

    pub(super) fn view_section<H: LibraryLike>(&self, cx: &Cx<'_, H>) -> Option<usize> {
        let directory = H::directory(cx);
        self.pending.section().filter(|target| Some(target.epoch) == directory.epoch())
            .map(|target| target.index).or_else(|| match self.wanted_kind {
                Some(kind) => directory.preferred(kind), None => directory.current(),
            })
            .filter(|index| *index < directory.sections().len())
    }

    pub(super) fn library_label<H: LibraryLike>(&self, section: usize, cx: &Cx<'_, H>) -> CString {
        let directory = H::directory(cx);
        let value = if let Some(section) = directory.sections().get(section) {
            let owner = section.sid.and_then(|sid| directory.sources().iter().find(|(id, _)| *id == sid))
                .map(|(_, source)| source.handle.as_str()).unwrap_or("");
            if owner.is_empty() { section.row.title.clone() } else { format!("{} {owner}", section.row.title) }
        } else {
            let total = self.view_section(cx).map(|section| directory.favorite_sections_for(section).count()).unwrap_or(0);
            format!("+{}", total.saturating_sub(self.libraries.len().saturating_sub(1)))
        };
        CString::new(value).unwrap_or_default()
    }

    pub(super) fn library_lays<H: LibraryLike>(&self, cx: &Cx<'_, H>) -> Vec<plx_ui::widgets::StripLay> {
        plx_ui::widgets::strip_layout_measured(
            self.libraries.iter().map(|(_, section)| self.library_label(*section, cx).to_string_lossy().into_owned()),
            MARGIN_X + plx_ui::widgets::STRIP_PAD, plx_ui::theme::size::BODY,
            plx_ui::widgets::STRIP_GAP_WIDE, cx.measure)
    }

    pub(super) fn toolbar_chip<H: LibraryLike>(&self, elem: u32, cx: &Cx<'_, H>) -> Chip {
        let listing = H::listing(cx);
        let queued = self.pending.grid().filter(|(target, _)| target.matches(listing)).map(|(_, action)| action);
        let (name, value) = if elem == TYPE {
            let kind = match queued {
                Some(GridAction::LibraryType(kind)) => *kind,
                _ => listing.library_type(),
            };
            (plx_platform::i18n::msg::browse_library_type_c(), kind.title(self.kind).to_owned())
        } else if elem == SORT {
            let sort = match queued {
                Some(GridAction::Sort { key, .. }) => listing.sorts().iter().find(|sort| &sort.key == key),
                _ => listing.sorts().get(listing.sort_index()),
            };
            (plx_platform::i18n::msg::browse_library_sort_c(), sort.map_or(plx_platform::i18n::msg::browse_library_title(), |sort| sort.title.as_str()).to_owned())
        } else {
            let genre = match queued {
                Some(GridAction::Genre { id }) => id.as_ref().and_then(|id| listing.genres().iter().find(|genre| &genre.id == id)),
                _ => listing.genre(),
            }.map(|genre| genre.title.as_str());
            let unwatched = match queued {
                Some(GridAction::Unwatched { desired }) => *desired,
                _ => listing.unwatched(),
            };
            // The further filters, with a queued edit to one of them shown as it will land.
            let mut more: Vec<&str> = listing.more_filters().iter()
                .filter(|f| !matches!(queued, Some(GridAction::Filter { field, .. }) if *field == f.field))
                .map(|f| f.title.as_str()).collect();
            if let Some(GridAction::Filter { value: Some((_, title)), .. }) = queued {
                more.push(title.as_str());
            }
            (plx_platform::i18n::msg::browse_library_filter_c(), filter_summary(genre, unwatched, &more))
        };
        Chip { name, value: CString::new(format!(" · {value}")).unwrap_or_default(), note: None }
    }

    pub(super) fn toolbar_chip_rect<H: LibraryLike>(&self, elem: u32, cx: &Cx<'_, H>, at: At) -> Rect {
        let x = MARGIN_X + self.toolbar_elems().iter().take_while(|&&key| key != elem)
            .map(|&key| self.toolbar_chip(key, cx).width(cx.measure) + 16.0).sum::<f32>();
        let (layout, scroll) = match at {
            At::Drawn => (&self.layout, self.scroll.pos),
            At::SpringTarget => (&self.target_layout, self.scroll_target),
        };
        Rect::new(x, CONTENT_TOP + layout.grid_block_top() + plx_ui::consts::TITLE_DY + CARD_DY - scroll,
            self.toolbar_chip(elem, cx).width(cx.measure), 52.0)
    }
}

/// The Filter chip's value: every filter in force, the genre and the further ones by their values
/// and Unwatched last, " · "-joined — at most three named, the rest counted ("+2") so the chip
/// stays a chip. Nothing in force reads "All".
pub(super) fn filter_summary(genre: Option<&str>, unwatched: bool, more: &[&str]) -> String {
    if more.is_empty() {
        return match (genre, unwatched) {
            (None, false) => plx_platform::i18n::msg::browse_library_all().into(),
            (None, true) => plx_platform::i18n::msg::browse_library_unwatched().into(),
            (Some(genre), false) => genre.into(),
            (Some(genre), true) => plx_platform::i18n::msg::browse_library_genre_unwatched(genre),
        };
    }
    let mut parts: Vec<&str> = genre.into_iter().chain(more.iter().copied()).collect();
    if unwatched {
        parts.push(plx_platform::i18n::msg::browse_library_unwatched());
    }
    const NAMED: usize = 3;
    if parts.len() > NAMED {
        let rest = parts.len() - (NAMED - 1);
        let mut out = parts[..NAMED - 1].join(" \u{00b7} ");
        out.push_str(&format!(" \u{00b7} +{rest}"));
        return out;
    }
    parts.join(" \u{00b7} ")
}

#[cfg(test)]
mod filter_summary_tests {
    use super::filter_summary;

    #[test]
    fn the_chip_names_every_filter_in_force_and_counts_past_three() {
        let unwatched = plx_platform::i18n::msg::browse_library_unwatched();
        assert_eq!(filter_summary(None, false, &[]), plx_platform::i18n::msg::browse_library_all());
        assert_eq!(filter_summary(Some("s1"), false, &["1999"]), "s1 \u{00b7} 1999");
        assert_eq!(filter_summary(None, true, &["4K"]), format!("4K \u{00b7} {unwatched}"));
        assert_eq!(filter_summary(Some("s1"), true, &["1999", "4K"]), "s1 \u{00b7} 1999 \u{00b7} +2");
    }
}
