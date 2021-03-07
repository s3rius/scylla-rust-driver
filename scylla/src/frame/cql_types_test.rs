use crate::cql_to_rust::FromCQLVal;
use crate::frame::response::result::CQLValue;
use crate::frame::value::Counter;
use crate::frame::value::Value;
use crate::transport::session::IntoTypedRows;
use crate::transport::session::Session;
use crate::SessionBuilder;
use bigdecimal::BigDecimal;
use chrono::NaiveDate;
use num_bigint::BigInt;
use std::cmp::PartialEq;
use std::env;
use std::fmt::Debug;
use std::str::FromStr;

// Used to prepare a table for test
// Creates keyspace ks
// Drops and creates table ks.{table_name} (id int PRIMARY KEY, val {type_name})
async fn init_test(table_name: &str, type_name: &str) -> Session {
    let uri = env::var("SCYLLA_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string());

    println!("Connecting to {} ...", uri);
    let session: Session = SessionBuilder::new().known_node(uri).build().await.unwrap();

    session
        .query(
            "CREATE KEYSPACE IF NOT EXISTS ks WITH REPLICATION = \
            {'class' : 'SimpleStrategy', 'replication_factor' : 1}",
            &[],
        )
        .await
        .unwrap();

    session
        .query(format!("DROP TABLE IF EXISTS ks.{}", table_name), &[])
        .await
        .unwrap();

    session
        .query(
            format!(
                "CREATE TABLE IF NOT EXISTS ks.{} (id int PRIMARY KEY, val {})",
                table_name, type_name
            ),
            &[],
        )
        .await
        .unwrap();

    session
}

// This function tests serialization and deserialization mechanisms by sending insert and select
// queries to running Scylla instance.
// To do so, it:
// Prepares a table for tests (by creating keyspace ks and table ks.{table_name} using init_test)
// Runs a test that, for every element of `tests`:
// - inserts 2 values (one encoded as string and one as bound values) into table ks.{type_name}
// - selects this 2 values and compares them with expected value
// Expected values and bound values are computed using T::from_str
async fn run_tests<T>(tests: &[&str], type_name: &str)
where
    T: Value + FromCQLVal<CQLValue> + FromStr + Debug + Clone + PartialEq,
{
    let session: Session = init_test(type_name, type_name).await;

    for test in tests.iter() {
        let insert_string_encoded_value = format!(
            "INSERT INTO ks.{} (id, val) VALUES (0, {})",
            type_name, test
        );
        session
            .query(insert_string_encoded_value, &[])
            .await
            .unwrap();

        let insert_bound_value = format!("INSERT INTO ks.{} (id, val) VALUES (1, ?)", type_name);
        let value_to_bound = T::from_str(test).ok().unwrap();
        session
            .query(insert_bound_value, (value_to_bound,))
            .await
            .unwrap();

        let select_values = format!("SELECT val from ks.{}", type_name);
        let read_values: Vec<T> = session
            .query(select_values, &[])
            .await
            .unwrap()
            .unwrap()
            .into_typed::<(T,)>()
            .map(Result::unwrap)
            .map(|row| row.0)
            .collect::<Vec<_>>();

        let expected_value = T::from_str(test).ok().unwrap();
        assert_eq!(read_values, vec![expected_value.clone(), expected_value]);
    }
}

#[tokio::test]
async fn test_varint() {
    let tests = [
        "0",
        "1",
        "127",
        "128",
        "129",
        "-1",
        "-128",
        "-129",
        "123456789012345678901234567890",
        "-123456789012345678901234567890",
    ];

    run_tests::<BigInt>(&tests, "varint").await;
}

#[tokio::test]
async fn test_decimal() {
    let tests = [
        "4.2",
        "0",
        "1.999999999999999999999999999999999999999",
        "997",
        "123456789012345678901234567890.1234567890",
        "-123456789012345678901234567890.1234567890",
    ];

    run_tests::<BigDecimal>(&tests, "decimal").await;
}

#[tokio::test]
async fn test_bool() {
    let tests = ["true", "false"];

    run_tests::<bool>(&tests, "boolean").await;
}

#[tokio::test]
async fn test_float() {
    let max = f32::MAX.to_string();
    let min = f32::MIN.to_string();
    let tests = [
        "3.14",
        "997",
        "0.1",
        "128",
        "-128",
        max.as_str(),
        min.as_str(),
    ];

    run_tests::<f32>(&tests, "float").await;
}

#[tokio::test]
async fn test_counter() {
    let big_increment = i64::MAX.to_string();
    let tests = ["1", "997", big_increment.as_str()];

    // Can't use run_tests, because counters are special and can't be inserted
    let type_name = "counter";
    let session: Session = init_test(type_name, type_name).await;

    for (i, test) in tests.iter().enumerate() {
        let update_bound_value = format!("UPDATE ks.{} SET val = val + ? WHERE id = ?", type_name);
        let value_to_bound = Counter(i64::from_str(test).unwrap());
        session
            .query(update_bound_value, (value_to_bound, i as i32))
            .await
            .unwrap();

        let select_values = format!("SELECT val FROM ks.{} WHERE id = ?", type_name);
        let read_values: Vec<Counter> = session
            .query(select_values, (i as i32,))
            .await
            .unwrap()
            .unwrap()
            .into_typed::<(Counter,)>()
            .map(Result::unwrap)
            .map(|row| row.0)
            .collect::<Vec<_>>();

        let expected_value = Counter(i64::from_str(test).unwrap());
        assert_eq!(read_values, vec![expected_value]);
    }
}

#[tokio::test]
async fn test_naive_date() {
    let session: Session = init_test("naive_date", "date").await;

    let min_naive_date: NaiveDate = chrono::naive::MIN_DATE;
    assert_eq!(min_naive_date, NaiveDate::from_ymd(-262144, 1, 1));

    let max_naive_date: NaiveDate = chrono::naive::MAX_DATE;
    assert_eq!(max_naive_date, NaiveDate::from_ymd(262143, 12, 31));

    let tests = [
        // Basic test values
        ("0000-1-1", Some(NaiveDate::from_ymd(0000, 1, 1))),
        ("1970-01-01", Some(NaiveDate::from_ymd(1970, 1, 1))),
        ("2020-03-07", Some(NaiveDate::from_ymd(2020, 3, 7))),
        ("1337-4-5", Some(NaiveDate::from_ymd(1337, 4, 5))),
        ("-1-12-31", Some(NaiveDate::from_ymd(-1, 12, 31))),
        // min/max values allowed by NaiveDate
        ("-262144-1-1", Some(min_naive_date)),
        ("262143-12-31", Some(max_naive_date)),
        // 1 less/more than min/max values allowed by NaiveDate
        ("-262145-12-31", None),
        ("262144-1-1", None),
        // min/max values allowed by the database
        ("-5877641-06-23", None),
        ("5881580-07-11", None),
    ];

    for (date_text, date) in tests.iter() {
        session
            .query(
                format!(
                    "INSERT INTO ks.naive_date (id, val) VALUES (0, '{}')",
                    date_text
                ),
                &[],
            )
            .await
            .unwrap();

        let read_date: Option<NaiveDate> = session
            .query("SELECT val from ks.naive_date", &[])
            .await
            .unwrap()
            .unwrap()
            .into_typed::<(NaiveDate,)>()
            .next()
            .unwrap()
            .ok()
            .map(|row| row.0);

        assert_eq!(read_date, *date);

        // If date is representable by NaiveDate try inserting it and reading again
        if let Some(naive_date) = date {
            session
                .query(
                    "INSERT INTO ks.naive_date (id, val) VALUES (0, ?)",
                    (naive_date,),
                )
                .await
                .unwrap();

            let (read_date,): (NaiveDate,) = session
                .query("SELECT val from ks.naive_date", &[])
                .await
                .unwrap()
                .unwrap()
                .into_typed::<(NaiveDate,)>()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(read_date, *naive_date);
        }
    }

    // 1 less/more than min/max values allowed by the database should cause error
    session
        .query(
            "INSERT INTO ks.naive_date (id, val) VALUES (0, '-5877641-06-22')",
            &[],
        )
        .await
        .unwrap_err();

    session
        .query(
            "INSERT INTO ks.naive_date (id, val) VALUES (0, '5881580-07-12')",
            &[],
        )
        .await
        .unwrap_err();
}
