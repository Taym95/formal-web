use crate::dom::Event;
use crate::dom::event::HasEvent;
use crate::js::Types;
use js_engine::gc::{GcCell, gc_cell_new};
use js_engine::gc_struct;
use js_engine::{ExecutionContext, JsTypes};

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
}
