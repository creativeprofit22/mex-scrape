//! Tests for `POST /roi`: results equal `domain::calculate_roi` (the
//! function the worker uses for search results), per-item errors, batch
//! limits (422), session and CSRF (401/403).

mod common;

use axum::http::{Method, StatusCode};
use common::{Client, TestApp};
use domain::{PriceMxn, RoiInput, calculate_roi};
use serde_json::{Value, json};

fn mxn(centavos: i64) -> PriceMxn {
    PriceMxn::from_centavos(centavos).unwrap()
}

/// Inputs covering profit, loss, fixed costs, zero units and zero prices.
fn inputs() -> Vec<RoiInput> {
    let base = RoiInput {
        sell_price: mxn(90_000),
        buy_price: mxn(50_000),
        amazon_fees: mxn(15_000),
        prep_fee: mxn(0),
        shipping: mxn(0),
        monthly_fixed: mxn(0),
        units_per_month: 0,
    };
    vec![
        base,
        RoiInput {
            prep_fee: mxn(1_000),
            shipping: mxn(2_500),
            monthly_fixed: mxn(100_000),
            units_per_month: 3,
            ..base
        },
        RoiInput {
            buy_price: mxn(99_999),
            ..base
        },
        RoiInput {
            monthly_fixed: mxn(1),
            ..base
        },
        RoiInput {
            sell_price: mxn(0),
            buy_price: mxn(0),
            amazon_fees: mxn(0),
            ..base
        },
    ]
}

fn request(i: &RoiInput) -> Value {
    json!({
        "sell_price_centavos": i.sell_price.centavos(),
        "buy_price_centavos": i.buy_price.centavos(),
        "amazon_fees_centavos": i.amazon_fees.centavos(),
        "prep_fee_centavos": i.prep_fee.centavos(),
        "shipping_centavos": i.shipping.centavos(),
        "monthly_fixed_centavos": i.monthly_fixed.centavos(),
        "units_per_month": i.units_per_month,
    })
}

fn expected(i: &RoiInput) -> Value {
    match calculate_roi(i) {
        Ok(r) => json!({
            "error": null,
            "fixed_cost_per_unit_centavos": r.fixed_cost_per_unit.centavos(),
            "total_cost_centavos": r.total_cost.centavos(),
            "profit_centavos": r.profit_centavos,
            "roi_bp": r.roi_bp,
            "margin_bp": r.margin_bp,
        }),
        Err(domain::RoiError::ZeroUnits) => json!({
            "error": "zero_units",
            "fixed_cost_per_unit_centavos": null, "total_cost_centavos": null,
            "profit_centavos": null, "roi_bp": null, "margin_bp": null,
        }),
        Err(domain::RoiError::Overflow) => unreachable!("fixture amounts are small"),
    }
}

async fn signed_in(app: &TestApp) -> Client {
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    client
}

async fn roi(client: &mut Client, items: Vec<Value>) -> common::Reply {
    client
        .send(Method::POST, "/roi", Some(json!({ "items": items })))
        .await
}

#[tokio::test]
async fn results_match_the_domain_calculation_in_order() {
    let app = TestApp::accounts_only();
    let mut client = signed_in(&app).await;
    let inputs = inputs();
    let reply = roi(&mut client, inputs.iter().map(request).collect()).await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    let want: Vec<Value> = inputs.iter().map(expected).collect();
    assert_eq!(reply.body, json!({ "results": want }));
    // Spot-check one value by hand: 900 - 150 - 500 = 250 profit on 500.
    assert_eq!(reply.body["results"][0]["profit_centavos"], 25_000);
    assert_eq!(reply.body["results"][0]["roi_bp"], 5_000);
    assert_eq!(reply.body["results"][3]["error"], "zero_units");
}

#[tokio::test]
async fn optional_costs_default_to_zero() {
    let app = TestApp::accounts_only();
    let mut client = signed_in(&app).await;
    let reply = roi(
        &mut client,
        vec![json!({
            "sell_price_centavos": 90_000, "buy_price_centavos": 50_000,
            "amazon_fees_centavos": 15_000
        })],
    )
    .await;
    assert_eq!(reply.body["results"][0], expected(&inputs()[0]));
}

#[tokio::test]
async fn batch_size_and_amounts_are_validated() {
    let app = TestApp::accounts_only();
    let mut client = signed_in(&app).await;
    let item = request(&inputs()[0]);

    let reply = roi(&mut client, vec![item.clone(); 100]).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["results"].as_array().unwrap().len(), 100);

    for (items, code) in [
        (vec![item.clone(); 101], "too_many"),
        (vec![], "empty"),
        (
            vec![json!({
                "sell_price_centavos": -1, "buy_price_centavos": 0, "amazon_fees_centavos": 0
            })],
            "invalid_amount",
        ),
    ] {
        let reply = roi(&mut client, items).await;
        assert_eq!(
            (reply.status, reply.error()),
            (StatusCode::UNPROCESSABLE_ENTITY, code)
        );
    }
    let reply = client
        .send(
            Method::POST,
            "/roi",
            Some(json!({ "items": [{"sell_price_centavos": 1}] })),
        )
        .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn needs_a_session_and_the_csrf_token() {
    let app = TestApp::accounts_only();
    let item = request(&inputs()[0]);

    let mut anonymous = app.client();
    anonymous.fetch_csrf().await;
    let reply = roi(&mut anonymous, vec![item.clone()]).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);

    let mut client = signed_in(&app).await;
    client.csrf = None;
    let reply = roi(&mut client, vec![item]).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn spec_documents_the_roi_route() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    assert_eq!(
        spec["paths"]["/roi"]["post"]["security"],
        json!([{ api::SESSION_SCHEME: [], api::CSRF_SCHEME: [] }])
    );
    for schema in [
        "RoiRequest",
        "RoiItemRequest",
        "RoiResponse",
        "RoiItemResponse",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
}
