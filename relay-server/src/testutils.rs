use std::path::{Path, PathBuf};

use bytes::Bytes;
use relay_event_schema::protocol::EventId;
use relay_protocol::RuleCondition;
use relay_sampling::config::{
    DecayingFunction, RuleId, RuleType, SamplingMode, SamplingRule, SamplingValue,
};
use relay_sampling::{DynamicSamplingContext, SamplingConfig};
use uuid::Uuid;

use crate::actors::project::ProjectState;
use crate::envelope::{Envelope, Item, ItemType};
use crate::extractors::RequestMeta;

pub fn state_with_rule_and_condition(
    sample_rate: Option<f64>,
    rule_type: RuleType,
    condition: RuleCondition,
) -> ProjectState {
    let rules = match sample_rate {
        Some(sample_rate) => vec![SamplingRule {
            condition,
            sampling_value: SamplingValue::SampleRate { value: sample_rate },
            ty: rule_type,
            id: RuleId(1),
            time_range: Default::default(),
            decaying_fn: DecayingFunction::Constant,
        }],
        None => Vec::new(),
    };

    project_state_with_config(SamplingConfig {
        rules: vec![],
        rules_v2: rules,
        mode: SamplingMode::Received,
    })
}

pub fn project_state_with_config(sampling_config: SamplingConfig) -> ProjectState {
    let mut state = ProjectState::allowed();
    state.config.dynamic_sampling = Some(sampling_config);
    state
}

pub fn create_sampling_context(sample_rate: Option<f64>) -> DynamicSamplingContext {
    DynamicSamplingContext {
        trace_id: uuid::Uuid::new_v4(),
        public_key: "12345678901234567890123456789012".parse().unwrap(),
        release: None,
        environment: None,
        transaction: None,
        sample_rate,
        user: Default::default(),
        replay_id: None,
        sampled: None,
        other: Default::default(),
    }
}

/// ugly hack to build an envelope with an optional trace context
pub fn new_envelope<T: Into<String>>(with_dsc: bool, transaction_name: T) -> Box<Envelope> {
    let transaction_name = transaction_name.into();
    let dsn = "https://e12d836b15bb49d7bbf99e64295d995b:@sentry.io/42";
    let event_id = EventId::new();

    let raw_event = if with_dsc {
        format!(
            "{{\"transaction\": \"{}\", \"event_id\":\"{}\",\"dsn\":\"{}\", \"trace\": {}}}\n",
            transaction_name,
            event_id.0.as_simple(),
            dsn,
            serde_json::to_string(&create_sampling_context(None)).unwrap(),
        )
    } else {
        format!(
            "{{\"transaction\": \"{}\", \"event_id\":\"{}\",\"dsn\":\"{}\"}}\n",
            transaction_name,
            event_id.0.as_simple(),
            dsn,
        )
    };

    let bytes = Bytes::from(raw_event);

    let mut envelope = Envelope::parse_bytes(bytes).unwrap();

    let item1 = Item::new(ItemType::Transaction);
    envelope.add_item(item1);

    let item2 = Item::new(ItemType::Attachment);
    envelope.add_item(item2);

    let item3 = Item::new(ItemType::Attachment);
    envelope.add_item(item3);

    envelope
}

pub fn empty_envelope() -> Box<Envelope> {
    let dsn = "https://e12d836b15bb49d7bbf99e64295d995b:@sentry.io/42"
        .parse()
        .unwrap();

    Envelope::from_request(Some(EventId::new()), RequestMeta::new(dsn))
}

/// A helper that creates, uses and removes a temporary directory.
pub struct TempDirGuard {
    path: PathBuf,
    previous_path: PathBuf,
}

impl TempDirGuard {
    /// Creates a new temporary directory and changes into it.
    ///
    /// When the guard is `dropped`, the current working directory is switched back to the old one,
    /// and the temporary directory is removed, including any files contained in it.
    pub fn new() -> Self {
        let previous_path = std::env::current_dir().unwrap();
        let path = std::env::temp_dir().join(Uuid::new_v4().to_string());
        std::fs::create_dir(&path).unwrap();
        std::env::set_current_dir(&path).unwrap();
        Self {
            path,
            previous_path,
        }
    }

    /// Returns the path of the temporary directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.previous_path).unwrap();
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
