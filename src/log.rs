use std::time::Instant;

use axum::{extract::Request, middleware::Next, response::Response};
use chrono::{DateTime, SecondsFormat, Utc};

use crate::state::NEXT_ACTIVITY_ID;

pub fn log_activity(event: &str, details: serde_json::Value) {
    let datetime = format_datetime(Utc::now());
    eprintln!("[{datetime}][{event}]{details}");
}

pub fn format_datetime(datetime: DateTime<Utc>) -> String {
    datetime.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub async fn log_request(request: Request, next: Next) -> Response {
    let request_id = NEXT_ACTIVITY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let started = Instant::now();
    log_activity(
        "http_request",
        serde_json::json!({
            "request_id": request_id, "method": method, "path": path,
        }),
    );
    let response = next.run(request).await;
    log_activity(
        "http_response",
        serde_json::json!({
            "request_id": request_id, "method": method, "path": path,
            "status": response.status().as_u16(), "duration_ms": started.elapsed().as_millis(),
        }),
    );
    response
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::format_datetime;

    #[test]
    fn formats_utc_dates_at_calendar_boundaries() {
        for (seconds, millis, expected) in [
            (0, 0, "1970-01-01T00:00:00.000Z"),
            (86_399, 999, "1970-01-01T23:59:59.999Z"),
            (86_400, 1, "1970-01-02T00:00:00.001Z"),
            (31_536_000, 0, "1971-01-01T00:00:00.000Z"),
            (951_782_400, 123, "2000-02-29T00:00:00.123Z"),
            (951_868_800, 0, "2000-03-01T00:00:00.000Z"),
            (4_107_542_400, 0, "2100-03-01T00:00:00.000Z"),
            (12_622_780_800, 0, "2370-01-01T00:00:00.000Z"),
        ] {
            assert_eq!(
                format_datetime(DateTime::from_timestamp(seconds, millis * 1_000_000).unwrap()),
                expected
            );
        }
    }
}
