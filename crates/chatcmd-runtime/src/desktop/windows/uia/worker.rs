use super::{FocusedElementSafety, Reply, Request, Snapshot, focus};
use crate::desktop::{DesktopElement, DesktopElementAction, DesktopRect};
use crate::{RuntimeError, RuntimeResult};
use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc,
};
use uiautomation::{
    UIAutomation, UIElement, UITreeWalker,
    core::UICacheRequest,
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UISelectionItemPattern, UITogglePattern,
        UIValuePattern,
    },
    types::{Handle, TreeScope, UIProperty},
    variants::Value,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::super::{ElementSignature, NativeElement, backend_error};

const MAX_ELEMENTS: usize = 256;
const MAX_DEPTH: usize = 12;

pub(super) fn run(requests: mpsc::Receiver<Request>, ready: mpsc::SyncSender<RuntimeResult<()>>) {
    let mut worker = match Worker::new() {
        Ok(worker) => {
            if ready.send(Ok(())).is_err() {
                shutdown(worker);
                return;
            }
            worker
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while let Ok(request) = requests.recv() {
        match request {
            Request::Snapshot { handle, reply } => {
                respond(reply, worker.snapshot(handle));
            }
            Request::Act {
                handle,
                target,
                action,
                reply,
            } => respond(reply, worker.act(handle, &target, &action)),
            Request::FocusedElementSafety { handle, reply } => {
                respond(reply, worker.focused_element_safety(handle));
            }
            Request::Shutdown => break,
        }
    }
    shutdown(worker);
}

fn respond<T>(reply: Reply<T>, result: RuntimeResult<T>) {
    let _ = reply.send(result);
}

struct Worker {
    automation: UIAutomation,
    walker: UITreeWalker,
    element_cache: UICacheRequest,
    last_snapshot_handle: Option<isize>,
    last_snapshot_elements: HashMap<Vec<usize>, UIElement>,
}

impl Worker {
    fn new() -> RuntimeResult<Self> {
        // SAFETY: this worker owns its thread and balances every successful call with
        // `CoUninitialize` in `shutdown` or this function's error path.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(backend_error)?;
        let result = (|| {
            let automation = UIAutomation::new_direct().map_err(backend_error)?;
            let walker = automation
                .get_control_view_walker()
                .map_err(backend_error)?;
            let element_cache = create_element_cache(&automation)?;
            Ok(Self {
                automation,
                walker,
                element_cache,
                last_snapshot_handle: None,
                last_snapshot_elements: HashMap::with_capacity(MAX_ELEMENTS),
            })
        })();
        if result.is_err() {
            // SAFETY: `CoInitializeEx` succeeded on this same thread above.
            unsafe { CoUninitialize() };
        }
        result
    }
}

fn shutdown(worker: Worker) {
    drop(worker);
    // SAFETY: `Worker::new` initialized COM exactly once on this same thread.
    unsafe { CoUninitialize() };
}

impl Worker {
    fn snapshot(&mut self, handle: isize) -> RuntimeResult<Snapshot> {
        self.last_snapshot_handle = None;
        self.last_snapshot_elements.clear();
        let root = self
            .automation
            .element_from_handle_build_cache(Handle::from(handle), &self.element_cache)
            .map_err(backend_error)?;
        let mut queue = VecDeque::from([(root, Vec::<usize>::new())]);
        let mut elements = Vec::with_capacity(MAX_ELEMENTS);
        let mut visited = 0usize;
        let mut truncated = false;

        while let Some((element, path)) = queue.pop_front() {
            if !path.is_empty() {
                if visited >= MAX_ELEMENTS {
                    truncated = true;
                    break;
                }
                visited += 1;
                if let Some(description) = describe_cached_element(&element, path.clone()) {
                    self.last_snapshot_elements
                        .insert(path.clone(), element.clone());
                    elements.push(description);
                }
            }
            if path.len() >= MAX_DEPTH {
                truncated |= self.walker.get_first_child(&element).is_ok();
                continue;
            }
            let remaining = MAX_ELEMENTS.saturating_sub(visited + queue.len());
            if remaining == 0 {
                truncated |= self.walker.get_first_child(&element).is_ok();
                continue;
            }
            let (children, child_truncated) = self.cached_children_bounded(&element, remaining);
            truncated |= child_truncated;
            for (index, child) in children.into_iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index);
                queue.push_back((child, child_path));
            }
        }
        self.last_snapshot_handle = Some(handle);
        Ok((elements, truncated))
    }

    fn act(
        &mut self,
        handle: isize,
        target: &NativeElement,
        action: &DesktopElementAction,
    ) -> RuntimeResult<()> {
        let cached = (self.last_snapshot_handle == Some(handle))
            .then(|| self.last_snapshot_elements.get(&target.path).cloned())
            .flatten();
        self.last_snapshot_handle = None;
        self.last_snapshot_elements.clear();
        let element = match cached {
            Some(element) => element,
            None => {
                let root = self
                    .automation
                    .element_from_handle(Handle::from(handle))
                    .map_err(backend_error)?;
                resolve_path(&self.walker, root, &target.path)?
            }
        };
        if current_signature(&element) != target.signature {
            return Err(stale_element());
        }
        if !element.is_enabled().unwrap_or(false) {
            return Err(RuntimeError::new(
                "desktop_element_disabled",
                "the target element is disabled",
            ));
        }
        invoke_action(&element, action)
    }

    fn focused_element_safety(&self, handle: isize) -> RuntimeResult<FocusedElementSafety> {
        focus::inspect(&self.automation, &self.walker, handle)
    }

    fn cached_children_bounded(&self, element: &UIElement, limit: usize) -> (Vec<UIElement>, bool) {
        let Ok(mut current) = self
            .walker
            .get_first_child_build_cache(element, &self.element_cache)
        else {
            return (Vec::new(), false);
        };
        let mut children = Vec::with_capacity(limit.min(16));
        loop {
            if children.len() >= limit {
                return (children, true);
            }
            let next = self
                .walker
                .get_next_sibling_build_cache(&current, &self.element_cache);
            children.push(current);
            match next {
                Ok(element) => current = element,
                Err(_) => return (children, false),
            }
        }
    }
}

