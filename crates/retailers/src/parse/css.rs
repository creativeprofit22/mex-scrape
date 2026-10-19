//! CSS-selector fallback using `scraper::Selector::parse`, as in
//! rust-scraper's `examples/document.rs`.

use scraper::{ElementRef, Html, Selector};

use super::{Draft, clean_text, compact, price};

/// Parses selectors, skipping any that are invalid (adapters assert in tests
/// that all of theirs parse; see [`all_parse`]).
#[must_use]
pub fn selectors(list: &[&str]) -> Vec<Selector> {
    list.iter()
        .filter_map(|s| Selector::parse(s).ok())
        .collect()
}

/// True when every selector in `list` is valid.
#[must_use]
pub fn all_parse(list: &[&str]) -> bool {
    list.iter().all(|s| Selector::parse(s).is_ok())
}

/// Collapsed text of an element.
#[must_use]
pub fn text(el: ElementRef<'_>) -> Option<String> {
    clean_text(&el.text().collect::<String>())
}

/// Text of the first element matching any selector (tried in order).
#[must_use]
pub fn first_text(doc: &Html, list: &[&str]) -> Option<String> {
    selectors(list)
        .iter()
        .find_map(|s| doc.select(s).find_map(text))
}

/// Attribute of the first element matching any selector that has it.
#[must_use]
pub fn first_attr(doc: &Html, list: &[&str], attr: &str) -> Option<String> {
    selectors(list).iter().find_map(|s| {
        doc.select(s)
            .find_map(|e| e.value().attr(attr).and_then(clean_text))
    })
}

/// Text of the first descendant of `el` matching any selector.
#[must_use]
pub fn child_text(el: ElementRef<'_>, list: &[&str]) -> Option<String> {
    selectors(list)
        .iter()
        .find_map(|s| el.select(s).find_map(text))
}

/// Attribute of the first descendant of `el` matching any selector.
#[must_use]
pub fn child_attr(el: ElementRef<'_>, list: &[&str], attr: &str) -> Option<String> {
    selectors(list).iter().find_map(|s| {
        el.select(s)
            .find_map(|e| e.value().attr(attr).and_then(clean_text))
    })
}

/// Every element matching the first selector in `list` that matches anything.
#[must_use]
pub fn items<'a>(doc: &'a Html, list: &[&str]) -> Vec<ElementRef<'a>> {
    selectors(list)
        .iter()
        .map(|s| doc.select(s).collect::<Vec<_>>())
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Selectors for one listing-tile layout (the CSS fallback on search and
/// category pages). Empty lists mean "not available".
#[derive(Debug, Clone, Copy)]
pub struct TileSpec {
    /// One element per product tile.
    pub tile: &'static [&'static str],
    /// The product link inside the tile; empty means the tile is the link.
    pub link: &'static [&'static str],
    /// Title text; when nothing matches, the link's `title` attribute.
    pub title: &'static [&'static str],
    /// Brand text.
    pub brand: &'static [&'static str],
    /// Selling price element.
    pub price: &'static [&'static str],
    /// Strike-through list price element.
    pub list_price: &'static [&'static str],
    /// Attribute holding a plain decimal price on the price elements
    /// (read before their text).
    pub price_attr: Option<&'static str>,
    /// Retailer SKU from the tile and its absolute-or-relative link.
    pub sku: fn(ElementRef<'_>, Option<&str>) -> Option<String>,
}

impl TileSpec {
    /// Every selector list, for the "all selectors parse" tests.
    #[must_use]
    pub fn lists(&self) -> [&'static [&'static str]; 6] {
        [
            self.tile,
            self.link,
            self.title,
            self.brand,
            self.price,
            self.list_price,
        ]
    }
}

/// Price in the first descendant of `el` matching any selector: `attr` if
/// given and present, else the first amount in its whitespace-free text.
#[must_use]
pub fn child_price(
    el: ElementRef<'_>,
    list: &[&str],
    attr: Option<&str>,
) -> Option<domain::PriceMxn> {
    let found = selectors(list).iter().find_map(|s| el.select(s).next())?;
    attr.and_then(|a| found.value().attr(a))
        .and_then(price::from_str)
        .or_else(|| price::first_in_text(&compact(&found.text().collect::<String>())))
}

