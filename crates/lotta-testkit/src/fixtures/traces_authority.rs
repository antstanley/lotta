use super::{
    FrameDirection, OrderingInvariant, ReferenceTrace, TraceFrame, TraceInvariantViolation, Value,
    is_server_broadcast, nested_string, violation, wire_type,
};
use crate::fixtures::sha256::lowercase_hex;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SUBSCRIBERS: usize = 8;

#[derive(Clone)]
struct Input<'a> {
    cause: &'a str,
    frame: &'a TraceFrame,
    accepted: bool,
}

#[derive(Clone)]
struct Begin<'a> {
    frame: &'a TraceFrame,
    emission: &'a str,
    message_type: &'a str,
    subscribers: Vec<u32>,
    hash: &'a str,
}

#[derive(Default)]
struct Authority<'a> {
    connections: BTreeMap<u32, bool>,
    inputs: BTreeMap<&'a str, Input<'a>>,
    accepted: BTreeSet<&'a str>,
    started: BTreeMap<&'a str, &'a TraceFrame>,
    leases: BTreeMap<&'a str, &'a str>,
    pending_replacement: BTreeMap<&'a str, (&'a str, &'a TraceFrame)>,
    emissions: BTreeSet<&'a str>,
}

pub(super) fn authoritative(trace: &ReferenceTrace) -> Result<(), TraceInvariantViolation> {
    let frames = trace.frames.as_slice();
    let mut state = Authority::default();
    let mut index = 0;
    while index < frames.len() {
        let frame = &frames[index];
        state.connection(frame)?;
        match wire_type(frame) {
            "input" => state.input(frame)?,
            "input_accepted" => state.accept(frame)?,
            "turn_started" | "turn_dequeued" => state.turn(frame)?,
            "lease_acquired" => state.acquire(frame)?,
            "lease_replaced" => state.replace(frame)?,
            "broadcast_begin" => {
                let begin = parse_begin(frame, &state)?;
                let consumed = state.broadcast(frames, index, &begin)?;
                index += consumed;
            }
            _ => Authority::protocol(frame)?,
        }
        index += 1;
    }
    if let Some((_, (_, frame))) = state.pending_replacement.first_key_value() {
        return Err(lease_error(frame, None));
    }
    Ok(())
}

impl<'a> Authority<'a> {
    fn connection(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        match wire_type(frame) {
            "open" if frame.direction == FrameDirection::Lifecycle => {
                if self.connections.insert(frame.connection_ordinal, true) == Some(true) {
                    return Err(cause_error(frame, None));
                }
            }
            "disconnect" if frame.direction == FrameDirection::Lifecycle => {
                if self.connections.insert(frame.connection_ordinal, false) != Some(true) {
                    return Err(cause_error(frame, None));
                }
            }
            _ if frame.direction != FrameDirection::Lifecycle
                && self.connections.get(&frame.connection_ordinal) != Some(&true) =>
            {
                return Err(cause_error(frame, None));
            }
            _ => {}
        }
        Ok(())
    }

    fn input(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        let request = nested_string(frame, &["request_id"]);
        let messages = frame
            .wire
            .as_value()
            .pointer("/payload/messages")
            .and_then(Value::as_array);
        let cause = messages.and_then(|items| {
            (items.len() == 1)
                .then(|| items[0].get("client_message_id").and_then(Value::as_str))
                .flatten()
        });
        let (Some(request), Some(cause)) = (request, cause) else {
            return Err(cause_error(frame, None));
        };
        if frame.caused_by.as_deref() != Some(cause)
            || self
                .inputs
                .insert(
                    request,
                    Input {
                        cause,
                        frame,
                        accepted: false,
                    },
                )
                .is_some()
        {
            return Err(cause_error(frame, None));
        }
        Ok(())
    }

