use super::FocusedElementSafety;
use crate::{RuntimeError, RuntimeResult};
use uiautomation::{UIAutomation, UIElement, UITreeWalker, types::Handle};

use super::super::backend_error;

const MAX_ANCESTORS: usize = 32;
const SENSITIVE_WORDS: &[&str] = &[
    "password",
    "passwd",
    "passcode",
    "credential",
    "credentials",
    "otp",
    "totp",
    "mfa",
    "pin",
    "login",
    "authentication",
];
const SENSITIVE_PHRASES: &[&str] = &[
    "one time code",
    "one time password",
    "verification code",
    "authentication code",
    "security code",
    "two factor",
    "sign in",
    "log in",
];
const SENSITIVE_COMPACT: &[&str] = &[
    "password",
    "passwd",
    "passcode",
    "credential",
    "onetimecode",
    "onetimepassword",
    "verificationcode",
    "authenticationcode",
    "securitycode",
    "twofactor",
    "signin",
    "login",
];

pub(super) fn inspect(
    automation: &UIAutomation,
    walker: &UITreeWalker,
    handle: isize,
) -> RuntimeResult<FocusedElementSafety> {
    let root = automation
        .element_from_handle(Handle::from(handle))
        .map_err(backend_error)?;
    let focused = automation.get_focused_element().map_err(backend_error)?;
    let root_process = root.get_process_id().map_err(backend_error)?;
    if focused.get_process_id().map_err(backend_error)? != root_process {
        return Err(focus_lost());
    }

    let mut current = focused;
    let mut password = false;
    let mut authentication_surface = false;
    for _ in 0..=MAX_ANCESTORS {
        password |= current.is_password().map_err(backend_error)?;
        authentication_surface |= is_authentication_surface(&current)?;
        if automation
            .compare_elements(&root, &current)
            .map_err(backend_error)?
        {
            return Ok(FocusedElementSafety {
                password,
                authentication_surface,
            });
        }
        current = walker.get_parent(&current).map_err(|_| focus_lost())?;
    }
    Err(focus_lost())
}

fn is_authentication_surface(element: &UIElement) -> RuntimeResult<bool> {
    let values = [
        element.get_name().map_err(backend_error)?,
        element.get_automation_id().map_err(backend_error)?,
        element.get_classname().map_err(backend_error)?,
    ];
    Ok(values.iter().any(|value| is_sensitive_label(value)))
}

fn is_sensitive_label(value: &str) -> bool {
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>();
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    if words.iter().any(|word| SENSITIVE_WORDS.contains(word))
        || SENSITIVE_PHRASES
            .iter()
            .any(|phrase| normalized.contains(phrase))
    {
        return true;
    }
    let compact = words.concat();
    SENSITIVE_COMPACT
        .iter()
        .any(|marker| compact.contains(marker))
}

fn focus_lost() -> RuntimeError {
    RuntimeError::new(
        "desktop_input_focus_lost",
        "keyboard focus is not inside the target window",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_authentication_labels() {
        for value in [
            "Password",
            "current_passwd",
            "One-time code",
            "verificationCodeInput",
            "Enter PIN",
            "Sign in",
        ] {
            assert!(is_sensitive_label(value), "missed {value}");
        }
    }

    #[test]
    fn ordinary_editor_labels_are_allowed() {
        for value in ["Message body", "Compose", "Search mail", "Recipient"] {
            assert!(!is_sensitive_label(value), "false positive for {value}");
        }
    }
}
