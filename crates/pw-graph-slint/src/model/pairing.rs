//! Which ports belong together.
//!
//! Easy-connect matches a source group to a destination group channel by
//! channel, falling back to declaration order when the names carry no
//! channel identity of their own.

use super::*;

pub(super) fn pair_ports(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let channel_matched = pair_ports_by_channel(outputs, inputs);
    if !channel_matched.is_empty() {
        return channel_matched;
    }
    // Nothing lined up by channel — a mono endpoint meeting a stereo one, or
    // ports that carry no channel at all. Fall back to pairing them in order
    // so the drag still connects instead of reporting no compatible ports.
    pair_ports_in_order(outputs, inputs)
}

pub(super) fn pair_ports_by_channel(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let mut used = vec![false; inputs.len()];
    let mut pairs = Vec::new();
    for output in outputs {
        let candidate = inputs
            .iter()
            .enumerate()
            .filter(|(index, input)| {
                !used[*index]
                    && ports_compatible(output.port_type, input.port_type)
                    && channels_can_pair(output, input)
            })
            .max_by_key(|(index, input)| {
                (
                    channel_pair_score(output, input),
                    name_pair_score(output, input),
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(index, _)| index);
        if let Some(index) = candidate {
            used[index] = true;
            pairs.push((output.id, inputs[index].id));
        }
    }
    pairs
}

pub(super) fn pair_ports_in_order(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let mut used = vec![false; inputs.len()];
    let mut pairs = Vec::new();
    for output in outputs {
        let candidate = inputs.iter().enumerate().find(|(index, input)| {
            !used[*index] && ports_compatible(output.port_type, input.port_type)
        });
        if let Some((index, input)) = candidate {
            used[index] = true;
            pairs.push((output.id, input.id));
        }
    }
    pairs
}

pub(super) fn ports_compatible(output: PortType, input: PortType) -> bool {
    output == input || output == PortType::Unknown || input == PortType::Unknown
}

pub(super) fn channels_can_pair(output: &Port, input: &Port) -> bool {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) => output.eq_ignore_ascii_case(&input),
        _ => true,
    }
}

pub(super) fn channel_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) if output.eq_ignore_ascii_case(&input) => 100,
        (Some(_), Some(_)) => 0,
        (Some(_), None) | (None, Some(_)) => 20,
        (None, None) => 10,
    }
}

pub(super) fn name_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_base_name(output), channel_base_name(input)) {
        (Some(output), Some(input)) if output.eq_ignore_ascii_case(&input) => 10,
        _ => 0,
    }
}

pub(super) fn channel_identity(port: &Port) -> Option<String> {
    port.channel
        .as_deref()
        .map(str::trim)
        .filter(|channel| !channel.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let suffix = port
                .name
                .rsplit(['_', '-', ' ', ':', '.'])
                .next()
                .unwrap_or_default();
            is_channel_token(suffix).then(|| suffix.to_owned())
        })
}

pub(super) fn channel_base_name(port: &Port) -> Option<String> {
    const DELIMITERS: [char; 5] = ['_', '-', ' ', ':', '.'];
    let name = port.name.as_str();
    let position = name.rfind(|character| DELIMITERS.contains(&character))?;
    let (base, suffix) = name.split_at(position);
    let suffix = suffix.trim_start_matches(DELIMITERS);
    (!base.is_empty() && is_channel_token(suffix)).then(|| base.to_owned())
}

pub(super) fn is_channel_token(token: &str) -> bool {
    const TOKENS: [&str; 34] = [
        "FL", "FR", "RL", "RR", "SL", "SR", "FC", "RC", "LFE", "MONO", "LEFT", "RIGHT", "L", "R",
        "C", "FLC", "FRC", "TC", "TFL", "TFR", "TFC", "TRL", "TRR", "TRC", "BFL", "BFR", "BFC",
        "BL", "BR", "BC", "BLC", "BRC", "TBL", "TBR",
    ];
    TOKENS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(token))
        || token.strip_prefix("AUX").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
        })
}