fn create_element_cache(automation: &UIAutomation) -> RuntimeResult<UICacheRequest> {
    let cache = automation.create_cache_request().map_err(backend_error)?;
    cache
        .set_tree_scope(TreeScope::Element)
        .map_err(backend_error)?;
    for property in [
        UIProperty::RuntimeId,
        UIProperty::Name,
        UIProperty::AutomationId,
        UIProperty::LocalizedControlType,
        UIProperty::ClassName,
        UIProperty::BoundingRectangle,
        UIProperty::IsEnabled,
        UIProperty::IsOffscreen,
        UIProperty::IsPassword,
        UIProperty::IsInvokePatternAvailable,
        UIProperty::IsValuePatternAvailable,
        UIProperty::ValueIsReadOnly,
        UIProperty::IsTogglePatternAvailable,
        UIProperty::IsSelectionItemPatternAvailable,
        UIProperty::IsExpandCollapsePatternAvailable,
    ] {
        cache.add_property(property).map_err(backend_error)?;
    }
    Ok(cache)
}

fn describe_cached_element(
    element: &UIElement,
    path: Vec<usize>,
) -> Option<(DesktopElement, NativeElement)> {
    let signature = cached_signature(element);
    let password = element.is_cached_password().unwrap_or(false);
    let supported_actions = cached_actions(element, password);
    let bounds = element
        .get_cached_bounding_rectangle()
        .ok()
        .and_then(|rect| {
            let width = rect.get_width();
            let height = rect.get_height();
            (width > 0 && height > 0).then(|| DesktopRect {
                x: rect.get_left(),
                y: rect.get_top(),
                width: u32::try_from(width).unwrap_or_default(),
                height: u32::try_from(height).unwrap_or_default(),
            })
        });
    let enabled = element.is_cached_enabled().unwrap_or(false);
    let offscreen = element.is_cached_offscreen().unwrap_or(true);
    if !is_useful(&signature, &supported_actions) {
        return None;
    }
    let public = DesktopElement {
        element_id: uuid::Uuid::new_v4().to_string(),
        name: signature.name.clone(),
        automation_id: signature.automation_id.clone(),
        control_type: signature.control_type.clone(),
        class_name: signature.class_name.clone(),
        bounds,
        enabled,
        offscreen,
        password,
        supported_actions,
    };
    Some((public, NativeElement { path, signature }))
}

fn is_useful(signature: &ElementSignature, actions: &[String]) -> bool {
    !actions.is_empty()
        || !signature.name.trim().is_empty()
        || !signature.automation_id.trim().is_empty()
}

fn cached_signature(element: &UIElement) -> ElementSignature {
    ElementSignature {
        runtime_id: cached_runtime_id(element),
        name: element.get_cached_name().unwrap_or_default(),
        automation_id: element.get_cached_automation_id().unwrap_or_default(),
        control_type: element
            .get_cached_localized_control_type()
            .unwrap_or_default(),
        class_name: element.get_cached_classname().unwrap_or_default(),
    }
}

