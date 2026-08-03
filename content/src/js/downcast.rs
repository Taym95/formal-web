//! Generic platform-object downcast helpers.
//!
//! These use [`ExecutionContext::with_object_any`] / `with_object_any_mut`
//! to extract native Rust data from JavaScript platform objects.

use crate::dom::{
    AbortController, AbortSignal, Document, Element, Event, EventTarget, Node, UIEvent,
};
use crate::html::{
    HTMLAnchorElement, HTMLElement, HTMLIFrameElement, HTMLInputElement, HTMLMediaElement,
    HTMLVideoElement, Window,
};
use crate::js::Types;
use js_engine::{Completion, ExecutionContext, JsTypes};

pub(crate) fn try_with_abort_signal_mut<R>(
    this: &<Types as JsTypes>::JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&mut AbortSignal, &mut dyn ExecutionContext<Types>) -> R,
) -> Completion<R, Types> {
    let obj = <Types as JsTypes>::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("abort signal receiver is not an object"))?;
    let signal_pointer: Option<*mut AbortSignal> = ec.with_object_any_mut(&obj).and_then(|data| {
        data.downcast_mut::<AbortSignal>()
            .map(|signal| signal as *mut AbortSignal)
    });
    let Some(signal_pointer) = signal_pointer else {
        return Err(ec.new_type_error("receiver is not an AbortSignal"));
    };
    // SAFETY: `obj` keeps the platform object alive for this call and the
    // reference is used only on the isolate thread.
    Ok(f(unsafe { &mut *signal_pointer }, ec))
}

pub(crate) fn try_with_abort_signal_ref<R>(
    object: &<Types as JsTypes>::JsObject,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&AbortSignal, &mut dyn ExecutionContext<Types>) -> R,
) -> Completion<R, Types> {
    let signal_pointer: Option<*const AbortSignal> = ec.with_object_any(object).and_then(|data| {
        data.downcast_ref::<AbortSignal>()
            .map(|signal| signal as *const AbortSignal)
    });
    let Some(signal_pointer) = signal_pointer else {
        return Err(ec.new_type_error("object is not an AbortSignal"));
    };
    // SAFETY: `object` keeps the platform object alive for this call.
    Ok(f(unsafe { &*signal_pointer }, ec))
}

pub(crate) fn try_with_abort_controller_ref<R>(
    object: &<Types as JsTypes>::JsObject,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&AbortController, &mut dyn ExecutionContext<Types>) -> R,
) -> Completion<R, Types> {
    let controller_pointer: Option<*const AbortController> =
        ec.with_object_any(object).and_then(|data| {
            data.downcast_ref::<AbortController>()
                .map(|controller| controller as *const AbortController)
        });
    let Some(controller_pointer) = controller_pointer else {
        return Err(ec.new_type_error("object is not an AbortController"));
    };
    // SAFETY: `object` keeps the platform object alive for this call.
    Ok(f(unsafe { &*controller_pointer }, ec))
}

