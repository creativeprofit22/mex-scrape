//! Retailer registry: the one table to edit when a retailer or sample URL
//! changes. Sample URLs are public pages (no login) and best guesses; the
//! access check reports a dead one as an error, not as blocked.

/// The kind of public page a sample URL points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageKind {
    /// A search results page.
    Search,
    /// A category listing page.
    Category,
    /// A single product page.
    Product,
}

impl PageKind {
    /// Every kind, in matrix column order.
    pub const ALL: [Self; 3] = [Self::Search, Self::Category, Self::Product];

    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Category => "category",
            Self::Product => "product",
        }
    }
}

/// One retailer with its hosts and sample pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetailerInfo {
    /// Stable lowercase id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Lowercase hosts that belong to this retailer.
    pub hosts: &'static [&'static str],
    /// One sample public URL per page kind.
    pub samples: &'static [(PageKind, &'static str)],
    /// How pages are fetched by default.
    pub access: AccessMode,
    /// Page kinds whose Unlocker HTML is proven to lack product data, so
    /// they need the Browser API (evidence in the store's doc and tests).
    pub browser_pages: &'static [PageKind],
    /// True when buying needs a paid membership (Costco, Sam's Club).
    pub membership_required: bool,
}

/// How a retailer's pages are fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessMode {
    /// Bright Data Web Unlocker (static HTML).
    Unlocker,
    /// Bright Data Browser API (rendered HTML), only where JavaScript is proven necessary.
    Browser,
}

impl AccessMode {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unlocker => "unlocker",
            Self::Browser => "browser",
        }
    }
}

impl RetailerInfo {
    /// How pages of `kind` are fetched: the Browser API for the proven
    /// JavaScript-only kinds, otherwise [`RetailerInfo::access`].
    #[must_use]
    pub fn access_for(&self, kind: PageKind) -> AccessMode {
        if self.browser_pages.contains(&kind) {
            AccessMode::Browser
        } else {
            self.access
        }
    }
}

