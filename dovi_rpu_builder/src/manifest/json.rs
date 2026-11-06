//! Typed reads of manifest JSON. Every miss is an `Error::Manifest` naming the field's path, so a
//! refusal says where the file is wrong; nothing here panics.

use serde_json::{Map, Value};

use crate::error::{Error, Result};

pub(super) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Error::Manifest(message.into()))
}

pub(super) fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| Error::Manifest(format!("{path} is not an object")))
}

pub(super) fn field<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a Value> {
    map.get(key)
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is missing")))
}

/// A key that must be present and may be null (`None`); otherwise read by `read`.
pub(super) fn nullable<T>(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    read: fn(&Map<String, Value>, &str, &str) -> Result<T>,
) -> Result<Option<T>> {
    match field(map, key, path)? {
        Value::Null => Ok(None),
        _ => read(map, key, path).map(Some),
    }
}

/// A key that may be missing or null (`None`); otherwise read by `read`.
pub(super) fn optional<T>(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
    read: fn(&Map<String, Value>, &str, &str) -> Result<T>,
) -> Result<Option<T>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => read(map, key, path).map(Some),
    }
}

pub(super) fn u64_at(map: &Map<String, Value>, key: &str, path: &str) -> Result<u64> {
    field(map, key, path)?
        .as_u64()
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is not an unsigned integer")))
}

pub(super) fn i64_at(map: &Map<String, Value>, key: &str, path: &str) -> Result<i64> {
    field(map, key, path)?
        .as_i64()
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is not an integer")))
}

/// An unsigned integer that must fit `T`.
pub(super) fn uint_at<T: TryFrom<u64>>(
    map: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<T> {
    let value = u64_at(map, key, path)?;
    T::try_from(value).map_err(|_| Error::Manifest(format!("{path}.{key} {value} is out of range")))
}

pub(super) fn bool_at(map: &Map<String, Value>, key: &str, path: &str) -> Result<bool> {
    field(map, key, path)?
        .as_bool()
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is not a boolean")))
}

pub(super) fn str_at<'a>(map: &'a Map<String, Value>, key: &str, path: &str) -> Result<&'a str> {
    field(map, key, path)?
        .as_str()
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is not a string")))
}

pub(super) fn array_at<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a Vec<Value>> {
    field(map, key, path)?
        .as_array()
        .ok_or_else(|| Error::Manifest(format!("{path}.{key} is not an array")))
}

/// Every element as an unsigned integer that fits `T`.
pub(super) fn uints<T: TryFrom<u64>>(values: &[Value], path: &str) -> Result<Vec<T>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_u64()
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| Error::Manifest(format!("{path}[{index}] is out of range")))
        })
        .collect()
}

/// A SHA-256 digest as 64 lowercase hex digits.
pub(super) fn digest_at(map: &Map<String, Value>, key: &str, path: &str) -> Result<String> {
    let text = str_at(map, key, path)?;
    check_digest(text, &format!("{path}.{key}"))?;
    Ok(text.to_owned())
}

/// A SHA-256 as 64 lowercase hex digits.
pub(super) fn check_digest(text: &str, path: &str) -> Result<()> {
    let valid = text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return fail(format!("{path} is not a SHA-256 hex digest"));
    }
    Ok(())
}

/// Indices that rise strictly and stay below `count`.
pub(super) fn check_rising(indices: &[u64], count: u64, path: &str) -> Result<()> {
    for (position, &index) in indices.iter().enumerate() {
        if index >= count {
            return fail(format!("{path}[{position}] = {index} is not below {count}"));
        }
        if position > 0 && index <= indices[position - 1] {
            return fail(format!("{path} is not strictly rising at [{position}]"));
        }
    }
    Ok(())
}
