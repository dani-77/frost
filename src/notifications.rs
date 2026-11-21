use serde_json::Value;

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub app: String,
    pub summary: String,
    pub body: String,
}

fn field<'a>(item: &'a Value, key: &str) -> &'a str {
    item.get(key)
        .and_then(|value| value.get("data"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

pub fn parse_history(text: &str) -> Result<Vec<Entry>, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("dunst history: {err}"))?;
    let rows = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "dunst history has no data array".to_owned())?;
    Ok(rows
        .iter()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| {
            Some(Entry {
                id: item.get("id")?.get("data")?.as_u64()?,
                app: field(item, "appname").to_owned(),
                summary: field(item, "summary").to_owned(),
                body: field(item, "body").to_owned(),
            })
        })
        .collect())
}

pub async fn read() -> Result<(bool, Vec<Entry>), String> {
    let (paused, history) = iced::futures::join!(
        crate::services::command("dunstctl", &["is-paused"]),
        crate::services::command("dunstctl", &["history"]),
    );
    Ok((paused?.trim() == "true", parse_history(&history?)?))
}

pub async fn toggle_dnd() -> Result<String, String> {
    crate::services::command("dunstctl", &["set-paused", "toggle"]).await
}

pub async fn recall(id: u64) -> Result<String, String> {
    crate::services::command("dunstctl", &["history-pop", &id.to_string()]).await
}

pub async fn clear() -> Result<String, String> {
    crate::services::command("dunstctl", &["history-clear"]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dunst_history() {
        let history = r#"{"data":[[{"id":{"data":2},"appname":{"data":"mail"},"summary":{"data":"Hello"},"body":{"data":"Message"}}]]}"#;
        let entries = parse_history(history).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].summary, "Hello");
    }
}