fn cached_runtime_id(element: &UIElement) -> Vec<i32> {
    let Ok(value) = element.get_cached_property_value(UIProperty::RuntimeId) else {
        return Vec::new();
    };
    match value.get_value() {
        Ok(Value::ArrayI4(values)) => values,
        _ => Vec::new(),
    }
}

fn cached_actions(element: &UIElement, password: bool) -> Vec<String> {
    let mut actions = Vec::with_capacity(6);
    if cached_bool(element, UIProperty::IsInvokePatternAvailable) {
        actions.push("invoke".to_owned());
    }
    if !password
        && cached_bool(element, UIProperty::IsValuePatternAvailable)
        && !cached_bool(element, UIProperty::ValueIsReadOnly)
    {
        actions.push("set_value".to_owned());
    }
    if cached_bool(element, UIProperty::IsTogglePatternAvailable) {
        actions.push("toggle".to_owned());
    }
    if cached_bool(element, UIProperty::IsSelectionItemPatternAvailable) {
        actions.push("select".to_owned());
    }
    if cached_bool(element, UIProperty::IsExpandCollapsePatternAvailable) {
        actions.extend(["expand".to_owned(), "collapse".to_owned()]);
    }
    actions
}

fn cached_bool(element: &UIElement, property: UIProperty) -> bool {
    let Ok(value) = element.get_cached_property_value(property) else {
        return false;
    };
    matches!(value.get_value(), Ok(Value::BOOL(true)))
}

fn current_signature(element: &UIElement) -> ElementSignature {
    ElementSignature {
        runtime_id: element.get_runtime_id().unwrap_or_default(),
        name: element.get_name().unwrap_or_default(),
        automation_id: element.get_automation_id().unwrap_or_default(),
        control_type: element.get_localized_control_type().unwrap_or_default(),
        class_name: element.get_classname().unwrap_or_default(),
    }
}

fn resolve_path(
    walker: &UITreeWalker,
    mut element: UIElement,
    path: &[usize],
) -> RuntimeResult<UIElement> {
    for index in path {
        let mut child = walker
            .get_first_child(&element)
            .map_err(|_| stale_element())?;
        for _ in 0..*index {
            child = walker
                .get_next_sibling(&child)
                .map_err(|_| stale_element())?;
        }
        element = child;
    }
    Ok(element)
}

fn invoke_action(element: &UIElement, action: &DesktopElementAction) -> RuntimeResult<()> {
    match action {
        DesktopElementAction::Invoke => pattern::<UIInvokePattern>(element, "invoke")?.invoke(),
        DesktopElementAction::SetValue { text } => {
            if element.is_password().unwrap_or(false) {
                return Err(RuntimeError::new(
                    "desktop_password_input_denied",
                    "password fields cannot be automated",
                ));
            }
            let value = pattern::<UIValuePattern>(element, "set_value")?;
            if value.is_readonly().unwrap_or(true) {
                return Err(RuntimeError::new(
                    "desktop_element_read_only",
                    "the target element is read-only",
                ));
            }
            value.set_value(text)
        }
        DesktopElementAction::Toggle => pattern::<UITogglePattern>(element, "toggle")?.toggle(),
        DesktopElementAction::Select => {
            pattern::<UISelectionItemPattern>(element, "select")?.select()
        }
        DesktopElementAction::Expand => {
            pattern::<UIExpandCollapsePattern>(element, "expand")?.expand()
        }
        DesktopElementAction::Collapse => {
            pattern::<UIExpandCollapsePattern>(element, "collapse")?.collapse()
        }
    }
    .map_err(backend_error)
}

fn pattern<T>(element: &UIElement, action: &str) -> RuntimeResult<T>
where
    T: uiautomation::patterns::UIPattern
        + TryFrom<::windows::core::IUnknown, Error = uiautomation::Error>,
{
    element.get_pattern::<T>().map_err(|_| {
        RuntimeError::new(
            "desktop_action_unsupported",
            format!("the target element does not support {action}"),
        )
    })
}

fn stale_element() -> RuntimeError {
    RuntimeError::new(
        "desktop_element_stale",
        "the target element changed; observe the window again",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(name: &str, automation_id: &str) -> ElementSignature {
        ElementSignature {
            runtime_id: vec![1, 2],
            name: name.to_owned(),
            automation_id: automation_id.to_owned(),
            control_type: String::new(),
            class_name: String::new(),
        }
    }

    #[test]
    fn useful_element_filter_keeps_named_or_actionable_controls() {
        assert!(is_useful(&signature("Save", ""), &[]));
        assert!(is_useful(&signature("", ""), &["invoke".to_owned()]));
    }

    #[test]
    fn useful_element_filter_drops_empty_offscreen_layout_nodes() {
        assert!(!is_useful(&signature("", ""), &[]));
    }
}
