//! Small compatibility functions; JSON numbers never pass through SQLite REAL.
use chrono::{NaiveDateTime, TimeZone, Utc};
use libsqlite3_sys as ffi;
use serde_json::Value;
use sqlx::SqliteConnection;
use std::{cmp::Ordering, ffi::c_void, panic::catch_unwind};

pub async fn register(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let mut handle = connection.lock_handle().await?;
    for (kind, name, count) in [
        (0, c"now", 0),
        (1, c"telesink_contains", 2),
        (2, c"telesink_ilike", 2),
        (3, c"telesink_property", 4),
        (4, c"telesink_local_date", 2),
        (5, c"telesink_json_set", 3),
    ] {
        // SQLx's locked handle excludes its worker thread while registering.
        // Function identity is an integer tag, never an owned/dereferenced pointer.
        let code = unsafe {
            ffi::sqlite3_create_function_v2(
                handle.as_raw_handle().as_ptr(),
                name.as_ptr(),
                count,
                ffi::SQLITE_UTF8
                    | if kind == 0 {
                        0
                    } else {
                        ffi::SQLITE_DETERMINISTIC
                    },
                kind as *mut c_void,
                Some(callback),
                None,
                None,
                None,
            )
        };
        if code != ffi::SQLITE_OK {
            return Err(sqlx::Error::Protocol(
                "could not register SQLite functions".into(),
            ));
        }
    }
    Ok(())
}

unsafe extern "C" fn callback(
    context: *mut ffi::sqlite3_context,
    count: i32,
    values: *mut *mut ffi::sqlite3_value,
) {
    // SQLite owns these arguments for this synchronous invocation only. Copy
    // text before returning and contain every Rust panic inside the C boundary.
    let result = catch_unwind(|| unsafe {
        let kind = ffi::sqlite3_user_data(context) as usize;
        let pointers = if count == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(values, count as usize)
        };
        let mut args = Vec::with_capacity(pointers.len());
        for &value in pointers {
            if ffi::sqlite3_value_type(value) == ffi::SQLITE_NULL {
                args.push(None);
            } else {
                let pointer = ffi::sqlite3_value_text(value);
                if pointer.is_null() {
                    return Err(());
                }
                let bytes =
                    std::slice::from_raw_parts(pointer, ffi::sqlite3_value_bytes(value) as usize);
                args.push(Some(std::str::from_utf8(bytes).map_err(|_| ())?.to_owned()));
            }
        }
        evaluate(kind, &args)
    });
    unsafe {
        match result {
            Ok(Ok(Some(value))) => ffi::sqlite3_result_text(
                context,
                value.as_ptr().cast(),
                value.len() as i32,
                ffi::SQLITE_TRANSIENT(),
            ),
            Ok(Ok(None)) => ffi::sqlite3_result_null(context),
            _ => ffi::sqlite3_result_error(
                context,
                c"invalid input to Feeds SQLite function".as_ptr(),
                -1,
            ),
        }
    }
}

