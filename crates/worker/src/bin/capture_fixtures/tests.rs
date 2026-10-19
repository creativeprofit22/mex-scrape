use super::*;

fn to(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn args_need_a_known_retailer_and_kind() {
    let o = parse_args(&to(&["--retailer", "liverpool", "--kind", "both"])).unwrap();
    assert_eq!(o.retailer.id, "liverpool");
    assert_eq!(o.kinds, vec![Kind::Search, Kind::Product]);
    assert_eq!(o.query, DEFAULT_QUERY);
    assert!(parse_args(&to(&["--kind", "search"])).is_err());
    assert!(parse_args(&to(&["--retailer", "liverpool"])).is_err());
    assert!(parse_args(&to(&["--retailer", "../etc", "--kind", "search"])).is_err());
    assert!(parse_args(&to(&["--retailer", "liverpool", "--kind", "x"])).is_err());
    assert!(
        parse_args(&to(&[
            "--retailer",
            "liverpool",
            "--kind",
            "both",
            "--url",
            "https://www.liverpool.com.mx/"
        ]))
        .is_err()
    );
    assert!(
        parse_args(&to(&[
            "--retailer",
            "liverpool",
            "--kind",
            "search",
            "--bogus"
        ]))
        .is_err()
    );
}

#[test]
fn urls_must_be_on_the_retailers_hosts() {
    let liverpool = retailers::all()
        .iter()
        .find(|r| r.id == "liverpool")
        .unwrap();
    assert!(checked_url(liverpool, "https://www.liverpool.com.mx/tienda?s=x").is_ok());
    assert!(checked_url(liverpool, "https://evil.example/").is_err());
    assert!(checked_url(liverpool, "not a url").is_err());
    assert_eq!(
        fixture_path(Path::new("f"), liverpool, Kind::Product),
        Path::new("f").join("liverpool").join("product.html")
    );
}

#[test]
fn category_only_stores_ask_for_a_url() {
    let o = parse_args(&to(&["--retailer", "waldos", "--kind", "search"])).unwrap();
    let message = search_target(&o).unwrap_err().to_string();
    assert!(message.contains("--url <category URL>"), "{message}");
    let o = parse_args(&to(&[
        "--retailer",
        "waldos",
        "--kind",
        "search",
        "--url",
        "https://waldos.com.mx/electrodomesticos",
    ]))
    .unwrap();
    assert!(search_target(&o).is_ok());
}

#[test]
fn budget_stops_at_the_hard_cap() {
    let mut budget = Budget(0);
    for _ in 0..HARD_CAP {
        budget.take().unwrap();
    }
    assert!(budget.take().is_err());
}

#[test]
fn browser_flag_is_opt_in() {
    let base = ["--retailer", "sears_mx", "--kind", "search"];
    assert!(!parse_args(&to(&base)).unwrap().browser);
    let mut with = base.to_vec();
    with.push("--browser");
    assert!(parse_args(&to(&with)).unwrap().browser);
}

#[test]
fn no_block_needs_browser() {
    let base = ["--retailer", "sears_mx", "--kind", "search"];
    assert!(!parse_args(&to(&base)).unwrap().no_block);
    let mut without_browser = base.to_vec();
    without_browser.push("--no-block");
    assert!(parse_args(&to(&without_browser)).is_err());
    let mut with_browser = without_browser.clone();
    with_browser.push("--browser");
    let options = parse_args(&to(&with_browser)).unwrap();
    assert!(options.browser && options.no_block);
}
