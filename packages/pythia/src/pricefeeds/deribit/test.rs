use super::*;
use awc::Client;
use chrono::{Datelike, TimeDelta, TimeZone};

#[test]
fn test_get_quoting_forward_at_date() {
    // From real observation of quotation time of deribit:

    // 11th June 2025: a Wednesday, only 10 expiries are available
    let today = Utc.with_ymd_and_hms(2025, 6, 11, 14, 0, 0).unwrap();
    let expiries = forward_price::option_expiries(today);

    let expected_dates = vec![
        Utc.with_ymd_and_hms(2025, 6, 12, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 13, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 14, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 20, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 27, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 7, 25, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 8, 29, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 9, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 12, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 3, 27, 8, 0, 0).unwrap(),
    ];

    assert_eq!(expiries, expected_dates);
    assert!(!delivery_price::is_an_expiry_date(today));

    // 12th June 2025: a Thursday, 11 expiries are available as a new weekly expiry
    // is available while the previous one is still not expired
    let today = Utc.with_ymd_and_hms(2025, 6, 12, 14, 0, 0).unwrap();
    let expiries = forward_price::option_expiries(today);

    let expected_dates = vec![
        Utc.with_ymd_and_hms(2025, 6, 13, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 14, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 15, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 20, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 6, 27, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 7, 04, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 7, 25, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 8, 29, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 9, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 12, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 3, 27, 8, 0, 0).unwrap(),
    ];

    assert_eq!(expiries, expected_dates);
    assert!(!delivery_price::is_an_expiry_date(today));

    let today = Utc.with_ymd_and_hms(2025, 6, 23, 8, 0, 0).unwrap();
    let expiries = forward_price::option_expiries(today);

    let expected_dates = vec![
        Utc.with_ymd_and_hms(2025, 06, 24, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 06, 25, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 06, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 06, 27, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 04, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 11, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 25, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 08, 29, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 09, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 12, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 03, 27, 8, 0, 0).unwrap(),
    ];

    assert_eq!(expiries, expected_dates);

    assert!(delivery_price::is_an_expiry_date(today));

    let today = Utc.with_ymd_and_hms(2025, 6, 27, 8, 0, 0).unwrap();
    let expiries = forward_price::option_expiries(today);

    let expected_dates = vec![
        Utc.with_ymd_and_hms(2025, 06, 28, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 06, 29, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 06, 30, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 04, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 11, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 18, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 07, 25, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 08, 29, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 09, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2025, 12, 26, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 03, 27, 8, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 06, 26, 8, 0, 0).unwrap(),
    ];

    assert_eq!(expiries, expected_dates);

    assert!(delivery_price::is_an_expiry_date(today));
}

#[actix::test]
async fn test_get_delivery_price_for_expiry() {
    let client = Client::new();
    let asset_pair = AssetPair::BtcUsd;
    let now = Utc::now();
    let today_expiry = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 8, 0, 0)
        .unwrap();
    let instant = today_expiry + TimeDelta::days(if now > today_expiry { 0 } else { -1 });
    delivery_price::retrieve_delivery_price(&client, asset_pair, instant)
        .await
        .unwrap();

    assert_eq!(
        delivery_price::retrieve_delivery_price(
            &client,
            asset_pair,
            Utc.with_ymd_and_hms(2022, 1, 1, 8, 0, 0).unwrap()
        )
        .await
        .unwrap(),
        47073.56
    );
}

#[actix::test]
async fn test_get_forward_prices() {
    let client = Client::new();
    let asset_pair = AssetPair::BtcUsd;
    let now = Utc::now();

    println!(
        "{:?}",
        forward_price::retrieve_dated_option_prices(&client, asset_pair, now)
            .await
            .unwrap()
    );
}
