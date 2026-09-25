use super::{backend_error, *};
use std::collections::VecDeque;
use uiautomation::{
    UIAutomation, UIElement, UITreeWalker,
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UISelectionItemPattern, UITogglePattern,
        UIValuePattern,
    },
    types::Handle,
};

const MAX_ELEMENTS: usize = 256;
const MAX_DEPTH: usize = 12;

pub(super) fn snapshot(
    handle: isize,
) -> RuntimeResult<(Vec<(DesktopElement, NativeElement)>, bool)> {
    let automation = UIAutomation::new().map_err(backend_error)?;
    let root = automation
        .element_from_handle(Handle::from(handle))
        .map_err(backend_error)?;
    let walker = automation
        .get_control_view_walker()
        .map_err(backend_error)?;
    let mut queue = VecDeque::from([(root, Vec::<usize>::new())]);
    let mut elements = Vec::new();
    let mut truncated = false;

    while let Some((element, path)) = queue.pop_front() {
        if elements.len() >= MAX_ELEMENTS {
            truncated = true;
            break;
        }
        if !path.is_empty() {
            elements.push(describe_element(&element, path.clone()));
        }
        if path.len() >= MAX_DEPTH {
            if walker
                .get_children(&element)
                .is_some_and(|children| !children.is_empty())
            {
                truncated = true;
            }
            continue;
        }
        if let Some(children) = walker.get_children(&element) {
            for (index, child) in children.into_iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index);
                queue.push_back((child, child_path));
            }
        }
    }
    Ok((elements, truncated))
}

pub(super) fn act(
    handle: isize,
    target: &NativeElement,
    action: &DesktopElementAction,
) -> RuntimeResult<()> {
    let automation = UIAutomation::new().map_err(backend_error)?;
    let root = automation
        .element_from_handle(Handle::from(handle))
        .map_err(backend_error)?;
    let walker = automation
        .get_control_view_walker()
        .map_err(backend_error)?;
    let element = resolve_path(&walker, root, &target.path)?;
    if signature(&element) != target.signature {
        return Err(RuntimeError::new(
            "desktop_element_stale",
            "the target element changed; observe the window again",
        ));
    }
    if !element.is_enabled().unwrap_or(false) {
        return Err(RuntimeError::new(
            "desktop_element_disabled",
            "the target element is disabled",
        ));
    }
    match action {
        DesktopElementAction::Invoke => pattern::<UIInvokePattern>(&element, "invoke")?.invoke(),
        DesktopElementAction::SetValue { text } => {
            if element.is_password().unwrap_or(false) {
                return Err(RuntimeError::new(
                    "desktop_password_input_denied",
                    "password fields cannot be automated",
                ));
            }
            let value = pattern::<UIValuePattern>(&element, "set_value")?;
            if value.is_readonly().unwrap_or(true) {
                return Err(RuntimeError::new(
                    "desktop_element_read_only",
                    "the target element is read-only",
                ));
            }
            value.set_value(text)
        }
        DesktopElementAction::Toggle => pattern::<UITogglePattern>(&element, "toggle")?.toggle(),
        DesktopElementAction::Select => {
            pattern::<UISelectionItemPattern>(&element, "select")?.select()
        }
        DesktopElementAction::Expand => {
            pattern::<UIExpandCollapsePattern>(&element, "expand")?.expand()
        }
        DesktopElementAction::Collapse => {
            pattern::<UIExpandCollapsePattern>(&element, "collapse")?.collapse()
        }
    }
    .map_err(backend_error)
}

fn describe_element(element: &UIElement, path: Vec<usize>) -> (DesktopElement, NativeElement) {
    let signature = signature(element);
    let password = element.is_password().unwrap_or(false);
    let value_pattern = element.get_pattern::<UIValuePattern>().ok();
    let mut supported_actions = Vec::with_capacity(5);
    if element.get_pattern::<UIInvokePattern>().is_ok() {
        supported_actions.push("invoke".to_owned());
    }
    if !password
        && value_pattern
            .as_ref()
            .is_some_and(|value| !value.is_readonly().unwrap_or(true))
    {
        supported_actions.push("set_value".to_owned());
    }
    if element.get_pattern::<UITogglePattern>().is_ok() {
        supported_actions.push("toggle".to_owned());
    }
    if element.get_pattern::<UISelectionItemPattern>().is_ok() {
        supported_actions.push("select".to_owned());
    }
    if element.get_pattern::<UIExpandCollapsePattern>().is_ok() {
        supported_actions.extend(["expand".to_owned(), "collapse".to_owned()]);
    }
    let bounds = element.get_bounding_rectangle().ok().and_then(|rect| {
        let width = rect.get_width();
        let height = rect.get_height();
        (width > 0 && height > 0).then(|| DesktopRect {
            x: rect.get_left(),
            y: rect.get_top(),
            width: u32::try_from(width).unwrap_or_default(),
            height: u32::try_from(height).unwrap_or_default(),
        })
    });
    let public = DesktopElement {
        element_id: uuid::Uuid::new_v4().to_string(),
        name: signature.name.clone(),
        automation_id: signature.automation_id.clone(),
        control_type: signature.control_type.clone(),
        class_name: signature.class_name.clone(),
        bounds,
        enabled: element.is_enabled().unwrap_or(false),
        offscreen: element.is_offscreen().unwrap_or(true),
        password,
        supported_actions,
    };
    (public, NativeElement { path, signature })
}

fn signature(element: &UIElement) -> ElementSignature {
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
        let children = walker.get_children(&element).ok_or_else(stale_element)?;
        element = children.get(*index).cloned().ok_or_else(stale_element)?;
    }
    Ok(element)
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
