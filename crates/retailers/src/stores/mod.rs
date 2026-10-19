//! One adapter per store. Add a store here and in [`ADAPTERS`].

mod bodega_aurrera;
mod chedraui;
mod claro;
mod costco_mx;
mod heb_mx;
mod liverpool;
mod mercadolibre;
mod palacio_de_hierro;
mod sams_mx;
mod sanborns;
mod sears_mx;
mod waldos;
mod walmart_glass;
mod walmart_mx;

use crate::adapter::Retailer;

/// Every adapter, in registry order.
pub(crate) const ADAPTERS: &[&'static dyn Retailer] = &[
    &walmart_mx::WalmartMx,
    &liverpool::Liverpool,
    &chedraui::Chedraui,
    &bodega_aurrera::BodegaAurrera,
    &costco_mx::CostcoMx,
    &sams_mx::SamsMx,
    &sears_mx::SearsMx,
    &palacio_de_hierro::PalacioDeHierro,
    &heb_mx::HebMx,
    &sanborns::Sanborns,
    &waldos::Waldos,
    &mercadolibre::MercadoLibre,
];