    fn accept(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        let Some(request) = nested_string(frame, &["request_id"]) else {
            return Err(cause_error(frame, None));
        };
        let Some(input) = self.inputs.get_mut(request) else {
            return Err(cause_error(frame, None));
        };
        if input.accepted || frame.caused_by.as_deref() != Some(input.cause) {
            return Err(cause_error(input.frame, Some(frame)));
        }
        input.accepted = true;
        self.accepted.insert(input.cause);
        Ok(())
    }

    fn accepted_cause(&self, frame: &'a TraceFrame) -> Result<&'a str, TraceInvariantViolation> {
        let Some(cause) = frame.caused_by.as_deref() else {
            return Err(cause_error(frame, None));
        };
        if !self.accepted.contains(cause) {
            return Err(cause_error(frame, None));
        }
        Ok(cause)
    }

    fn turn(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        let cause = self.accepted_cause(frame)?;
        let lease = nested_string(frame, &["caused_by"])
            .filter(|wire| *wire == cause)
            .and_then(|_| nested_string(frame, &["lease_id"]));
        if lease != frame.lease_id.as_deref() || self.started.insert(cause, frame).is_some() {
            return Err(lease_error(frame, None));
        }
        Ok(())
    }

    fn acquire(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        let cause = self.accepted_cause(frame)?;
        let lease = nested_string(frame, &["lease_id"]);
        if nested_string(frame, &["caused_by"]) != Some(cause) || lease != frame.lease_id.as_deref()
        {
            return Err(lease_error(frame, None));
        }
        let Some(lease) = lease else {
            return Err(lease_error(frame, None));
        };
        if let Some((expected, replaced)) = self.pending_replacement.remove(cause) {
            if expected != lease {
                return Err(lease_error(replaced, Some(frame)));
            }
        } else if self
            .started
            .get(cause)
            .and_then(|start| start.lease_id.as_deref())
            != Some(lease)
        {
            return Err(lease_error(frame, None));
        }
        self.leases.insert(cause, lease);
        Ok(())
    }

    fn replace(&mut self, frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        let cause = self.accepted_cause(frame)?;
        let old = nested_string(frame, &["old_lease_id"]);
        let new = nested_string(frame, &["new_lease_id"]);
        if nested_string(frame, &["caused_by"]) != Some(cause)
            || old != self.leases.get(cause).copied()
            || new != frame.lease_id.as_deref()
            || new.is_none()
        {
            return Err(lease_error(frame, None));
        }
        let new = new.unwrap_or_default();
        self.leases.insert(cause, new);
        self.pending_replacement.insert(cause, (new, frame));
        Ok(())
    }

    fn protocol(frame: &'a TraceFrame) -> Result<(), TraceInvariantViolation> {
        if is_server_broadcast(frame) {
            return Err(group_error(frame, None));
        }
        if frame.direction == FrameDirection::ServerToClient && frame.broadcast_emission.is_some() {
            return Err(group_error(frame, None));
        }
        Ok(())
    }

    fn broadcast(
        &mut self,
        frames: &'a [TraceFrame],
        index: usize,
        begin: &Begin<'a>,
    ) -> Result<usize, TraceInvariantViolation> {
        if !self.emissions.insert(begin.emission) {
            return Err(group_error(begin.frame, None));
        }
        let mut deliveries = Vec::new();
        for frame in &frames[index + 1..] {
            if !is_server_broadcast(frame)
                || frame.broadcast_emission.as_deref() != Some(begin.emission)
            {
                break;
            }
            deliveries.push(frame);
        }
        if deliveries.len() != begin.subscribers.len() {
            return Err(group_error(begin.frame, deliveries.first().copied()));
        }
        let mut logical = None;
        for (ordinal, delivery) in begin.subscribers.iter().zip(&deliveries) {
            if delivery.connection_ordinal != *ordinal
                || wire_type(delivery) != begin.message_type
                || self.connections.get(ordinal) != Some(&true)
            {
                return Err(group_error(begin.frame, Some(delivery)));
            }
            self.delivery_authority(begin, delivery)?;
            let payload = canonical_delivery(delivery)
                .ok_or_else(|| group_error(begin.frame, Some(delivery)))?;
            if logical_payload_sha256(delivery.wire.as_value()).as_deref() != Some(begin.hash)
                || logical.as_ref().is_some_and(|first| first != &payload)
            {
                return Err(group_error(begin.frame, Some(delivery)));
            }
            logical.get_or_insert(payload);
        }
        Ok(deliveries.len())
    }

    fn delivery_authority(
        &self,
        begin: &Begin<'a>,
        delivery: &'a TraceFrame,
    ) -> Result<(), TraceInvariantViolation> {
        if let Some(cause) = delivery.caused_by.as_deref() {
            if !self.accepted.contains(cause) {
                return Err(cause_error(begin.frame, Some(delivery)));
            }
            if matches!(wire_type(delivery), "stream_delta" | "turn_finished")
                && (delivery.lease_id.as_deref() != self.leases.get(cause).copied()
                    || delivery.lease_current != Some(true))
            {
                return Err(lease_error(begin.frame, Some(delivery)));
            }
        }
        Ok(())
    }
}

fn parse_begin<'a>(
    frame: &'a TraceFrame,
    state: &Authority<'a>,
) -> Result<Begin<'a>, TraceInvariantViolation> {
    let emission = nested_string(frame, &["emission"]);
    let message_type = nested_string(frame, &["message_type"]);
    let hash = nested_string(frame, &["payload_sha256"]);
    let subscribers = frame
        .wire
        .as_value()
        .get("subscriber_ordinals")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|value| u32::try_from(value).ok())
                .collect::<Vec<_>>()
        });
    let valid_hash = hash.is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    let valid_subscribers = subscribers.as_ref().is_some_and(|values| {
        !values.is_empty()
            && values.len() <= MAX_SUBSCRIBERS
            && values.windows(2).all(|pair| pair[0] < pair[1])
    });
    if emission.is_none()
        || message_type.is_none()
        || !valid_hash
        || !valid_subscribers
        || frame.broadcast_emission.is_some()
    {
        return Err(group_error(frame, None));
    }
    if frame.caused_by.is_some() {
        state.accepted_cause(frame)?;
    }
    Ok(Begin {
        frame,
        emission: emission.unwrap_or_default(),
        message_type: message_type.unwrap_or_default(),
        subscribers: subscribers.unwrap_or_default(),
        hash: hash.unwrap_or_default(),
    })
}

