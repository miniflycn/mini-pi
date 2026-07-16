/// Generate a title for a chat thread using the pi-commander worker.
///
/// The worker endpoint is `https://pi.raven-ai.one/api/ai` with `action: "generate-title"`.
pub fn generate_title(content: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    const AI_WORKER_URL: &str = "https://pi.raven-ai.one";

    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/ai", AI_WORKER_URL.trim_end_matches('/'));

    let body = serde_json::json!({
        "action": "generate-title",
        "content": content
    });

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("title worker returned {}: {}", status, body).into());
    }

    let response_json: serde_json::Value = response.json()?;

    let title = response_json
        .get("title")
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            format!(
                "unexpected title response format: {}",
                serde_json::to_string(&response_json).unwrap_or_default()
            )
        })?
        .trim()
        .to_string();

    Ok(title)
}
