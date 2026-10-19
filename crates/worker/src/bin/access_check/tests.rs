use super::*;

fn to(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn civil_date_matches_known_days() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(19_782), "2024-02-29");
    assert_eq!(civil_date(20_380), "2025-10-19");
}

#[test]
fn args_are_capped_and_unknown_flags_rejected() {
    let options = parse_args(&to(&["--live", "--max-urls", "99"])).unwrap();
    assert!(options.live);
    assert_eq!(options.max_urls, HARD_CAP);
    assert!(!parse_args(&[]).unwrap().live);
    assert!(parse_args(&to(&["--bogus"])).is_err());
    assert!(parse_args(&to(&["--max-urls"])).is_err());
}

#[test]
fn matrix_labels_each_cell_and_states_the_mode() {
    let retailer = &retailers::all()[0];
    let row = vec![
        (PageKind::Search, Cell::Allowed),
        (PageKind::Category, Cell::BlockedRobots),
        (PageKind::Product, Cell::BlockedPolicy),
    ];
    let text = render("2026-10-19", true, &[(retailer, row)]);
    assert!(text.contains("2026-10-19"));
    assert!(text.contains("| allowed | blocked by robots.txt | blocked by policy |"));
    assert!(render("d", false, &[]).contains("live check pending"));
}

#[test]
fn kyc_refusals_map_to_robots_or_policy_cells() {
    assert_eq!(
        cell_for_refusal(RefusalReason::RobotsDisallowed),
        Cell::BlockedRobots
    );
    assert_eq!(
        cell_for_refusal(RefusalReason::BrowserRobots),
        Cell::BlockedRobots
    );
    assert_eq!(cell_for_refusal(RefusalReason::Policy), Cell::BlockedPolicy);
}

fn retailer(id: &str) -> &'static RetailerInfo {
    retailers::all().iter().find(|r| r.id == id).unwrap()
}

#[test]
fn browser_pages_use_the_browser_api_and_never_fall_back_to_unlocker() {
    for id in ["sears_mx", "sanborns"] {
        let store = retailer(id);
        assert_eq!(
            channel_for(store, PageKind::Category, true),
            Channel::Browser
        );
        assert_eq!(
            channel_for(store, PageKind::Category, false),
            Channel::NoBrowserZone
        );
        assert_eq!(
            channel_for(store, PageKind::Search, false),
            Channel::Unlocker
        );
    }
    assert_eq!(
        channel_for(retailer("walmart_mx"), PageKind::Category, true),
        Channel::Unlocker
    );
}

#[test]
fn matrix_marks_browser_cells_and_missing_browser_zone() {
    let row = vec![
        (PageKind::Search, Cell::Allowed),
        (PageKind::Category, Cell::NeedsBrowserZone),
        (PageKind::Product, Cell::Allowed),
    ];
    let text = render("d", true, &[(retailer("sears_mx"), row)]);
    assert!(text.contains(
        "| allowed | not checked (needs BRIGHTDATA_BROWSER_ZONE) (Browser API) | allowed |"
    ));
}
