use super::super::events::Listener;
use super::super::{
    CommonProps, ElementId, ElementKind, EventType, ListenerId, UiDocument, UiEvent, Value,
};
use egui::Response;

impl UiDocument {
    /// Registers a listener for an element event.
    pub fn on(
        &mut self,
        id: impl Into<ElementId>,
        event_type: EventType,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        let listener_id = ListenerId(self.next_listener_id);
        self.next_listener_id = self.next_listener_id.wrapping_add(1).max(1);
        self.listeners
            .entry((id.into(), event_type))
            .or_default()
            .push(Listener {
                id: listener_id,
                callback: Box::new(callback),
            });
        listener_id
    }

    /// Web-style alias for on.
    pub fn add_event_listener(
        &mut self,
        id: impl Into<ElementId>,
        event_type: EventType,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, event_type, callback)
    }

    /// Registers a change listener.
    pub fn on_change(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Change, callback)
    }

    /// Registers an input listener.
    pub fn on_input(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Input, callback)
    }

    /// Registers a click listener.
    pub fn on_click(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Click, callback)
    }

    /// Registers the result of a custom widget that is rendered outside the
    /// built-in component set. This lets icon buttons and card controls keep
    /// their custom painting while still participating in document lookup and
    /// click listeners.
    pub fn record_click(&mut self, id: impl AsRef<str>, clicked: bool) -> bool {
        let id = ElementId::new(id.as_ref());
        let common = CommonProps::new(id.clone());
        self.prepare(&common, ElementKind::Button, Value::Bool(false), vec![]);
        if clicked {
            self.record_button_click(&id, Value::Bool(true));
        }
        clicked
    }

    /// Removes a previously registered listener.
    pub fn remove_event_listener(&mut self, listener_id: ListenerId) -> bool {
        let mut removed = false;
        let mut empty_keys = Vec::new();
        for (key, listeners) in &mut self.listeners {
            let before = listeners.len();
            listeners.retain(|listener| listener.id != listener_id);
            removed |= listeners.len() != before;
            if listeners.is_empty() {
                empty_keys.push(key.clone());
            }
        }
        for key in empty_keys {
            self.listeners.remove(&key);
        }
        removed
    }

    /// Queues and immediately dispatches a programmatic event.
    pub fn dispatch_event(&mut self, event: UiEvent) {
        self.queue_event(event);
        self.dispatch_pending_events();
    }

    /// Returns pending events without removing them.
    pub fn pending_events(&self) -> impl Iterator<Item = &UiEvent> {
        self.pending_events.iter()
    }

    /// Dispatches all queued events to listeners in insertion order.
    pub fn dispatch_pending_events(&mut self) {
        while let Some(event) = self.pending_events.pop_front() {
            let key = (event.id.clone(), event.event_type);
            let Some(mut listeners) = self.listeners.remove(&key) else {
                continue;
            };
            for listener in &mut listeners {
                (listener.callback)(&event);
            }
            self.listeners.entry(key).or_default().extend(listeners);
        }
    }

    pub(in crate::components) fn observe(
        &mut self,
        id: &ElementId,
        before: &Value,
        after: Value,
        response: &Response,
        event_types: &[EventType],
    ) {
        self.observe_focus(id, response);
        if response.changed() && before != &after {
            if let Some(element) = self.elements.get_mut(id) {
                element.value = after.clone();
                element.changed = true;
            }
            for event_type in event_types {
                self.queue_event(UiEvent {
                    id: id.clone(),
                    event_type: *event_type,
                    value: after.clone(),
                    previous_value: Some(before.clone()),
                });
            }
        }
        if response.lost_focus() {
            let value = self.value(id).cloned().unwrap_or(Value::None);
            self.queue_event(UiEvent::new(id.clone(), EventType::Blur, value));
        }
    }

    pub(in crate::components) fn observe_focus(&mut self, id: &ElementId, response: &Response) {
        if response.gained_focus() {
            let value = self.value(id).cloned().unwrap_or(Value::None);
            self.queue_event(UiEvent::new(id.clone(), EventType::Focus, value));
        }
    }

    pub(in crate::components) fn record_button_click(&mut self, id: &ElementId, value: Value) {
        if let Some(element) = self.elements.get_mut(id) {
            element.value = value.clone();
            element.changed = true;
            element.clicked = true;
        }
        self.queue_event(UiEvent::new(id.clone(), EventType::Click, value));
    }

    pub(super) fn queue_event(&mut self, event: UiEvent) {
        self.pending_events.push_back(event);
    }
}
