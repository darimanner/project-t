//! Add-host modal: field state, focus cycling, validation. Rendering and key
//! routing live in `workspace.rs`; this module is the testable state machine.

use core_store::{AuthKind, NewHost};

use crate::text_input::InputState;

/// Focusable fields, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Label,
    Host,
    Port,
    User,
    AuthToggle,
    Secret,
}

const FIELD_ORDER: [Field; 6] = [
    Field::Label,
    Field::Host,
    Field::Port,
    Field::User,
    Field::AuthToggle,
    Field::Secret,
];

pub struct HostForm {
    pub label: InputState,
    pub host: InputState,
    pub port: InputState,
    pub user: InputState,
    pub auth: AuthKind,
    /// Password (masked) or key path, depending on `auth`.
    pub secret: InputState,
    pub focus: Field,
    pub error: Option<String>,
}

impl Default for HostForm {
    fn default() -> Self {
        Self {
            label: InputState::default(),
            host: InputState::default(),
            port: InputState::default(),
            user: InputState::default(),
            auth: AuthKind::Password,
            secret: InputState::new_masked(),
            focus: Field::Label,
            error: None,
        }
    }
}

/// A validated form, ready to persist.
pub struct ValidatedHost {
    pub host: NewHost,
    /// Present only for password auth; goes to the keyring, never the DB.
    pub password: Option<String>,
}

impl HostForm {
    pub fn focused_input(&mut self) -> Option<&mut InputState> {
        match self.focus {
            Field::Label => Some(&mut self.label),
            Field::Host => Some(&mut self.host),
            Field::Port => Some(&mut self.port),
            Field::User => Some(&mut self.user),
            Field::AuthToggle => None,
            Field::Secret => Some(&mut self.secret),
        }
    }

    pub fn focus_next(&mut self) {
        self.focus_step(1);
    }

    pub fn focus_prev(&mut self) {
        self.focus_step(FIELD_ORDER.len() - 1);
    }

    fn focus_step(&mut self, step: usize) {
        let i = FIELD_ORDER.iter().position(|f| *f == self.focus).unwrap();
        self.focus = FIELD_ORDER[(i + step) % FIELD_ORDER.len()];
    }

    /// Selects an auth kind directly (mouse click on a chip). No-op when
    /// already selected, so the secret text is preserved.
    pub fn set_auth(&mut self, auth: AuthKind) {
        if self.auth != auth {
            self.toggle_auth();
        }
    }

    /// Flips password ⇄ key auth. The secret field switches meaning, so its
    /// text is cleared and masking follows the auth kind.
    pub fn toggle_auth(&mut self) {
        self.auth = match self.auth {
            AuthKind::Password => AuthKind::Key,
            AuthKind::Key => AuthKind::Password,
        };
        self.secret.set_text("");
        self.secret.masked = self.auth == AuthKind::Password;
    }

    /// Checks fields and produces the host to persist, or a message to display.
    pub fn validate(&self) -> Result<ValidatedHost, String> {
        let host = self.host.text().trim();
        let user = self.user.text().trim();
        if host.is_empty() {
            return Err("host is required".to_owned());
        }
        if user.is_empty() {
            return Err("user is required".to_owned());
        }
        let port_text = self.port.text().trim();
        let port: u16 = if port_text.is_empty() {
            22
        } else {
            port_text
                .parse()
                .map_err(|_| format!("invalid port '{port_text}'"))?
        };
        let label = {
            let l = self.label.text().trim();
            if l.is_empty() {
                format!("{user}@{host}")
            } else {
                l.to_owned()
            }
        };
        let secret = self.secret.text().trim();
        let (key_path, password) = match self.auth {
            AuthKind::Key => {
                if secret.is_empty() {
                    return Err("key path is required".to_owned());
                }
                (Some(secret.to_owned()), None)
            }
            AuthKind::Password => (None, Some(secret.to_owned()).filter(|s| !s.is_empty())),
        };
        Ok(ValidatedHost {
            host: NewHost {
                label,
                host: host.to_owned(),
                port,
                username: user.to_owned(),
                auth: self.auth,
                key_path,
            },
            password,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> HostForm {
        let mut f = HostForm::default();
        f.host.set_text("1.2.3.4");
        f.user.set_text("root");
        f.secret.set_text("pw");
        f
    }

    #[test]
    fn valid_password_host_defaults() {
        let v = filled().validate().unwrap();
        assert_eq!(v.host.label, "root@1.2.3.4"); // auto label
        assert_eq!(v.host.port, 22); // default port
        assert_eq!(v.host.auth, AuthKind::Password);
        assert_eq!(v.password.as_deref(), Some("pw"));
        assert!(v.host.key_path.is_none());
    }

    #[test]
    fn missing_host_or_user_rejected() {
        let mut f = filled();
        f.host.set_text("  ");
        assert!(f.validate().is_err());
        let mut f = filled();
        f.user.set_text("");
        assert!(f.validate().is_err());
    }

    #[test]
    fn bad_port_rejected_custom_port_kept() {
        let mut f = filled();
        f.port.set_text("abc");
        assert!(f.validate().is_err());
        f.port.set_text("2222");
        assert_eq!(f.validate().unwrap().host.port, 2222);
    }

    #[test]
    fn key_auth_requires_path_and_stores_no_password() {
        let mut f = filled();
        f.toggle_auth(); // → Key, secret cleared
        assert!(f.validate().is_err()); // empty key path
        f.secret.set_text("~/.ssh/id_ed25519");
        let v = f.validate().unwrap();
        assert_eq!(v.host.auth, AuthKind::Key);
        assert_eq!(v.host.key_path.as_deref(), Some("~/.ssh/id_ed25519"));
        assert!(v.password.is_none());
    }

    #[test]
    fn focus_cycles_and_toggle_flips_masking() {
        let mut f = HostForm::default();
        assert_eq!(f.focus, Field::Label);
        f.focus_prev();
        assert_eq!(f.focus, Field::Secret);
        f.focus_next();
        assert_eq!(f.focus, Field::Label);

        assert!(f.secret.masked);
        f.toggle_auth();
        assert!(!f.secret.masked); // key path is visible
    }
}