/// Drafts for every tile matching `spec`.
#[must_use]
pub fn tile_drafts(doc: &Html, spec: &TileSpec) -> Vec<Draft> {
    items(doc, spec.tile)
        .into_iter()
        .map(|tile| {
            let link = if spec.link.is_empty() {
                Some(tile)
            } else {
                selectors(spec.link)
                    .iter()
                    .find_map(|s| tile.select(s).next())
            };
            let href = link
                .and_then(|a| a.value().attr("href"))
                .and_then(clean_text);
            let title = child_text(tile, spec.title).or_else(|| {
                link.and_then(|a| a.value().attr("title"))
                    .and_then(clean_text)
            });
            let price = child_price(tile, spec.price, spec.price_attr);
            let list_price = child_price(tile, spec.list_price, spec.price_attr)
                .filter(|l| price.is_some_and(|p| *l > p));
            Draft {
                title,
                brand: child_text(tile, spec.brand),
                price,
                list_price,
                sku: (spec.sku)(tile, href.as_deref()),
                url: href,
                ..Draft::default()
            }
        })
        .collect()
}

/// The last non-empty path segment of a URL or path (query and fragment
/// removed).
#[must_use]
pub fn last_segment(href: &str) -> Option<&str> {
    let end = href.find(['?', '#']).unwrap_or(href.len());
    href[..end].rsplit('/').find(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: TileSpec = TileSpec {
        tile: &["li.tile"],
        link: &["a.go"],
        title: &[".name"],
        brand: &[],
        price: &[".now"],
        list_price: &[".was"],
        price_attr: Some("data-amount"),
        sku: |_, href| href.and_then(last_segment).map(str::to_owned),
    };

    #[test]
    fn tiles_read_links_titles_and_prices() {
        let doc = Html::parse_document(
            r#"<ul><li class="tile"><a class="go" href="/p/11?x=1" title="T1">x</a>
                 <span class="now" data-amount="99.5">$1</span><s class="was">$ 1,200.00</s></li>
               <li class="tile"><a class="go" href="/p/12/"><b class="name"> Dos </b></a>
                 <span class="now">$2,000</span><s class="was">$10</s></li></ul>"#,
        );
        let drafts = tile_drafts(&doc, &SPEC);
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].title.as_deref(), Some("T1"));
        assert_eq!(drafts[0].price.map(|p| p.centavos()), Some(9_950));
        assert_eq!(drafts[0].list_price.map(|p| p.centavos()), Some(120_000));
        assert_eq!(drafts[0].sku.as_deref(), Some("11"));
        assert_eq!(drafts[1].title.as_deref(), Some("Dos"));
        assert_eq!(drafts[1].price.map(|p| p.centavos()), Some(200_000));
        // A "list price" below the price is not a list price.
        assert_eq!(drafts[1].list_price, None);
        assert_eq!(drafts[1].sku.as_deref(), Some("12"));
        assert!(SPEC.lists().iter().all(|l| all_parse(l)));
    }

    #[test]
    fn falls_through_selectors_in_order() {
        let doc = Html::parse_document(
            r#"<div class="card"><h2> Licuadora <b>Oster</b></h2><a href="/p/1">x</a></div>
               <div class="card"><h2>Otra</h2></div>"#,
        );
        assert_eq!(
            first_text(&doc, &[".missing", "h2"]).as_deref(),
            Some("Licuadora Oster")
        );
        assert_eq!(
            first_attr(&doc, &["a[href]"], "href").as_deref(),
            Some("/p/1")
        );
        let cards = items(&doc, &[".none", ".card"]);
        assert_eq!(cards.len(), 2);
        assert_eq!(child_text(cards[1], &["h2"]).as_deref(), Some("Otra"));
        assert_eq!(
            child_attr(cards[0], &["a"], "href").as_deref(),
            Some("/p/1")
        );
        assert!(all_parse(&["div > a", "[data-x]"]));
        assert!(!all_parse(&["div >"]));
    }
}