fn canonical_delivery(frame: &TraceFrame) -> Option<String> {
    canonical_payload(frame.wire.as_value())
}

pub(crate) fn logical_payload_sha256(value: &Value) -> Option<String> {
    canonical_payload(value).map(|payload| lowercase_hex(payload.as_bytes()))
}

fn canonical_payload(value: &Value) -> Option<String> {
    let mut value = value.clone();
    strip_volatile(&mut value);
    sort_value(&mut value);
    serde_json::to_string(&value).ok()
}

fn strip_volatile(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["runtime", "event_seq", "emitted_at", "idempotency_key"] {
                map.remove(key);
            }
            for child in map.values_mut() {
                strip_volatile(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(strip_volatile),
        _ => {}
    }
}

fn sort_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let old = std::mem::take(map);
            let mut entries = old.into_iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, mut child) in entries {
                sort_value(&mut child);
                map.insert(key, child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(sort_value),
        _ => {}
    }
}

fn cause_error(first: &TraceFrame, second: Option<&TraceFrame>) -> TraceInvariantViolation {
    violation(
        OrderingInvariant::InputAcceptedBeforeCausedEvents,
        first,
        second,
    )
}
fn lease_error(first: &TraceFrame, second: Option<&TraceFrame>) -> TraceInvariantViolation {
    violation(
        OrderingInvariant::NoServerEventFromStaleLeaseAfterReplacement,
        first,
        second,
    )
}
fn group_error(first: &TraceFrame, second: Option<&TraceFrame>) -> TraceInvariantViolation {
    violation(
        OrderingInvariant::BroadcastDeliveryStableAscendingConnectionOrdinal,
        first,
        second,
    )
}
