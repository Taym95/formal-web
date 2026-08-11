use crate::dom::Event;
use crate::dom::event::HasEvent;
use crate::js::Types;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{Completion, ExecutionContext, JsTypes};

type JsValue = <Types as JsTypes>::JsValue;
type JsObject = <Types as JsTypes>::JsObject;

/// <https://html.spec.whatwg.org/#messageevent>
#[gc_struct]
pub struct MessageEvent {
    /// <https://dom.spec.whatwg.org/#event>
    pub event: Event,

    /// <https://html.spec.whatwg.org/#dom-messageevent-data>
    pub data: GcCell<Option<JsValue>>,

    /// <https://html.spec.whatwg.org/#dom-messageevent-origin>
    /// The message's origin: an origin, a string, or null.  Stored as the
    /// serialized origin (the empty string represents null, matching the
    /// `origin` attribute getter's null branch).
    pub origin: GcCell<String>,

    /// <https://html.spec.whatwg.org/#dom-messageevent-lasteventid>
    pub last_event_id: GcCell<String>,

    /// <https://html.spec.whatwg.org/#dom-messageevent-source>
    pub source: GcCell<Option<JsObject>>,

    /// <https://html.spec.whatwg.org/#dom-messageevent-ports>
    pub ports: GcCell<Vec<JsObject>>,

    /// <https://webidl.spec.whatwg.org/#dfn-frozen-array-type>
    /// The frozen array returned by the `ports` getter, created lazily so the
    /// attribute returns the same object on every access.
    pub ports_array: GcCell<Option<JsObject>>,
}

/// <https://html.spec.whatwg.org/#dictdef-messageeventinit>
pub(crate) struct MessageEventInit {
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub data: JsValue,
    pub origin: String,
    pub last_event_id: String,
    pub source: Option<JsObject>,
    pub ports: Vec<JsObject>,
}

impl HasEvent for MessageEvent {
    fn event(&self) -> &Event {
        &self.event
    }

    fn event_mut(&mut self) -> &mut Event {
        &mut self.event
    }
}

impl MessageEvent {
    /// <https://html.spec.whatwg.org/#dom-messageevent-messageevent>
    pub(crate) fn new(
        type_: String,
        init: MessageEventInit,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        // Step 1: Let e be the result of creating a new MessageEvent object,
        //         with its type attribute set to type, and each of the
        //         following attributes initialized to the corresponding values
        //         of eventInitDict: data, origin, lastEventId, source, and ports.
        Self {
            event: Event::new(
                type_,
                init.bubbles,
                init.cancelable,
                init.composed,
                false,
                0.0,
                ec,
            ),
            data: gc_cell_new(Some(init.data), ec),
            origin: gc_cell_new(init.origin, ec),
            last_event_id: gc_cell_new(init.last_event_id, ec),
            source: gc_cell_new(init.source, ec),
            ports: gc_cell_new(init.ports, ec),
            ports_array: gc_cell_new(None, ec),
        }
    }

    /// <https://html.spec.whatwg.org/#dom-messageevent-data>
    pub(crate) fn data_value(&self, ec: &mut dyn ExecutionContext<Types>) -> Option<JsValue> {
        self.data.borrow(ec).clone()
    }

    /// <https://html.spec.whatwg.org/#dom-messageevent-origin>
    pub(crate) fn origin_value(&self, ec: &mut dyn ExecutionContext<Types>) -> String {
        self.origin.borrow(ec).clone()
    }

    /// <https://html.spec.whatwg.org/#dom-messageevent-lasteventid>
    pub(crate) fn last_event_id_value(&self, ec: &mut dyn ExecutionContext<Types>) -> String {
        self.last_event_id.borrow(ec).clone()
    }

    /// <https://html.spec.whatwg.org/#dom-messageevent-source>
    pub(crate) fn source_value(&self, ec: &mut dyn ExecutionContext<Types>) -> Option<JsObject> {
        self.source.borrow(ec).clone()
    }

    /// <https://html.spec.whatwg.org/#dom-messageevent-ports>
    pub(crate) fn ports_value(&self, ec: &mut dyn ExecutionContext<Types>) -> Vec<JsObject> {
        self.ports.borrow(ec).clone()
    }

    /// <https://webidl.spec.whatwg.org/#dfn-frozen-array-type>
    /// Return the frozen array backing the `ports` attribute, creating it on
    /// first access so every getter call returns the same object.
    pub(crate) fn ports_value_frozen(
        &self,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Completion<JsObject, Types> {
        if let Some(array) = self.ports_array.borrow(ec).clone() {
            return Ok(array);
        }
        let ports = self.ports.borrow(ec).clone();
        let array = ec.create_empty_array();
        for (index, port) in ports.iter().enumerate() {
            let index_key = ec.property_key_from_str(&index.to_string());
            ec.set(
                array.clone(),
                index_key,
                <Types as JsTypes>::value_from_object(port.clone()),
                true,
            )?;
        }
        ec.set_integrity_level(array.clone(), js_engine::IntegrityLevel::Frozen)?;
        self.ports_array.borrow_mut(ec).replace(array.clone());
        Ok(array)
    }
}
