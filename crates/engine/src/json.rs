//! JSON lookups that cannot panic, for provider responses and tool inputs.

use serde_json::Value;

static NULL: Value = Value::Null;

/// The value at JSON `pointer` (`/choices/0/message`), or `null`.
pub fn at<'a>(v: &'a Value, pointer: &str) -> &'a Value {
    v.pointer(pointer).unwrap_or(&NULL)
}

/// Set `key` on an object; a non-object stays as it is.
pub fn set(v: &mut Value, key: &str, value: Value) {
    if let Some(o) = v.as_object_mut() {
        o.insert(key.to_owned(), value);
    }
}