pub(crate) fn try_set_event_target_reflector(
    value: &<Types as JsTypes>::JsValue,
    ec: &mut dyn ExecutionContext<Types>,
) {
    if let Some(obj) = <Types as JsTypes>::value_as_object(value) {
        let obj_clone = obj.clone();
        // AbortSignal exposes its EventTarget through a shared cell, so its
        // reflector is set after the registry borrow ends (its setter needs
        // `ec`). Clone the handle out; the clone shares the same cell. The
        // reflector is cloned up front because the walk below moves
        // `obj_clone` into whichever branch matches.
        let signal_reflector = obj_clone.clone();
        let mut signal_to_update: Option<AbortSignal> = None;
        if let Some(data) = ec.with_object_any_mut(&obj) {
            // Walk all known platform object types that embed an EventTarget.
            if let Some(window) = data.downcast_mut::<Window>() {
                window.event_target.reflector = Some(obj_clone);
            } else if let Some(document) = data.downcast_mut::<Document>() {
                document.node.event_target.reflector = Some(obj_clone);
            } else if let Some(element) = data.downcast_mut::<Element>() {
                element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(html_element) = data.downcast_mut::<HTMLElement>() {
                html_element.element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(anchor) = data.downcast_mut::<HTMLAnchorElement>() {
                anchor.html_element.element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(iframe) = data.downcast_mut::<HTMLIFrameElement>() {
                iframe.html_element.element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(media) = data.downcast_mut::<HTMLMediaElement>() {
                media.html_element.element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(input) = data.downcast_mut::<HTMLInputElement>() {
                input.html_element.element.node.event_target.reflector = Some(obj_clone);
            } else if let Some(video) = data.downcast_mut::<HTMLVideoElement>() {
                video
                    .media_element
                    .html_element
                    .element
                    .node
                    .event_target
                    .reflector = Some(obj_clone);
            } else if let Some(node) = data.downcast_mut::<Node>() {
                node.event_target.reflector = Some(obj_clone);
            } else if let Some(target) = data.downcast_mut::<EventTarget>() {
                target.reflector = Some(obj_clone);
            } else if let Some(signal) = data.downcast_mut::<AbortSignal>() {
                signal_to_update = Some(signal.clone());
            } else if let Some(event) = data.downcast_mut::<Event>() {
                event.reflector = Some(obj_clone);
            } else if let Some(ui_event) = data.downcast_mut::<UIEvent>() {
                ui_event.event.reflector = Some(obj_clone);
            }
        }
        if let Some(signal) = signal_to_update {
            signal.with_event_target_mut(
                |event_target, _ec| event_target.reflector = Some(signal_reflector),
                ec,
            );
        }
    }
}

pub(crate) fn event_target_from_js_object(
    ec: &mut dyn ExecutionContext<Types>,
    object: &<Types as JsTypes>::JsObject,
) -> Option<EventTarget> {
    ec.with_object_any(object).and_then(|data| {
        if let Some(window) = data.downcast_ref::<Window>() {
            Some(window.event_target.clone())
        } else if let Some(document) = data.downcast_ref::<Document>() {
            Some(document.node.event_target.clone())
        } else if let Some(element) = data.downcast_ref::<Element>() {
            Some(element.node.event_target.clone())
        } else if let Some(html_element) = data.downcast_ref::<HTMLElement>() {
            Some(html_element.element.node.event_target.clone())
        } else if let Some(node) = data.downcast_ref::<Node>() {
            Some(node.event_target.clone())
        } else if let Some(event_target) = data.downcast_ref::<EventTarget>() {
            Some(event_target.clone())
        } else {
            None
        }
    })
}

pub(crate) fn try_with_event_target_mut<R>(
    this: &<Types as JsTypes>::JsValue,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&mut EventTarget, &mut dyn ExecutionContext<Types>) -> R,
) -> Completion<R, Types> {
    let obj = <Types as JsTypes>::value_as_object(this)
        .ok_or_else(|| ec.new_type_error("event target receiver is not an object"))?;

    let event_target_pointer: Option<*mut EventTarget> =
        ec.with_object_any_mut(&obj).and_then(|data| {
            if let Some(window) = data.downcast_mut::<Window>() {
                Some(&mut window.event_target as *mut EventTarget)
            } else if let Some(document) = data.downcast_mut::<Document>() {
                Some(&mut document.node.event_target as *mut EventTarget)
            } else if let Some(element) = data.downcast_mut::<Element>() {
                Some(&mut element.node.event_target as *mut EventTarget)
            } else if let Some(html_element) = data.downcast_mut::<HTMLElement>() {
                Some(&mut html_element.element.node.event_target as *mut EventTarget)
            } else if let Some(anchor) = data.downcast_mut::<HTMLAnchorElement>() {
                Some(&mut anchor.html_element.element.node.event_target as *mut EventTarget)
            } else if let Some(iframe) = data.downcast_mut::<HTMLIFrameElement>() {
                Some(&mut iframe.html_element.element.node.event_target as *mut EventTarget)
            } else if let Some(media) = data.downcast_mut::<HTMLMediaElement>() {
                Some(&mut media.html_element.element.node.event_target as *mut EventTarget)
            } else if let Some(input) = data.downcast_mut::<HTMLInputElement>() {
                Some(&mut input.html_element.element.node.event_target as *mut EventTarget)
            } else if let Some(video) = data.downcast_mut::<HTMLVideoElement>() {
                Some(
                    &mut video.media_element.html_element.element.node.event_target
                        as *mut EventTarget,
                )
            } else if let Some(node) = data.downcast_mut::<Node>() {
                Some(&mut node.event_target as *mut EventTarget)
            } else if let Some(target) = data.downcast_mut::<EventTarget>() {
                Some(target as *mut EventTarget)
            } else {
                None
            }
        });
    if let Some(event_target_pointer) = event_target_pointer {
        // SAFETY: `obj` keeps the platform object alive for this call and the
        // reference is used only on the isolate thread.
        return Ok(f(unsafe { &mut *event_target_pointer }, ec));
    }
    // Fall back to the AbortSignal path, which exposes its EventTarget through
    // the shared cell.
    let signal_pointer: Option<*const AbortSignal> = ec.with_object_any(&obj).and_then(|data| {
        data.downcast_ref::<AbortSignal>()
            .map(|signal| signal as *const AbortSignal)
    });
    let Some(signal_pointer) = signal_pointer else {
        return Err(ec.new_type_error("receiver is not an EventTarget"));
    };
    // SAFETY: `obj` keeps the platform object alive for this call.
    let signal = unsafe { &*signal_pointer };
    // The closure receives the execution context that
    // `with_event_target_mut` passes alongside the borrowed event target.
    Ok(signal.with_event_target_mut(|event_target, ec| f(event_target, ec), ec))
}

pub(crate) fn with_abort_signal_ref<R>(
    object: &<Types as JsTypes>::JsObject,
    ec: &mut dyn ExecutionContext<Types>,
    f: impl FnOnce(&AbortSignal, &mut dyn ExecutionContext<Types>) -> R,
) -> Completion<R, Types> {
    let type_error = ec.new_type_error("object is not an AbortSignal");
    let signal_pointer = ec
        .with_object_any(object)
        .and_then(|data| data.downcast_ref::<AbortSignal>())
        .map(|signal| signal as *const AbortSignal)
        .ok_or(type_error)?;
    // SAFETY: `object` keeps the platform object alive for this call.
    let signal = unsafe { &*signal_pointer };
    Ok(f(signal, ec))
}