pub(crate) const WALMART_MX: RetailerInfo = RetailerInfo {
    id: "walmart_mx",
    name: "Walmart MX",
    hosts: &["www.walmart.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.walmart.com.mx/search?q=licuadora",
        ),
        (
            PageKind::Category,
            "https://www.walmart.com.mx/browse/electrodomesticos/cocina",
        ),
        (
            PageKind::Product,
            "https://www.walmart.com.mx/ip/licuadora/00750100000001",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const LIVERPOOL: RetailerInfo = RetailerInfo {
    id: "liverpool",
    name: "Liverpool",
    hosts: &["www.liverpool.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.liverpool.com.mx/tienda?s=licuadora",
        ),
        (
            PageKind::Category,
            "https://www.liverpool.com.mx/tienda/electrodomesticos/catst1090234",
        ),
        (
            PageKind::Product,
            "https://www.liverpool.com.mx/tienda/pdp/licuadora/1100000001",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const CHEDRAUI: RetailerInfo = RetailerInfo {
    id: "chedraui",
    name: "Chedraui",
    hosts: &["www.chedraui.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.chedraui.com.mx/licuadora?_q=licuadora&map=ft",
        ),
        (
            PageKind::Category,
            "https://www.chedraui.com.mx/electrodomesticos",
        ),
        (
            PageKind::Product,
            "https://www.chedraui.com.mx/licuadora-oster-1000001/p",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const BODEGA_AURRERA: RetailerInfo = RetailerInfo {
    id: "bodega_aurrera",
    name: "Bodega Aurrera",
    hosts: &["www.bodegaaurrera.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.bodegaaurrera.com.mx/c/kp/licuadoras-de-inmersion",
        ),
        (
            PageKind::Category,
            "https://www.bodegaaurrera.com.mx/browse/electrodomesticos/cocina",
        ),
        (
            PageKind::Product,
            "https://www.bodegaaurrera.com.mx/ip/Licuadora-de-inmersion-Ganiza-6-en-1-1100W-15-velocidades-con-accesorios/00045047183451",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const COSTCO_MX: RetailerInfo = RetailerInfo {
    id: "costco_mx",
    name: "Costco MX",
    hosts: &["www.costco.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.costco.com.mx/search?text=licuadora",
        ),
        (
            PageKind::Category,
            "https://www.costco.com.mx/Electrodomesticos/c/cos_2",
        ),
        (
            PageKind::Product,
            "https://www.costco.com.mx/Electrodomesticos/Electrodomesticos-de-Cocina/Licuadoras-y-Procesadores/Nutribullet-SmartSense-Licuadora-1400W/p/687400",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: true,
};

pub(crate) const SAMS_MX: RetailerInfo = RetailerInfo {
    id: "sams_mx",
    name: "Sam's Club MX",
    hosts: &["www.sams.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.sams.com.mx/search?q=licuadora&z=1",
        ),
        (
            PageKind::Category,
            "https://www.sams.com.mx/browse/electrodomesticos/cocina",
        ),
        (
            PageKind::Product,
            "https://www.sams.com.mx/ip/licuadora-ninja-2-1-l-1200w-bn700/980022073",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: true,
};

pub(crate) const SEARS_MX: RetailerInfo = RetailerInfo {
    id: "sears_mx",
    name: "Sears MX",
    hosts: &["www.sears.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.sears.com.mx/resultados/q=licuadora/1",
        ),
        (
            PageKind::Category,
            "https://www.sears.com.mx/cat/l%C3%ADnea%20blanca%20y%20electrodom%C3%A9sticos/electrodom%C3%A9sticos%20de%20cocina/licuadoras%20y%20picadoras?id=4714",
        ),
        (
            PageKind::Product,
            "https://www.sears.com.mx/producto/306869/cafetera-programable-koblenz-ckm-212-pin-1-5-l-negro-end2",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[PageKind::Category],
    membership_required: false,
};

pub(crate) const PALACIO_DE_HIERRO: RetailerInfo = RetailerInfo {
    id: "palacio_de_hierro",
    name: "El Palacio de Hierro",
    hosts: &["www.elpalaciodehierro.com"],
    samples: &[
        (
            PageKind::Search,
            "https://www.elpalaciodehierro.com/buscar?q=licuadora",
        ),
        (
            PageKind::Category,
            "https://www.elpalaciodehierro.com/hogar/electrodomesticos/",
        ),
        (
            PageKind::Product,
            "https://www.elpalaciodehierro.com/kitchenaid-batidora-de-pedestal-sun-dried-tomato-artisan-plus-45555106.html",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const HEB_MX: RetailerInfo = RetailerInfo {
    id: "heb_mx",
    name: "HEB MX",
    hosts: &["www.heb.com.mx"],
    samples: &[
        (PageKind::Search, "https://www.heb.com.mx/search?q=cafe"),
        (PageKind::Category, "https://www.heb.com.mx/cafe"),
        (
            PageKind::Product,
            "https://www.heb.com.mx/tanto-mucho-cafe-de-olla-mezcla-1910-molido-500-gr-787233/p",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const SANBORNS: RetailerInfo = RetailerInfo {
    id: "sanborns",
    name: "Sanborns",
    hosts: &["www.sanborns.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://www.sanborns.com.mx/resultados/q=licuadora/1",
        ),
        (
            PageKind::Category,
            "https://www.sanborns.com.mx/cat/tecnolog%C3%ADa%20y%20electr%C3%B3nica/tv%20y%20video/pantallas?id=100609",
        ),
        (
            PageKind::Product,
            "https://www.sanborns.com.mx/producto/968461/pantalla-samsung-de-75-pulgadas-un75u8200hf-uhd/",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[PageKind::Category],
    membership_required: false,
};

pub(crate) const WALDOS: RetailerInfo = RetailerInfo {
    id: "waldos",
    name: "Waldo's",
    hosts: &["waldos.com.mx", "www.waldos.com.mx"],
    samples: &[
        (
            PageKind::Search,
            "https://waldos.com.mx/catalogsearch/result/?q=licuadora",
        ),
        (
            PageKind::Category,
            "https://waldos.com.mx/electrodomesticos/electrodomesticos-de-cocina",
        ),
        (
            PageKind::Product,
            "https://waldos.com.mx/crepera-electrica-turmix-gris-cp-3",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

pub(crate) const MERCADOLIBRE: RetailerInfo = RetailerInfo {
    id: "mercadolibre",
    name: "MercadoLibre MX",
    hosts: &[
        "listado.mercadolibre.com.mx",
        "articulo.mercadolibre.com.mx",
        "www.mercadolibre.com.mx",
    ],
    samples: &[
        (
            PageKind::Search,
            "https://listado.mercadolibre.com.mx/licuadora",
        ),
        (
            PageKind::Category,
            "https://listado.mercadolibre.com.mx/electrodomesticos/",
        ),
        (
            PageKind::Product,
            "https://www.mercadolibre.com.mx/licuadora-ninja-profesional-plus-bn701laa-1400w-tritura-hielo-al-instante-capacidad-de-21l-para-toda-la-familia-3-programas-auto-iq-smoothies-helados-y-frozen-negro/p/MLM52768030",
        ),
    ],
    access: AccessMode::Unlocker,
    browser_pages: &[],
    membership_required: false,
};

const RETAILERS: &[RetailerInfo] = &[
    WALMART_MX,
    LIVERPOOL,
    CHEDRAUI,
    BODEGA_AURRERA,
    COSTCO_MX,
    SAMS_MX,
    SEARS_MX,
    PALACIO_DE_HIERRO,
    HEB_MX,
    SANBORNS,
    WALDOS,
    MERCADOLIBRE,
];

/// Every registered retailer, in a fixed order.
#[must_use]
pub const fn all() -> &'static [RetailerInfo] {
    RETAILERS
}

/// Every registered host, sorted and unique.
#[must_use]
pub fn hosts() -> Vec<&'static str> {
    let mut hosts: Vec<&'static str> = RETAILERS
        .iter()
        .flat_map(|r| r.hosts.iter().copied())
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// The retailer owning `host` (case-insensitive), if registered.
#[must_use]
pub fn retailer_for_host(host: &str) -> Option<&'static RetailerInfo> {
    RETAILERS
        .iter()
        .find(|r| r.hosts.iter().any(|h| h.eq_ignore_ascii_case(host)))
}

#[cfg(test)]
mod tests {
    use brightdata::TargetUrl;

    use super::*;

    #[test]
    fn browser_api_only_for_proven_javascript_pages() {
        assert_eq!(SEARS_MX.access_for(PageKind::Category), AccessMode::Browser);
        assert_eq!(SEARS_MX.access_for(PageKind::Product), AccessMode::Unlocker);
        assert_eq!(SANBORNS.access_for(PageKind::Category), AccessMode::Browser);
        // Search is disallowed by robots.txt, so no browser is spent on it.
        assert_eq!(SANBORNS.access_for(PageKind::Search), AccessMode::Unlocker);
        assert_eq!(SANBORNS.access_for(PageKind::Product), AccessMode::Unlocker);
        let with_browser: Vec<&str> = all()
            .iter()
            .filter(|r| !r.browser_pages.is_empty())
            .map(|r| r.id)
            .collect();
        assert_eq!(with_browser, ["sears_mx", "sanborns"]);
        assert!(all().iter().all(|r| r.access == AccessMode::Unlocker));
    }

    #[test]
    fn every_sample_is_a_valid_url_on_a_registered_host_with_all_kinds() {
        for retailer in all() {
            for kind in PageKind::ALL {
                assert_eq!(
                    retailer.samples.iter().filter(|(k, _)| *k == kind).count(),
                    1,
                    "{} {kind:?}",
                    retailer.id
                );
            }
            for (_, url) in retailer.samples {
                let target = TargetUrl::parse(url).unwrap();
                assert!(retailer.hosts.iter().any(|h| *h == target.host()), "{url}");
            }
        }
    }

    #[test]
    fn ids_are_valid_and_unique_and_membership_is_costco_and_sams_only() {
        let mut ids: Vec<&str> = all().iter().map(|r| r.id).collect();
        for id in &ids {
            assert!(domain::RetailerId::new(id).is_ok(), "{id}");
        }
        assert_eq!(ids.len(), 12);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 12);
        let members: Vec<&str> = all()
            .iter()
            .filter(|r| r.membership_required)
            .map(|r| r.id)
            .collect();
        assert_eq!(members, ["costco_mx", "sams_mx"]);
    }

    #[test]
    fn registry_membership_flags_match_the_domain_helper() {
        for r in all() {
            let id = domain::RetailerId::new(r.id).unwrap();
            assert_eq!(
                r.membership_required,
                domain::is_membership_retailer(&id),
                "{}",
                r.id
            );
        }
    }

    #[test]
    fn host_lookup_is_case_insensitive_and_unknown_is_none() {
        assert_eq!(
            retailer_for_host("WWW.Liverpool.com.mx").unwrap().id,
            "liverpool"
        );
        assert!(retailer_for_host("evil.example").is_none());
        assert!(hosts().windows(2).all(|w| w[0] < w[1]));
    }
}