fn evaluate(kind: usize, args: &[Option<String>]) -> Result<Option<String>, ()> {
    let arg = |i: usize| args.get(i).and_then(Option::as_deref).ok_or(());
    let truth = |value: bool| Ok(Some(if value { "1" } else { "0" }.into()));
    match kind {
        0 => Ok(Some(Utc::now().naive_utc().format("%F %T%.f").to_string())),
        1 => truth(arg(0)?.to_lowercase().contains(&arg(1)?.to_lowercase())),
        2 => {
            let mut pattern = String::from("(?is)\\A");
            let mut chars = arg(1)?.chars();
            while let Some(c) = chars.next() {
                match c {
                    '%' => pattern.push_str(".*"),
                    '_' => pattern.push('.'),
                    '\\' => pattern.push_str(&regex::escape(&chars.next().ok_or(())?.to_string())),
                    c => pattern.push_str(&regex::escape(&c.to_string())),
                }
            }
            pattern.push_str("\\z");
            truth(
                regex::Regex::new(&pattern)
                    .map_err(|_| ())?
                    .is_match(arg(0)?),
            )
        }
        3 => {
            let value: Value = serde_json::from_str(arg(0)?).map_err(|_| ())?;
            truth(property_matches(
                &value,
                arg(1)?,
                arg(2)?,
                args.get(3).and_then(Option::as_deref).unwrap_or(""),
            ))
        }
        4 => {
            let raw_time = arg(0)?;
            let time = NaiveDateTime::parse_from_str(raw_time, "%Y-%m-%d %H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(raw_time, "%Y-%m-%dT%H:%M:%S%.f"))
                .map_err(|_| ())?;
            let zone: chrono_tz::Tz = arg(1)?.parse().map_err(|_| ())?;
            Ok(Some(
                Utc.from_utc_datetime(&time)
                    .with_timezone(&zone)
                    .date_naive()
                    .to_string(),
            ))
        }
        5 => {
            let mut value: Value = serde_json::from_str(arg(0)?).map_err(|_| ())?;
            let object = value.as_object_mut().ok_or(())?;
            object.insert(arg(1)?.into(), Value::String(arg(2)?.into()));
            Ok(Some(value.to_string()))
        }
        _ => Err(()),
    }
}

pub fn property_matches(properties: &Value, key: &str, op: &str, expected: &str) -> bool {
    let Some(value) = properties.get(key) else {
        return false;
    };
    if op == "exists" {
        return true;
    }
    if value.is_null() {
        return false;
    }
    let actual = match value {
        Value::String(s) => s.clone(),
        other => pg_json_text(other),
    };
    match op {
        "eq" => actual == expected,
        "lt" | "gt" => decimal_compare(&actual, expected).is_some_and(|order| {
            order
                == if op == "lt" {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
        }),
        _ => false,
    }
}

fn pg_json_text(value: &Value) -> String {
    match value {
        Value::Number(number) => pg_number_text(&number.to_string()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(pg_json_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => {
            let mut pairs: Vec<_> = map.iter().collect();
            pairs.sort_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
            format!(
                "{{{}}}",
                pairs
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap(),
                        pg_json_text(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        _ => value.to_string(),
    }
}

fn pg_number_text(raw: &str) -> String {
    // PostgreSQL jsonb expands exponents and preserves the resulting numeric
    // scale. Do this with digits, never a floating-point round trip.
    let unsigned = raw.trim_start_matches('-');
    let (base, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let Ok(exponent) = exponent.parse::<i64>() else {
        return raw.into();
    };
    let integer_length = base.find('.').unwrap_or(base.len()) as i64;
    let digits = base.replace('.', "");
    let Some(point) = integer_length.checked_add(exponent) else {
        return raw.into();
    };
    // Bound work even for a new JSON number outside PostgreSQL numeric's range.
    if !(-16383..=131072).contains(&point) || digits.len() as i64 - point > 16383 {
        return raw.into();
    }
    let mut text = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point >= digits.len() as i64 {
        format!("{}{}", digits, "0".repeat(point as usize - digits.len()))
    } else {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    };
    text = text.trim_start_matches('0').to_owned();
    if text.is_empty() || text.starts_with('.') {
        text.insert(0, '0');
    }
    if raw.starts_with('-') && digits.bytes().any(|digit| digit != b'0') {
        text.insert(0, '-');
    }
    text
}

fn decimal(value: &str) -> Option<(bool, String, i64)> {
    // Do not trim here: PostgreSQL's property numeric test rejects whitespace.
    if crate::feed::numeric(value)?.len() != value.len() {
        return None;
    }
    let negative = value.starts_with('-');
    let value = value.trim_start_matches(['-', '+']);
    let (base, exponent) = value
        .split_once(['e', 'E'])
        .map_or(Some((value, 0)), |(b, e)| Some((b, e.parse::<i64>().ok()?)))?;
    let fraction = base.split_once('.').map_or(0, |(_, f)| f.len() as i64);
    let mut digits = base.replace('.', "").trim_start_matches('0').to_owned();
    if digits.is_empty() {
        return Some((false, "0".into(), 0));
    }
    let mut scale = exponent.checked_sub(fraction)?;
    while digits.ends_with('0') {
        digits.pop();
        scale = scale.checked_add(1)?;
    }
    Some((negative, digits, scale))
}

pub fn decimal_compare(left: &str, right: &str) -> Option<Ordering> {
    let (ln, ld, ls) = decimal(left)?;
    let (rn, rd, rs) = decimal(right)?;
    if ln != rn {
        return Some(if ln {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let order = if ld == "0" || rd == "0" {
        (ld != "0").cmp(&(rd != "0"))
    } else {
        (ld.len() as i128 + ls as i128)
            .cmp(&(rd.len() as i128 + rs as i128))
            .then_with(|| {
                ld.bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(ld.len().max(rd.len()))
                    .cmp(
                        rd.bytes()
                            .chain(std::iter::repeat(b'0'))
                            .take(ld.len().max(rd.len())),
                    )
            })
    };
    Some(if ln { order.reverse() } else { order })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_number_text_matches_postgres_exponents_without_rounding() {
        for (raw, expected) in [
            ("1e3", "1000"),
            ("1.00e1", "10.0"),
            ("1.0e-2", "0.010"),
            ("-0.00", "0.00"),
            ("0e2", "0"),
            ("0.001e3", "1"),
            ("9007199254740993", "9007199254740993"),
        ] {
            assert_eq!(pg_number_text(raw), expected);
            let properties: Value = serde_json::from_str(&format!("{{\"n\":{raw}}}")).unwrap();
            assert!(property_matches(&properties, "n", "eq", expected));
        }
    }
    #[test]
    fn decimal_comparison_keeps_arbitrary_precision() {
        assert_eq!(
            decimal_compare("9007199254740993", "9007199254740992"),
            Some(Ordering::Greater)
        );
        assert_eq!(decimal_compare("1e1000", "9e999"), Some(Ordering::Greater));
        assert_eq!(decimal_compare("-1.50", "-1.5"), Some(Ordering::Equal));
        assert_eq!(decimal_compare("-.00001", "0"), Some(Ordering::Less));
        assert_eq!(decimal_compare("12x", "12"), None);
        assert_eq!(decimal_compare(" 12", "12"), None);
    }
}
