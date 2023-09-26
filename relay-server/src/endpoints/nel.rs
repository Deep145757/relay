//! Endpoints for Network Error Logging reports.

use axum::extract::{DefaultBodyLimit, FromRequest};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{post, MethodRouter};
use bytes::Bytes;
use relay_config::Config;
use relay_event_schema::protocol::EventId;
use serde_json::Value;

use crate::endpoints::common::{self, BadStoreRequest};
use crate::envelope::{ContentType, Envelope, Item, ItemType};
use crate::extractors::{Mime, RequestMeta};
use crate::service::ServiceState;

#[derive(Debug, FromRequest)]
#[from_request(state(ServiceState))]
struct NelReportParams {
    meta: RequestMeta,
    body: Bytes,
}

impl NelReportParams {
    fn extract_envelope(self) -> Result<Box<Envelope>, BadStoreRequest> {
        let Self { meta, body } = self;

        if body.is_empty() {
            return Err(BadStoreRequest::EmptyBody);
        }

        let items: Value = serde_json::from_slice(&body).map_err(BadStoreRequest::InvalidJson)?;
        let mut envelope = Envelope::from_request(Some(EventId::new()), meta);

        // Iterate only if the body contains the list of the items.
        for item in items.as_array().into_iter().flatten() {
            let mut report_item = Item::new(ItemType::Nel);
            report_item.set_payload(ContentType::Json, item.to_owned().to_string());
            envelope.add_item(report_item);
        }

        Ok(envelope)
    }
}

fn is_nel_mime(mime: Mime) -> bool {
    let ty = mime.type_().as_str();
    let subty = mime.subtype().as_str();
    let suffix = mime.suffix().map(|suffix| suffix.as_str());

    matches!(
        (ty, subty, suffix),
        ("application", "json", None) | ("application", "reports", Some("json"))
    )
}

/// This handles all messages coming on the NEL endpoint.
async fn handle(
    state: ServiceState,
    mime: Mime,
    params: NelReportParams,
) -> Result<impl IntoResponse, BadStoreRequest> {
    if !is_nel_mime(mime) {
        return Ok(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response());
    }

    let envelope = params.extract_envelope()?;
    common::handle_envelope(&state, envelope).await?;
    Ok(().into_response())
}

pub fn route<B>(config: &Config) -> MethodRouter<ServiceState, B>
where
    B: axum::body::HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<axum::BoxError>,
{
    post(handle).route_layer(DefaultBodyLimit::max(config.max_event_size()))
}
