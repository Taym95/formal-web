use crate::dom::Event;
use crate::dom::event::HasEvent;
use crate::js::Types;
use blitz_traits::events::{DomEvent, EventState};
use js_engine::{ExecutionContext, JsTypes, gc_struct};

type JsObject = <Types as JsTypes>::JsObject;

/// <https://w3c.github.io/uievents/#interface-uievent>
#[gc_struct]
pub struct UIEvent {
    /// <https://dom.spec.whatwg.org/#event>
    pub event: Event,

    /// <https://w3c.github.io/uievents/#dom-uievent-view>
    pub view: Option<JsObject>,

    /// <https://w3c.github.io/uievents/#dom-uievent-detail>
    #[ignore_trace]
    pub detail: i32,
}

/// <https://w3c.github.io/uievents/#dictdef-uieventinit>
pub(crate) struct UIEventInit {
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub detail: i32,
}

/// <https://w3c.github.io/pointerevents/#interface-mouseevent>
#[gc_struct]
pub struct MouseEvent {
    /// <https://w3c.github.io/uievents/#interface-uievent>
    pub ui_event: UIEvent,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-screenx>
    #[ignore_trace]
    pub screen_x: f64,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-screeny>
    #[ignore_trace]
    pub screen_y: f64,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-clientx>
    #[ignore_trace]
    pub client_x: f64,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-clienty>
    #[ignore_trace]
    pub client_y: f64,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-button>
    #[ignore_trace]
    pub button: i16,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-buttons>
    #[ignore_trace]
    pub buttons: u16,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-ctrlkey>
    #[ignore_trace]
    pub ctrl_key: bool,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-shiftkey>
    #[ignore_trace]
    pub shift_key: bool,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-altkey>
    #[ignore_trace]
    pub alt_key: bool,

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-metakey>
    #[ignore_trace]
    pub meta_key: bool,
}

/// <https://w3c.github.io/pointerevents/#dictdef-mouseeventinit>
pub(crate) struct MouseEventInit {
    pub bubbles: bool,
    pub cancelable: bool,
    pub composed: bool,
    pub detail: i32,
    pub screen_x: f64,
    pub screen_y: f64,
    pub client_x: f64,
    pub client_y: f64,
    pub button: i16,
    pub buttons: u16,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
}

impl HasEvent for UIEvent {
    fn event(&self) -> &Event {
        &self.event
    }

    fn event_mut(&mut self) -> &mut Event {
        &mut self.event
    }
}

impl HasEvent for MouseEvent {
    fn event(&self) -> &Event {
        &self.ui_event.event
    }

    fn event_mut(&mut self) -> &mut Event {
        &mut self.ui_event.event
    }
}

impl UIEvent {
    /// <https://w3c.github.io/uievents/#dom-uievent-uievent>
    pub(crate) fn new(
        type_: String,
        init: UIEventInit,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        // Step 1: Let e be the result of creating a new UIEvent object, with its
        //         type attribute set to type, and each of the following attributes
        //         initialized to the corresponding values of eventInitDict: view,
        //         and detail.
        // Note: view is not yet resolved from the eventTarget's Window; it stays
        // None until the event is dispatched through the UI-event pipeline.
        // Step 2: Initialize e's bubbles, cancelable, and composed attributes to
        //         the values of the corresponding members of eventInitDict.
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
            view: None,
            detail: init.detail,
        }
    }

    /// <https://w3c.github.io/uievents/#initialize-a-uievent>
    pub(crate) fn from_dom_event(
        dom_event: &DomEvent,
        view: Option<JsObject>,
        time_stamp: f64,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        Self {
            event: Event::new(
                dom_event.name().to_owned(),
                dom_event.bubbles,
                dom_event.cancelable,
                false,
                true,
                time_stamp,
                ec,
            ),
            view,
            detail: 0,
        }
    }

    /// <https://w3c.github.io/uievents/#dom-uievent-view>
    pub(crate) fn view_value(&self) -> Option<JsObject> {
        self.view.clone()
    }

    /// <https://w3c.github.io/uievents/#dom-uievent-detail>
    pub(crate) fn detail_value(&self) -> i32 {
        self.detail
    }

    pub(crate) fn apply_to_event_state(
        &self,
        event_state: &mut EventState,
        ec: &mut dyn ExecutionContext<Types>,
    ) {
        if *self.event.canceled_flag.borrow(ec) {
            event_state.prevent_default();
        }
    }
}

impl MouseEvent {
    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-mouseevent>
    pub(crate) fn new(
        type_: String,
        init: MouseEventInit,
        ec: &mut dyn ExecutionContext<Types>,
    ) -> Self {
        // Step 1: Let e be the result of creating a new MouseEvent object, with its
        //         type attribute set to type, and each of the following attributes
        //         initialized to the corresponding values of eventInitDict: screenX,
        //         screenY, clientX, clientY, ctrlKey, shiftKey, altKey, metaKey,
        //         button, buttons, relatedTarget, and view.
        // Note: relatedTarget and view are not yet modeled; both stay null.
        // Step 2: Initialize e's bubbles, cancelable, and composed attributes to
        //         the values of the corresponding members of eventInitDict.
        // Step 3: Set e's detail attribute to the value of eventInitDict's detail
        //         member.
        Self {
            ui_event: UIEvent {
                event: Event::new(
                    type_,
                    init.bubbles,
                    init.cancelable,
                    init.composed,
                    false,
                    0.0,
                    ec,
                ),
                view: None,
                detail: init.detail,
            },
            screen_x: init.screen_x,
            screen_y: init.screen_y,
            client_x: init.client_x,
            client_y: init.client_y,
            button: init.button,
            buttons: init.buttons,
            ctrl_key: init.ctrl_key,
            shift_key: init.shift_key,
            alt_key: init.alt_key,
            meta_key: init.meta_key,
        }
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-clientx>
    pub(crate) fn client_x_value(&self) -> f64 {
        self.client_x
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-clienty>
    pub(crate) fn client_y_value(&self) -> f64 {
        self.client_y
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-screenx>
    pub(crate) fn screen_x_value(&self) -> f64 {
        self.screen_x
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-screeny>
    pub(crate) fn screen_y_value(&self) -> f64 {
        self.screen_y
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-button>
    pub(crate) fn button_value(&self) -> i16 {
        self.button
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-buttons>
    pub(crate) fn buttons_value(&self) -> u16 {
        self.buttons
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-ctrlkey>
    pub(crate) fn ctrl_key_value(&self) -> bool {
        self.ctrl_key
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-shiftkey>
    pub(crate) fn shift_key_value(&self) -> bool {
        self.shift_key
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-altkey>
    pub(crate) fn alt_key_value(&self) -> bool {
        self.alt_key
    }

    /// <https://w3c.github.io/pointerevents/#dom-mouseevent-metakey>
    pub(crate) fn meta_key_value(&self) -> bool {
        self.meta_key
    }
}
