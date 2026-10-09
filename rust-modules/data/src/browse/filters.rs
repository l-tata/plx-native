//! **The library's further filters** — beyond the Unwatched switch and the genre, the filters the
//! SECTION advertises for the listed type (`includeMeta=1` → `Meta.Type[].Filter[]`): year, decade,
//! resolution, HDR, content rating, director, actor, studio. Each is a field of the section listing
//! (`/library/sections/{k}/all?<field>=<value>`); a field with values (`filterType` string or
//! integer) is chosen from its own value list (`/library/sections/{k}/<field>`, the directory the
//! genre list already comes from), a boolean one is a switch (`<field>=1`).
//!
//! Filters COMBINE: every active one joins the query, with the Unwatched switch and the genre, so
//! the listing is their intersection (PMS ANDs distinct fields). A field holds one value at a time.
//! They are kept per section for the session, beside the rest of the section's query; only the
//! sort, Unwatched, the genre and the listing type are remembered across launches.
//!
//! What a server offers decides what is shown, in [`OFFERED`]'s order — a server that does not
//! advertise HDR (or a TV listing, which has no director) simply has no such row.

use plx_plex::plex::MetaType;

/// The fields offered beside the genre, in the order the Filter menu lists them. Anything else a
/// server advertises (collection, label, country, …) is not offered.
pub const OFFERED: &[&str] = &["year", "decade", "resolution", "hdr", "contentRating", "director", "actor", "studio"];

/// How a filter is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    /// On or off (`<field>=1`).
    Switch,
    /// One value from the field's own list.
    Values,
}

/// One filter the section advertises.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterDef {
    /// The listing's query field, e.g. `year`.
    pub field: String,
    /// The server's (localized) name for it, e.g. "Year".
    pub title: String,
    pub kind: FilterKind,
}

/// One filter in force: the field, the value sent, and what the value is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveFilter {
    pub field: String,
    pub value: String,
    /// The value's title ("1999", "4K"); a switch's is its filter's title ("HDR").
    pub title: String,
}

/// The offered filters of the listed type's menu, in [`OFFERED`] order.
pub fn defs(kind: &MetaType) -> Vec<FilterDef> {
    OFFERED
        .iter()
        .filter_map(|field| {
            let f = kind.filter.iter().find(|f| f.filter == *field)?;
            let kind = match f.filter_type.as_str() {
                "boolean" => FilterKind::Switch,
                "string" | "integer" | "tag" | "" => FilterKind::Values,
                _ => return None,
            };
            Some(FilterDef {
                field: f.filter.clone(),
                title: if f.title.is_empty() { f.filter.clone() } else { f.title.clone() },
                kind,
            })
        })
        .collect()
}

/// Put `value` in force for `field` (replacing the field's previous value), or take the field off
/// with `None`. `true` when anything changed.
pub fn set(active: &mut Vec<ActiveFilter>, field: &str, value: Option<(String, String)>) -> bool {
    let before = active.clone();
    active.retain(|f| f.field != field);
    if let Some((value, title)) = value {
        // Keep the menu's order, so the header reads the same however the filters were chosen.
        let rank = |f: &str| OFFERED.iter().position(|o| *o == f).unwrap_or(usize::MAX);
        let at = active.iter().position(|f| rank(&f.field) > rank(field)).unwrap_or(active.len());
        active.insert(at, ActiveFilter { field: field.to_owned(), value, title });
    }
    *active != before
}

/// The query pairs the active filters add to a listing.
pub fn pairs(active: &[ActiveFilter]) -> impl Iterator<Item = (String, String)> + '_ {
    active.iter().map(|f| (f.field.clone(), f.value.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(json: &str) -> MetaType {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_offered_filters_come_from_the_section_in_menu_order() {
        let kind = meta(r#"{"active":true,"Filter":[
            {"filter":"genre","filterType":"string","title":"Genre"},
            {"filter":"studio","filterType":"string","title":"Studio"},
            {"filter":"hdr","filterType":"boolean","title":"HDR"},
            {"filter":"year","filterType":"integer","title":"Year"},
            {"filter":"label","filterType":"string","title":"Labels"},
            {"filter":"unwatched","filterType":"boolean","title":"Unplayed"}]}"#);
        let got: Vec<_> = defs(&kind).into_iter().map(|d| (d.field, d.kind)).collect();
        assert_eq!(got, [
            ("year".to_owned(), FilterKind::Values),
            ("hdr".to_owned(), FilterKind::Switch),
            ("studio".to_owned(), FilterKind::Values),
        ], "genre and Unwatched have rows of their own; a field not offered here is left out");
    }

    #[test]
    fn filters_combine_one_value_per_field_in_menu_order() {
        let mut active = Vec::new();
        assert!(set(&mut active, "studio", Some(("7".into(), "s7".into()))));
        assert!(set(&mut active, "year", Some(("1999".into(), "1999".into()))));
        assert!(set(&mut active, "year", Some(("2001".into(), "2001".into()))), "a field holds one value");
        let got: Vec<_> = pairs(&active).collect();
        assert_eq!(got, [("year".to_owned(), "2001".to_owned()), ("studio".to_owned(), "7".to_owned())]);
        assert!(!set(&mut active, "hdr", None), "taking off a filter not in force changes nothing");
        assert!(set(&mut active, "year", None));
        assert_eq!(active.len(), 1);
    }
}
