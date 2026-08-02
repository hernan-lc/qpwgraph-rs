//! Human-friendly node/port name formatting: stripping transport prefixes,
//! shortening device identifiers, and truncating long labels for the canvas.

use pw_graph_i18n::I18n;

pub(crate) fn display_node_name(name: &str, i18n: &I18n) -> String {
    let (kind_key, detail) = if let Some(detail) = name.strip_prefix("alsa_input.") {
        ("canvas.node_name_alsa_input", short_device_name(detail))
    } else if let Some(detail) = name.strip_prefix("alsa_output.") {
        ("canvas.node_name_alsa_output", short_device_name(detail))
    } else if let Some(detail) = name.strip_prefix("bluez_input.") {
        ("canvas.node_name_bluetooth_input", bluetooth_suffix(detail))
    } else if let Some(detail) = name.strip_prefix("bluez_output.") {
        (
            "canvas.node_name_bluetooth_output",
            bluetooth_suffix(detail),
        )
    } else if let Some(detail) = name.strip_prefix("bluez_capture_internal.") {
        (
            "canvas.node_name_bluetooth_capture",
            bluetooth_suffix(detail),
        )
    } else if name.starts_with("bluez_midi.") {
        ("canvas.node_name_bluetooth_midi", None)
    } else if name.starts_with("v4l2_input.") {
        ("canvas.node_name_camera_input", None)
    } else if name.starts_with("Midi Through:") {
        ("canvas.node_name_midi_through", None)
    } else {
        return name.replace(['_', '-'], " ");
    };
    let kind = i18n.text(kind_key);
    detail
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!("{kind} - {detail}"))
        .unwrap_or(kind)
}

fn short_device_name(detail: &str) -> Option<String> {
    let device = detail.split("usb-").nth(1).unwrap_or(detail);
    let words: Vec<_> = device
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .take(3)
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn bluetooth_suffix(detail: &str) -> Option<String> {
    if !detail
        .chars()
        .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '_' | '-'))
    {
        return None;
    }
    let digits: String = detail
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();
    (digits.len() >= 4).then(|| {
        let suffix = &digits[digits.len() - 4..];
        format!("{}:{}", &suffix[..2], &suffix[2..])
    })
}

/// Cleans up a raw PipeWire/ALSA port name for display: drops the leading
/// device label some backends prefix onto the port (`"Foo: Port-0"`),
/// localizes the capture/playback markers, and title-cases the result.
pub(crate) fn display_port_name(name: &str, i18n: &I18n) -> String {
    let name = name.rsplit(": ").next().unwrap_or(name);
    let name = name
        .replace("(capture)", &i18n.text("canvas.capture"))
        .replace("(playback)", &i18n.text("canvas.playback"))
        .replace(['_', '-'], " ");
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), characters.as_str())
}

pub(crate) fn compact_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let compact: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{compact}...")
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_hide_transport_prefixes_without_losing_device_identity() {
        let i18n = I18n::default();
        assert_eq!(display_node_name("Dummy-Driver", &i18n), "Dummy Driver");
        assert_eq!(
            display_node_name("bluez_output.B0:F0:0C:5E:99:5A", &i18n),
            "Bluetooth Output - 99:5A"
        );
        assert_eq!(
            display_port_name("Midi Through: Port-0 (capture)", &i18n),
            "Port 0 Capture"
        );
    }

    #[test]
    fn display_names_use_the_selected_locale() {
        let i18n = I18n::from_language("es");
        assert_eq!(
            display_node_name("bluez_input.B0:F0:0C:5E:99:5A", &i18n),
            "Entrada Bluetooth - 99:5A"
        );
        assert_eq!(
            display_port_name("Midi Through: Port-0 (capture)", &i18n),
            "Port 0 Captura"
        );
    }
}
